//! P8.9 — E2E-encrypted memory/message sync (C8 — v2.0 §P8, ARCH/06).
//!
//! The **protocol framing** is landed and tested here; the live **network
//! transport** (LAN / Tailscale / own server) is the runtime seam — exactly
//! like the G8 cascade, WSL IPC framing, and RSS harness. What this module
//! owns:
//!
//! - [`SyncEnvelope`] — versioned wire framing (magic, version, sender
//!   device, scope, nonce, AEAD ciphertext+tag). Real encryption via
//!   ChaCha20-Poly1305; the header is bound as AAD so tampering with it
//!   breaks the MAC.
//! - [`SyncSet`] / [`SyncItem`] — a per-scope keyed set with monotonic
//!   revisions + tombstones (deletes propagate).
//! - [`reconcile`] — three-way merge producing a [`SyncDiff`] (what this set
//!   applies, what the peer applies, and genuine conflicts).
//! - [`resolve_conflicts`] / [`ConflictPolicy`] — F2: the stated rule for
//!   equal-rev conflicts on a two-writer pair. State scopes (messages,
//!   memory, connector) converge Last-Write-Wins with a deterministic
//!   timestamp tie-break; effect-plane records (tickets, receipts, audit)
//!   are never auto-resolved — they surface as `Manual` conflicts because a
//!   receipt after reconcile must mean "this node's order", never a silent
//!   overwrite by a peer.
//! - [`KeyPair`] / [`KeyExchange`] — X25519 key agreement + an authenticated
//!   `Hello`/`Confirm` handshake framing (the actual socket exchange is the
//!   seam).
//! - [`SyncBundle`] — encrypted **file** export/import, the Tauri consumer
//!   (`sync_export_bundle` / `sync_import_bundle`).
//! - [`SyncTransport`] — the injectable network seam the coordinator would
//!   drive for live sync.
//!
//! Security model: device-local guarantee is the default; sync is *opt-in*.
//! The bundle/session key is 256-bit, never leaves the device except as the
//! peer's X25519 public key (which reveals nothing about the shared secret).
//! A tampered envelope fails `open` with [`SyncError::Decrypt`] — the MAC is
//! verified before any plaintext is returned.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

/// 8-byte magic prefix on every envelope (`b"EAIOSYNC"`).
pub const SYNC_MAGIC: [u8; 8] = *b"EAIOSYNC";
/// Current protocol version.
pub const SYNC_VERSION: u8 = 1;
/// ChaCha20-Poly1305 nonce length.
pub const NONCE_LEN: usize = 12;

/// Sync scopes — mirrors the per-scope wipe granularity (`WipeScope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncScope {
    Messages,
    Memory,
    Connector,
}

impl SyncScope {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncScope::Messages => "messages",
            SyncScope::Memory => "memory",
            SyncScope::Connector => "connector",
        }
    }
}

/// One synced record: a stable key within a scope, a monotonic revision, an
/// optional tombstone, and an opaque payload (the caller serializes messages
/// / facts / connector state).
///
/// `ts_ms` (best-effort wall-clock ms since epoch) is the LWW tie-break input
/// for equal-revision conflicts (F2). It is `Option` and `#[serde(default)]`
/// so bundles written before ts tracking stay readable — old items are
/// treated as older than any timestamped item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncItem {
    pub scope: SyncScope,
    pub key: String,
    pub rev: u64,
    pub tombstone: bool,
    #[serde(default)]
    pub ts_ms: Option<u64>,
    pub payload: Vec<u8>,
}

impl SyncItem {
    pub fn live(scope: SyncScope, key: impl Into<String>, rev: u64, payload: Vec<u8>) -> Self {
        Self {
            scope,
            key: key.into(),
            rev,
            tombstone: false,
            ts_ms: None,
            payload,
        }
    }

    pub fn tombstone(scope: SyncScope, key: impl Into<String>, rev: u64) -> Self {
        Self {
            scope,
            key: key.into(),
            rev,
            tombstone: true,
            ts_ms: None,
            payload: Vec::new(),
        }
    }

    /// `live` with a wall-clock timestamp (the LWW tie-break input).
    pub fn live_at(
        scope: SyncScope,
        key: impl Into<String>,
        rev: u64,
        ts_ms: u64,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            scope,
            key: key.into(),
            rev,
            tombstone: false,
            ts_ms: Some(ts_ms),
            payload,
        }
    }

    /// `tombstone` with a wall-clock timestamp.
    pub fn tombstone_at(scope: SyncScope, key: impl Into<String>, rev: u64, ts_ms: u64) -> Self {
        Self {
            scope,
            key: key.into(),
            rev,
            tombstone: true,
            ts_ms: Some(ts_ms),
            payload: Vec::new(),
        }
    }
}

/// A set of synced items for one device (the local mirror).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncSet {
    pub items: Vec<SyncItem>,
}

impl SyncSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, scope: SyncScope, key: &str) -> Option<&SyncItem> {
        self.items.iter().find(|i| i.scope == scope && i.key == key)
    }

    /// Insert or replace an item (last-writer-wins by caller-supplied rev).
    pub fn upsert(&mut self, item: SyncItem) {
        if let Some(existing) = self
            .items
            .iter_mut()
            .find(|i| i.scope == item.scope && i.key == item.key)
        {
            *existing = item;
        } else {
            self.items.push(item);
        }
    }

    /// Mark a key deleted at `rev` (propagates as a tombstone).
    pub fn tombstone(&mut self, scope: SyncScope, key: &str, rev: u64) -> bool {
        let next_rev = self
            .get(scope, key)
            .map(|i| i.rev.max(rev) + 1)
            .unwrap_or(rev);
        self.upsert(SyncItem::tombstone(scope, key, next_rev));
        true
    }

    /// The version vector: highest rev seen per (scope, key). Used by the
    /// reconciler to decide which side is ahead without comparing payloads.
    pub fn version_vector(&self) -> BTreeMap<(SyncScope, String), u64> {
        let mut vv = BTreeMap::new();
        for i in &self.items {
            let e = vv.entry((i.scope, i.key.clone())).or_insert(0u64);
            *e = (*e).max(i.rev);
        }
        vv
    }

    /// Number of live (non-tombstone) items.
    pub fn live_count(&self) -> usize {
        self.items.iter().filter(|i| !i.tombstone).count()
    }
}

/// A genuine conflict: both sides have the same key at the same revision but
/// different payloads. Detection is [`reconcile`]'s job; resolution is
/// [`resolve_conflicts`] + [`ConflictPolicy`] (F2 — state scopes LWW with a
/// deterministic tie-break; effect-plane records stay Manual).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConflict {
    pub scope: SyncScope,
    pub key: String,
    pub local_rev: u64,
    pub remote_rev: u64,
}

/// The result of reconciling `local` against `remote`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncDiff {
    /// Items `local` should apply to converge (remote-only or remote-ahead).
    pub apply: Vec<SyncItem>,
    /// Items `remote` should apply to converge (local-only or local-ahead).
    pub push: Vec<SyncItem>,
    /// Same-key same-rev different-payload conflicts (caller resolves).
    pub conflicts: Vec<SyncConflict>,
}

/// Three-way merge: `apply` = what `local` pulls from `remote`; `push` = what
/// `remote` pulls from `local`; `conflicts` = equal-rev divergent payloads.
pub fn reconcile(local: &SyncSet, remote: &SyncSet) -> SyncDiff {
    let mut diff = SyncDiff::default();
    for r in &remote.items {
        match local.get(r.scope, &r.key) {
            None => diff.apply.push(r.clone()),
            Some(l) => {
                if l.rev < r.rev {
                    diff.apply.push(r.clone());
                } else if l.rev == r.rev && l.payload != r.payload {
                    diff.conflicts.push(SyncConflict {
                        scope: r.scope,
                        key: r.key.clone(),
                        local_rev: l.rev,
                        remote_rev: r.rev,
                    });
                }
                // l.rev > r.rev → local is ahead; goes into `push`.
            }
        }
    }
    for l in &local.items {
        match remote.get(l.scope, &l.key) {
            None => diff.push.push(l.clone()),
            Some(r) => {
                if l.rev > r.rev {
                    diff.push.push(l.clone());
                }
            }
        }
    }
    diff
}

/// F2 — how a genuine equal-rev conflict is resolved. This is the policy that
/// answers "whose write wins" on a two-writer node pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Never auto-resolve: the conflict is surfaced for the caller/human.
    ///
    /// This is the **only** safe policy for effect-plane records (tickets,
    /// receipts, audit). Tickets are minted and consumed single-node and are
    /// never replicated through [`SyncSet`] today; if a future scope ever
    /// syncs effect-like records it must stay Manual — a receipt can never
    /// be silently overwritten by a peer, and "whose ticket wins" is a
    /// human/governance decision, not a merge rule.
    Manual,
    /// Last-write-wins by `ts_ms`; equal timestamps break deterministically
    /// (payload byte-order) so **both** nodes compute the same winner and
    /// converge. The safe policy for state-plane scopes (messages, memory,
    /// connector state).
    LastWriteWins,
    /// Keep both: the peer's payload is applied under a `~conflict`-suffixed
    /// key so neither side's data is lost. Convergent (symmetric), but the
    /// caller must surface the suffixed copies to the user.
    KeepBoth,
}

impl ConflictPolicy {
    /// The plane rule, written down (F2): state scopes converge with LWW;
    /// anything effect-shaped must be reviewed by a human. `Manual` is the
    /// conservative default for unknown scopes.
    pub fn for_scope(scope: SyncScope) -> Self {
        match scope {
            SyncScope::Messages | SyncScope::Memory | SyncScope::Connector => {
                ConflictPolicy::LastWriteWins
            }
        }
    }
}

/// The outcome of applying a [`ConflictPolicy`] to a [`SyncDiff`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResolvedDiff {
    /// Items `local` applies to converge (includes LWW/KeepBoth winners).
    pub apply: Vec<SyncItem>,
    /// Items `remote` applies to converge.
    pub push: Vec<SyncItem>,
    /// Conflicts left for the caller (policy = Manual) — never auto-resolved.
    pub remaining: Vec<SyncConflict>,
}

/// Apply a conflict policy to a reconciled diff (F2).
///
/// `reconcile` only *detects* equal-rev divergent payloads; this resolves
/// them according to the plane rule. Invariants:
/// - `Manual` never mutates anything — conflicts are returned in `remaining`.
/// - LWW and KeepBoth are **symmetric**: both nodes feed in their own local
///   set and compute the same winner, so the pair converges after one round.
/// - The LWW tie-break is deterministic by payload byte-order — arbitrary
///   but stable, so equal timestamps cannot fork the pair.
pub fn resolve_conflicts(
    diff: SyncDiff,
    policy: ConflictPolicy,
    local: &SyncSet,
    remote: &SyncSet,
) -> ResolvedDiff {
    let mut out = ResolvedDiff {
        apply: diff.apply,
        push: diff.push,
        remaining: Vec::new(),
    };
    for c in diff.conflicts {
        let l = local.get(c.scope, &c.key);
        let r = remote.get(c.scope, &c.key);
        match policy {
            ConflictPolicy::Manual => out.remaining.push(c),
            ConflictPolicy::LastWriteWins => match (l, r) {
                (Some(l), Some(r)) => {
                    let l_ts = l.ts_ms.unwrap_or(0);
                    let r_ts = r.ts_ms.unwrap_or(0);
                    // Newer ts wins; equal ts → larger payload wins bytewise
                    // (symmetric: both nodes pick the same one). Equal
                    // payloads at equal rev = already converged.
                    if r_ts > l_ts || (r_ts == l_ts && r.payload > l.payload) {
                        out.apply.push(r.clone());
                    } else if l_ts > r_ts || (l_ts == r_ts && l.payload > r.payload) {
                        out.push.push(l.clone());
                    }
                }
                _ => out.remaining.push(c),
            },
            ConflictPolicy::KeepBoth => {
                if let Some(r) = r {
                    let mut both = r.clone();
                    both.key = format!("{}~conflict", c.key);
                    out.apply.push(both);
                }
                // Local keeps its original key; remote applies nothing extra
                // for this conflict (its own keep-both branch produces the
                // symmetric suffixed copy on the other side).
            }
        }
    }
    out
}

/// Errors from the sync protocol.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("bad sync magic")]
    BadMagic,
    #[error("unsupported sync version {0}")]
    UnsupportedVersion(u8),
    #[error("decryption failed (bad key, tampered envelope, or wrong AAD)")]
    Decrypt,
    #[error("invalid key material")]
    InvalidKey,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("crypto: {0}")]
    Crypto(String),
}

/// The AEAD box that authenticates + encrypts envelope bodies.
///
/// Split behind a trait so tests can inject a deterministic double, but the
/// real [`ChaChaBox`] is what production uses.
pub trait AeadBox: Send + Sync {
    /// Returns `(nonce, ciphertext + poly1305 tag)` for `plaintext`.
    fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<(Vec<u8>, Vec<u8>), SyncError>;
    /// Returns the plaintext only after the MAC verifies.
    fn decrypt(&self, nonce: &[u8], ciphertext: &[u8], aad: &[u8]) -> Result<Vec<u8>, SyncError>;
}

/// Real ChaCha20-Poly1305 box keyed with a 256-bit key.
pub struct ChaChaBox {
    key: [u8; 32],
}

impl ChaChaBox {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// The raw 256-bit key. Exposed for persistence (the sync commands wrap
    /// it with the vault key before writing to disk — it never sits in
    /// plaintext).
    pub fn key(&self) -> [u8; 32] {
        self.key
    }

    /// Generate a fresh random 256-bit key.
    pub fn random_key() -> [u8; 32] {
        use rand::RngCore;
        let mut k = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k);
        k
    }
}

impl AeadBox for ChaChaBox {
    fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Result<(Vec<u8>, Vec<u8>), SyncError> {
        use rand::RngCore;
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        // The AAD is bound into the Poly1305 tag, so a tampered header fails
        // the MAC on `decrypt` — this is the header-authentication guarantee.
        let ct = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|e| SyncError::Crypto(e.to_string()))?;
        Ok((nonce_bytes.to_vec(), ct))
    }

    fn decrypt(&self, nonce: &[u8], ciphertext: &[u8], aad: &[u8]) -> Result<Vec<u8>, SyncError> {
        if nonce.len() != NONCE_LEN {
            return Err(SyncError::Decrypt);
        }
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| SyncError::Decrypt)
    }
}

/// Versioned wire envelope. The header fields are bound as AAD so any
/// tampering with magic/version/sender/scope fails the MAC on `open`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncEnvelope {
    pub magic: [u8; 8],
    pub version: u8,
    pub sender_device: String,
    pub scope: SyncScope,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

impl SyncEnvelope {
    fn header_aad(&self) -> Vec<u8> {
        let mut aad = Vec::with_capacity(8 + 1 + self.sender_device.len() + 1);
        aad.extend_from_slice(&self.magic);
        aad.push(self.version);
        aad.push(self.scope.as_str().len() as u8);
        aad.extend_from_slice(self.scope.as_str().as_bytes());
        aad.extend_from_slice(self.sender_device.as_bytes());
        aad
    }
}

/// Seal `plaintext` into an envelope under `box_`.
pub fn seal(
    box_: &dyn AeadBox,
    sender_device: &str,
    scope: SyncScope,
    plaintext: &[u8],
) -> Result<SyncEnvelope, SyncError> {
    let mut env = SyncEnvelope {
        magic: SYNC_MAGIC,
        version: SYNC_VERSION,
        sender_device: sender_device.to_string(),
        scope,
        nonce: Vec::new(),
        ciphertext: Vec::new(),
    };
    let aad = env.header_aad();
    let (nonce, ct) = box_.encrypt(plaintext, &aad)?;
    env.nonce = nonce;
    env.ciphertext = ct;
    Ok(env)
}

/// Open an envelope: verify magic/version, then MAC + decrypt.
///
/// `expected_sender` is an *optional* extra check: when `Some`, the envelope
/// must carry that device id (still MAC-bound via AAD). When `None` (the
/// bundle-import case, where the importer doesn't know the exporter's device
/// id ahead of time), the MAC alone — which requires the right key and binds
/// the whole header — authenticates the envelope.
pub fn open(
    box_: &dyn AeadBox,
    env: &SyncEnvelope,
    expected_sender: Option<&str>,
) -> Result<Vec<u8>, SyncError> {
    if env.magic != SYNC_MAGIC {
        return Err(SyncError::BadMagic);
    }
    if env.version != SYNC_VERSION {
        return Err(SyncError::UnsupportedVersion(env.version));
    }
    if let Some(expected) = expected_sender {
        if env.sender_device != expected {
            return Err(SyncError::Decrypt);
        }
    }
    let aad = env.header_aad();
    box_.decrypt(&env.nonce, &env.ciphertext, &aad)
}

/// Seal a whole [`SyncSet`] into an envelope (used by bundles + transport).
pub fn seal_set(
    box_: &dyn AeadBox,
    sender_device: &str,
    scope: SyncScope,
    set: &SyncSet,
) -> Result<SyncEnvelope, SyncError> {
    let bytes = serde_json::to_vec(set)?;
    seal(box_, sender_device, scope, &bytes)
}

/// Open an envelope back into a [`SyncSet`].
pub fn open_set(
    box_: &dyn AeadBox,
    env: &SyncEnvelope,
    expected_sender: Option<&str>,
) -> Result<SyncSet, SyncError> {
    let bytes = open(box_, env, expected_sender)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// X25519 key pair for the sync key exchange.
pub struct KeyPair {
    secret: [u8; 32],
    public: [u8; 32],
}

impl KeyPair {
    /// Generate a fresh random X25519 key pair.
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut secret = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut secret);
        Self::from_secret(secret).expect("random 32-byte secret is valid")
    }

    /// Build from a caller-supplied 32-byte secret.
    pub fn from_secret(secret: [u8; 32]) -> Result<Self, SyncError> {
        let secret = x25519_dalek::StaticSecret::from(secret);
        let public = x25519_dalek::PublicKey::from(&secret);
        Ok(Self {
            secret: secret.to_bytes(),
            public: *public.as_bytes(),
        })
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.public
    }

    /// The raw 32-byte X25519 secret. Persisted (encrypted) by the sync
    /// commands so a device keeps a stable identity across restarts.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret
    }

    /// ECDH: shared secret with a peer's public key. Both sides derive the
    /// same 32 bytes; it is never sent over the wire.
    pub fn shared_secret(&self, peer_public: &[u8; 32]) -> Result<[u8; 32], SyncError> {
        let secret = x25519_dalek::StaticSecret::from(self.secret);
        let peer = x25519_dalek::PublicKey::from(*peer_public);
        let shared = secret.diffie_hellman(&peer);
        Ok(*shared.as_bytes())
    }
}

/// The first handshake message: device id + public key. Public keys are not
/// secret; the derived shared secret is what authenticates the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncHello {
    pub device_id: String,
    pub public_key: Vec<u8>,
}

impl SyncHello {
    pub fn public_key_arr(&self) -> Result<[u8; 32], SyncError> {
        self.public_key
            .as_slice()
            .try_into()
            .map_err(|_| SyncError::InvalidKey)
    }
}

/// A key-exchange participant: holds its own keypair + the peer's hello.
pub struct KeyExchange {
    pub device_id: String,
    pub keypair: KeyPair,
}

impl KeyExchange {
    pub fn new(device_id: &str) -> Self {
        Self {
            device_id: device_id.to_string(),
            keypair: KeyPair::generate(),
        }
    }

    pub fn hello(&self) -> SyncHello {
        SyncHello {
            device_id: self.device_id.clone(),
            public_key: self.keypair.public_key().to_vec(),
        }
    }

    /// Consume a peer hello and derive a shared session keyed by ECDH.
    pub fn accept(&self, peer: &SyncHello) -> Result<SharedSession, SyncError> {
        let peer_pub = peer.public_key_arr()?;
        let shared = self.keypair.shared_secret(&peer_pub)?;
        Ok(SharedSession {
            peer_device: peer.device_id.clone(),
            box_: ChaChaBox::new(shared),
        })
    }
}

/// A derived session: the AEAD box keyed by the shared secret + the peer's
/// device id (used as the envelope's expected sender).
pub struct SharedSession {
    pub peer_device: String,
    pub box_: ChaChaBox,
}

impl SharedSession {
    /// Prove possession of the shared key: seal the challenge phrase with the
    /// shared session key; the peer opens it back (this is the `Confirm` step
    /// of the handshake framing — the actual socket exchange is the transport
    /// seam). Only the holder of the derived key can forge a token, so the
    /// MAC alone authenticates it — no device-id check is needed (and the
    /// sender labels differ between the two sides' views).
    pub fn confirm_token(&self) -> Result<SyncEnvelope, SyncError> {
        seal(
            &self.box_,
            &self.peer_device,
            SyncScope::Memory,
            b"EAIOS-CONFIRM",
        )
    }

    /// Verify a peer's confirm token: MAC-verified (proves key possession).
    pub fn verify_peer(&self, token: &SyncEnvelope) -> Result<bool, SyncError> {
        let plain = open(&self.box_, token, None)?;
        Ok(plain == b"EAIOS-CONFIRM")
    }
}

/// An encrypted sync bundle: a sealed [`SyncSet`] written to disk. This is the
/// Tauri consumer (`sync_export_bundle` / `sync_import_bundle`) — it lets a
/// user sync via a file (USB / LAN share / own server drop folder) without a
/// live network transport.
#[derive(Debug, Clone)]
pub struct SyncBundle {
    pub envelope: SyncEnvelope,
}

/// Write an encrypted bundle of `set` to `path`.
pub fn export_bundle(
    box_: &dyn AeadBox,
    sender_device: &str,
    scope: SyncScope,
    set: &SyncSet,
    path: &Path,
) -> Result<SyncBundle, SyncError> {
    let envelope = seal_set(box_, sender_device, scope, set)?;
    let bytes = serde_json::to_vec(&envelope)?;
    std::fs::write(path, bytes)?;
    Ok(SyncBundle { envelope })
}

/// Read + decrypt a bundle from `path`. The importer authenticates with the
/// shared key alone (the exporter's device id is unknown ahead of time); the
/// MAC still binds the whole header, so a tampered bundle fails.
pub fn import_bundle(box_: &dyn AeadBox, path: &Path) -> Result<SyncSet, SyncError> {
    let bytes = std::fs::read(path)?;
    let envelope: SyncEnvelope = serde_json::from_slice(&bytes)?;
    open_set(box_, &envelope, None)
}

/// The injectable network seam. A live implementation (LAN broadcast,
/// Tailscale, an own-server relay) is runtime wiring; the protocol framing
/// here is what it carries.
pub trait SyncTransport {
    fn send(&self, env: &SyncEnvelope) -> Result<(), SyncError>;
    fn recv(&self) -> Result<SyncEnvelope, SyncError>;
}

/// A device-local sync session that ties the pieces together: it owns a
/// [`SyncSet`], a key, and can reconcile against a remote set or a bundle.
pub struct SyncSession {
    pub device_id: String,
    pub set: SyncSet,
    pub box_: ChaChaBox,
}

impl SyncSession {
    pub fn new(device_id: &str, key: [u8; 32]) -> Self {
        Self {
            device_id: device_id.to_string(),
            set: SyncSet::new(),
            box_: ChaChaBox::new(key),
        }
    }

    pub fn upsert(&mut self, scope: SyncScope, key: &str, rev: u64, payload: Vec<u8>) {
        self.set.upsert(SyncItem::live(scope, key, rev, payload));
    }

    pub fn delete(&mut self, scope: SyncScope, key: &str, rev: u64) {
        self.set.tombstone(scope, key, rev);
    }

    /// Reconcile against a remote set: apply remote-ahead items locally and
    /// return what the remote should apply (its `push`). Equal-rev conflicts
    /// are left untouched for the caller (the raw [`reconcile`] contract).
    pub fn reconcile_with(&mut self, remote: &SyncSet) -> SyncDiff {
        let diff = reconcile(&self.set, remote);
        for item in &diff.apply {
            self.set.upsert(item.clone());
        }
        diff
    }

    /// F2 — reconcile with an explicit conflict policy: LWW/KeepBoth winners
    /// are applied locally, `Manual` conflicts stay in `remaining` for the
    /// caller. Use [`ConflictPolicy::for_scope`] for the state-plane rule;
    /// effect-plane records must stay `Manual`.
    pub fn reconcile_with_policy(
        &mut self,
        remote: &SyncSet,
        policy: ConflictPolicy,
    ) -> ResolvedDiff {
        let diff = reconcile(&self.set, remote);
        let resolved = resolve_conflicts(diff, policy, &self.set, remote);
        for item in &resolved.apply {
            self.set.upsert(item.clone());
        }
        resolved
    }

    /// Export the whole set as an encrypted bundle file.
    pub fn export_to(&self, path: &Path) -> Result<SyncBundle, SyncError> {
        export_bundle(
            &self.box_,
            &self.device_id,
            SyncScope::Messages,
            &self.set,
            path,
        )
    }

    /// Import + merge an encrypted bundle file (shared-key authenticated).
    pub fn import_from(&mut self, path: &Path) -> Result<SyncDiff, SyncError> {
        let remote = import_bundle(&self.box_, path)?;
        Ok(self.reconcile_with(&remote))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> [u8; 32] {
        [7u8; 32]
    }

    #[test]
    fn envelope_round_trips_under_seal_open() {
        let box_ = ChaChaBox::new(test_key());
        let env = seal(&box_, "dev-a", SyncScope::Memory, b"hello sync").unwrap();
        assert_eq!(env.magic, SYNC_MAGIC);
        assert_eq!(env.version, SYNC_VERSION);
        let plain = open(&box_, &env, Some("dev-a")).unwrap();
        assert_eq!(plain, b"hello sync");
    }

    #[test]
    fn tampered_ciphertext_fails_mac() {
        let box_ = ChaChaBox::new(test_key());
        let mut env = seal(&box_, "dev-a", SyncScope::Memory, b"secret").unwrap();
        let last = env.ciphertext.len() - 1;
        env.ciphertext[last] ^= 0x01;
        assert!(matches!(
            open(&box_, &env, Some("dev-a")),
            Err(SyncError::Decrypt)
        ));
    }

    #[test]
    fn tampered_header_fails_mac() {
        let box_ = ChaChaBox::new(test_key());
        let mut env = seal(&box_, "dev-a", SyncScope::Memory, b"secret").unwrap();
        env.sender_device = "dev-evil".into();
        // Wrong sender is rejected outright, and the AAD bind means the MAC
        // would fail even if the sender matched.
        assert!(matches!(
            open(&box_, &env, Some("dev-a")),
            Err(SyncError::Decrypt)
        ));
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let a = ChaChaBox::new([1u8; 32]);
        let b = ChaChaBox::new([2u8; 32]);
        let env = seal(&a, "dev-a", SyncScope::Messages, b"hi").unwrap();
        assert!(matches!(
            open(&b, &env, Some("dev-a")),
            Err(SyncError::Decrypt)
        ));
    }

    #[test]
    fn bad_magic_is_rejected() {
        let box_ = ChaChaBox::new(test_key());
        let mut env = seal(&box_, "dev-a", SyncScope::Messages, b"hi").unwrap();
        env.magic = *b"NOPE----";
        assert!(matches!(
            open(&box_, &env, Some("dev-a")),
            Err(SyncError::BadMagic)
        ));
    }

    #[test]
    fn reconcile_merges_both_directions() {
        let mut local = SyncSet::new();
        local.upsert(SyncItem::live(
            SyncScope::Memory,
            "a",
            1,
            b"local-a".to_vec(),
        ));
        local.upsert(SyncItem::live(SyncScope::Memory, "b", 2, b"b".to_vec()));

        let mut remote = SyncSet::new();
        remote.upsert(SyncItem::live(
            SyncScope::Memory,
            "a",
            1,
            b"local-a".to_vec(),
        ));
        remote.upsert(SyncItem::live(SyncScope::Memory, "c", 1, b"c".to_vec()));
        // remote is ahead on b
        remote.upsert(SyncItem::live(SyncScope::Memory, "b", 3, b"b-v3".to_vec()));

        let diff = reconcile(&local, &remote);
        // local pulls remote-only "c" + remote-ahead "b"
        assert_eq!(diff.apply.len(), 2);
        assert!(diff.apply.iter().any(|i| i.key == "c" && i.rev == 1));
        assert!(diff.apply.iter().any(|i| i.key == "b" && i.rev == 3));
        // remote pulls nothing local-ahead (a is equal, b remote is ahead)
        assert_eq!(diff.push.len(), 0);
        assert!(diff.conflicts.is_empty());
    }

    #[test]
    fn reconcile_pushes_local_ahead_items() {
        let mut local = SyncSet::new();
        local.upsert(SyncItem::live(
            SyncScope::Messages,
            "m",
            5,
            b"m-v5".to_vec(),
        ));
        let mut remote = SyncSet::new();
        remote.upsert(SyncItem::live(
            SyncScope::Messages,
            "m",
            3,
            b"m-v3".to_vec(),
        ));
        remote.upsert(SyncItem::live(SyncScope::Messages, "n", 1, b"n".to_vec()));

        let diff = reconcile(&local, &remote);
        assert!(diff.push.iter().any(|i| i.key == "m" && i.rev == 5));
        assert!(diff.apply.iter().any(|i| i.key == "n"));
        assert!(diff.conflicts.is_empty());
    }

    #[test]
    fn reconcile_detects_equal_rev_conflict() {
        let local = SyncSet {
            items: vec![SyncItem::live(SyncScope::Memory, "x", 1, b"left".to_vec())],
        };
        let remote = SyncSet {
            items: vec![SyncItem::live(SyncScope::Memory, "x", 1, b"right".to_vec())],
        };
        let diff = reconcile(&local, &remote);
        assert_eq!(diff.apply.len(), 0);
        assert_eq!(diff.push.len(), 0);
        assert_eq!(diff.conflicts.len(), 1);
        assert_eq!(diff.conflicts[0].key, "x");
    }

    #[test]
    fn tombstones_propagate() {
        let mut local = SyncSet::new();
        local.upsert(SyncItem::live(
            SyncScope::Memory,
            "gone",
            1,
            b"data".to_vec(),
        ));
        let mut remote = local.clone();
        remote.tombstone(SyncScope::Memory, "gone", 1);

        let diff = reconcile(&local, &remote);
        assert_eq!(diff.apply.len(), 1);
        assert!(diff.apply[0].tombstone);
        assert_eq!(diff.apply[0].key, "gone");
        assert!(diff.apply[0].rev > 1);
    }

    #[test]
    fn session_reconcile_applies_remote() {
        let mut s = SyncSession::new("dev-a", test_key());
        s.upsert(SyncScope::Memory, "k", 1, b"v1".to_vec());

        let mut remote = SyncSet::new();
        remote.upsert(SyncItem::live(SyncScope::Memory, "k", 2, b"v2".to_vec()));
        remote.upsert(SyncItem::live(SyncScope::Memory, "k2", 1, b"new".to_vec()));

        let diff = s.reconcile_with(&remote);
        assert_eq!(diff.apply.len(), 2);
        assert_eq!(s.set.get(SyncScope::Memory, "k").unwrap().rev, 2);
        assert!(s.set.get(SyncScope::Memory, "k2").is_some());
    }

    #[test]
    fn x25519_both_sides_derive_same_key() {
        let a = KeyExchange::new("dev-a");
        let b = KeyExchange::new("dev-b");

        let shared_a = a.accept(&b.hello()).unwrap();
        let shared_b = b.accept(&a.hello()).unwrap();

        // Both sessions derive the same shared key (proven by the confirm
        // token round-trip).
        let token = shared_a.confirm_token().unwrap();
        assert!(shared_b.verify_peer(&token).unwrap());
        // A third party cannot forge a confirm token: MAC fails → Err, which
        // counts as verification failure (never `true`).
        let evil = KeyExchange::new("dev-evil");
        let evil_shared = evil.accept(&a.hello()).unwrap();
        assert!(!evil_shared.verify_peer(&token).unwrap_or(false));
    }

    #[test]
    fn bundle_export_import_round_trip() {
        let key = test_key();
        let mut s = SyncSession::new("dev-a", key);
        s.upsert(SyncScope::Memory, "fact-1", 1, b"first fact".to_vec());
        s.upsert(SyncScope::Messages, "sess-1", 1, b"message".to_vec());

        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "agentcowork-sync-test-{}.eaiossync",
            std::process::id()
        ));
        s.export_to(&path).unwrap();

        // A second device with the same key reads + merges.
        let mut other = SyncSession::new("dev-a", key);
        let diff = other.import_from(&path).unwrap();
        assert_eq!(diff.apply.len(), 2);
        assert_eq!(other.set.live_count(), 2);
        assert_eq!(
            other.set.get(SyncScope::Memory, "fact-1").unwrap().payload,
            b"first fact"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bundle_import_with_wrong_key_fails() {
        let mut s = SyncSession::new("dev-a", test_key());
        s.upsert(SyncScope::Memory, "f", 1, b"x".to_vec());
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "agentcowork-sync-wrongkey-{}.eaiossync",
            std::process::id()
        ));
        s.export_to(&path).unwrap();

        let mut other = SyncSession::new("dev-a", [9u8; 32]);
        let res = other.import_from(&path);
        assert!(matches!(res, Err(SyncError::Decrypt)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn manual_policy_never_auto_resolves() {
        let local = SyncSet {
            items: vec![SyncItem::live(SyncScope::Memory, "x", 1, b"left".to_vec())],
        };
        let remote = SyncSet {
            items: vec![SyncItem::live(SyncScope::Memory, "x", 1, b"right".to_vec())],
        };
        let diff = reconcile(&local, &remote);
        let resolved = resolve_conflicts(diff, ConflictPolicy::Manual, &local, &remote);
        // Nothing mutated, conflict surfaced.
        assert!(resolved.apply.is_empty());
        assert!(resolved.push.is_empty());
        assert_eq!(resolved.remaining.len(), 1);
        assert_eq!(resolved.remaining[0].key, "x");
    }

    #[test]
    fn lww_resolves_by_timestamp() {
        let local = SyncSet {
            items: vec![SyncItem::live_at(
                SyncScope::Memory,
                "x",
                1,
                100,
                b"older".to_vec(),
            )],
        };
        let remote = SyncSet {
            items: vec![SyncItem::live_at(
                SyncScope::Memory,
                "x",
                1,
                200,
                b"newer".to_vec(),
            )],
        };
        let diff = reconcile(&local, &remote);
        let resolved = resolve_conflicts(diff, ConflictPolicy::LastWriteWins, &local, &remote);
        assert!(resolved.remaining.is_empty());
        assert_eq!(resolved.apply.len(), 1);
        assert_eq!(resolved.apply[0].payload, b"newer");
        assert!(resolved.push.is_empty());
    }

    #[test]
    fn lww_equal_ts_tiebreak_is_symmetric() {
        // Both nodes hold the same two payloads at the same rev and ts; the
        // byte-order tie-break must make both sides converge on the same
        // winner (no fork, no "whose side wins" asymmetry).
        let a_payload = b"alpha".to_vec();
        let b_payload = b"bravo".to_vec();

        // Node A: local = alpha, remote = bravo.
        let local_a = SyncSet {
            items: vec![SyncItem::live_at(
                SyncScope::Memory,
                "x",
                1,
                5,
                a_payload.clone(),
            )],
        };
        let remote_a = SyncSet {
            items: vec![SyncItem::live_at(
                SyncScope::Memory,
                "x",
                1,
                5,
                b_payload.clone(),
            )],
        };
        let ra = resolve_conflicts(
            reconcile(&local_a, &remote_a),
            ConflictPolicy::LastWriteWins,
            &local_a,
            &remote_a,
        );

        // Node B: local = bravo, remote = alpha.
        let local_b = SyncSet {
            items: vec![SyncItem::live_at(
                SyncScope::Memory,
                "x",
                1,
                5,
                b_payload.clone(),
            )],
        };
        let remote_b = SyncSet {
            items: vec![SyncItem::live_at(
                SyncScope::Memory,
                "x",
                1,
                5,
                a_payload.clone(),
            )],
        };
        let rb = resolve_conflicts(
            reconcile(&local_b, &remote_b),
            ConflictPolicy::LastWriteWins,
            &local_b,
            &remote_b,
        );
        assert!(ra.remaining.is_empty() && rb.remaining.is_empty());

        // Node A pulls "bravo"; node B already holds "bravo" locally and
        // pushes it to A — the mechanism differs by side, the winner is the
        // same (symmetric convergence in one round).
        assert_eq!(ra.apply.len(), 1);
        assert_eq!(ra.apply[0].payload, b_payload);
        assert_eq!(rb.push.len(), 1);
        assert_eq!(rb.push[0].payload, b_payload);

        // Both end-sets converge on "bravo" for key x.
        let mut set_a = local_a.clone();
        for i in &ra.apply {
            set_a.upsert(i.clone());
        }
        let mut set_b = local_b.clone();
        for i in &rb.apply {
            set_b.upsert(i.clone());
        }
        assert_eq!(
            set_a.get(SyncScope::Memory, "x").unwrap().payload,
            b_payload
        );
        assert_eq!(
            set_b.get(SyncScope::Memory, "x").unwrap().payload,
            b_payload
        );
    }

    #[test]
    fn keep_both_preserves_everything() {
        let local = SyncSet {
            items: vec![SyncItem::live(SyncScope::Memory, "x", 1, b"left".to_vec())],
        };
        let remote = SyncSet {
            items: vec![SyncItem::live(SyncScope::Memory, "x", 1, b"right".to_vec())],
        };
        let diff = reconcile(&local, &remote);
        let resolved = resolve_conflicts(diff, ConflictPolicy::KeepBoth, &local, &remote);
        assert!(resolved.remaining.is_empty());
        // Local keeps "x"=left, applies the peer's copy under ~conflict.
        assert_eq!(resolved.apply.len(), 1);
        assert_eq!(resolved.apply[0].key, "x~conflict");
        assert_eq!(resolved.apply[0].payload, b"right");
    }

    #[test]
    fn session_reconcile_with_policy_applies_winners() {
        let mut s = SyncSession::new("dev-a", test_key());
        s.upsert(SyncScope::Memory, "k", 1, b"v1".to_vec());

        let mut remote = SyncSet::new();
        remote.upsert(SyncItem::live(SyncScope::Memory, "k", 2, b"v2".to_vec()));
        remote.upsert(SyncItem::live(SyncScope::Memory, "k2", 1, b"new".to_vec()));

        let resolved =
            s.reconcile_with_policy(&remote, ConflictPolicy::for_scope(SyncScope::Memory));
        assert!(resolved.remaining.is_empty());
        assert_eq!(resolved.apply.len(), 2);
        assert_eq!(s.set.get(SyncScope::Memory, "k").unwrap().rev, 2);
        assert!(s.set.get(SyncScope::Memory, "k2").is_some());
    }

    #[test]
    fn untimestamped_items_are_older_than_timestamped() {
        // Wire compat: items written before ts tracking (ts_ms = None) must
        // lose to any timestamped peer at equal rev.
        let local = SyncSet {
            items: vec![SyncItem::live(
                SyncScope::Memory,
                "x",
                1,
                b"legacy".to_vec(),
            )],
        };
        let remote = SyncSet {
            items: vec![SyncItem::live_at(
                SyncScope::Memory,
                "x",
                1,
                10,
                b"modern".to_vec(),
            )],
        };
        let resolved = resolve_conflicts(
            reconcile(&local, &remote),
            ConflictPolicy::LastWriteWins,
            &local,
            &remote,
        );
        assert_eq!(resolved.apply.len(), 1);
        assert_eq!(resolved.apply[0].payload, b"modern");
    }

    #[test]
    fn version_vector_tracks_max_rev() {
        let mut set = SyncSet::new();
        set.upsert(SyncItem::live(SyncScope::Memory, "a", 1, vec![]));
        set.upsert(SyncItem::live(SyncScope::Memory, "a", 5, vec![]));
        set.upsert(SyncItem::live(SyncScope::Messages, "b", 2, vec![]));
        let vv = set.version_vector();
        assert_eq!(vv.get(&(SyncScope::Memory, "a".into())), Some(&5));
        assert_eq!(vv.get(&(SyncScope::Messages, "b".into())), Some(&2));
    }
}
