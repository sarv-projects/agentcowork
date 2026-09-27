//! P8.9 — live TCP sync transport (the seam `agentcowork-core::sync` documents).
//!
//! Direct TCP to a peer `ip:port` — covers LAN and Tailscale tailnets alike
//! (both are plain IP to the socket). Protocol per connection, fixed order:
//!
//! 1. both sides send their plaintext [`SyncHello`] (device id + X25519
//!    public key; public keys are not secret),
//! 2. each derives the ECDH [`SharedSession`],
//! 3. both sides exchange sealed confirm tokens (`EAIOS-CONFIRM`) — the MAC
//!    proves key possession,
//! 4. both sides seal their full mirror set under the session key, swap,
//!    open, and reconcile locally (one-shot bidirectional convergence).
//!
//! Frames are `[u32 BE length][JSON bytes]` (same convention as WslFrame),
//! capped at [`MAX_FRAME_BYTES`].
//!
//! Security model (honest): raw X25519 without an out-of-band check is
//! MITM-able on a hostile LAN; on Tailscale/WireGuard the tunnel is the
//! mitigation. For manual verification the UI can compare
//! [`fingerprint`] of each side's public key.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Optional post-exchange hook (the persistence seam for `SyncServer`).
type SyncObserver = Arc<dyn Fn(&SyncSession) + Send + Sync>;
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::sync::{
    KeyExchange, SharedSession, SyncEnvelope, SyncError, SyncHello, SyncSession, SyncSet, open_set,
    reconcile, seal_set,
};

/// Hard cap for one wire frame (16 MiB of JSON envelope).
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// Wire errors (transport-level; protocol errors stay [`SyncError`]).
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame too large ({0} bytes, cap {MAX_FRAME_BYTES})")]
    FrameTooLarge(u32),
    #[error("connection closed mid-frame")]
    ConnectionClosed,
    #[error("handshake failed (peer could not prove key possession)")]
    HandshakeFailed,
    #[error("protocol error: {0}")]
    Protocol(#[from] SyncError),
}

/// Write one length-prefixed frame.
pub fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), WireError> {
    let len = u32::try_from(bytes.len()).map_err(|_| WireError::FrameTooLarge(u32::MAX))?;
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()?;
    Ok(())
}

/// Read one length-prefixed frame (cap-enforced).
pub fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, WireError> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge(len));
    }
    let mut body = vec![0u8; len as usize];
    stream.read_exact(&mut body)?;
    Ok(body)
}

/// Short hex fingerprint of a public key (SHA-256, first 16 hex chars) so
/// two humans can compare keys out-of-band before trusting a pairing.
pub fn fingerprint(public_key: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(public_key);
    let digest = h.finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Result of one successful sync exchange with a peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeOutcome {
    pub peer_device: String,
    /// Peer pubkey fingerprint (for UI display / out-of-band compare).
    pub peer_fingerprint: String,
    /// Items applied locally from the peer.
    pub applied: usize,
    /// Local items the peer should apply (already in our set; peer pulls by
    /// running its own symmetric exchange).
    pub pushed: usize,
    /// Equal-rev divergent payloads needing caller resolution.
    pub conflicts: usize,
}

fn hello_frame(kx: &KeyExchange) -> Result<Vec<u8>, WireError> {
    Ok(serde_json::to_vec(&kx.hello())?)
}

/// Run the full handshake + one-shot set exchange over an established
/// connection, mutating `session` in place. Used by both the server conn
/// thread and the client.
pub fn run_exchange(
    mut stream: TcpStream,
    session: &mut SyncSession,
) -> Result<ExchangeOutcome, WireError> {
    // 1-2. Hello exchange + ECDH derive.
    let kx = KeyExchange::new(&session.device_id);
    write_frame(&mut stream, &hello_frame(&kx)?)?;
    let peer_hello: SyncHello = serde_json::from_slice(&read_frame(&mut stream)?)?;
    let peer_fp = fingerprint(&peer_hello.public_key);
    let shared: SharedSession = kx.accept(&peer_hello)?;

    // 3. Confirm tokens both ways (MAC proves key possession).
    let token = shared.confirm_token().map_err(WireError::Protocol)?;
    write_frame(&mut stream, &serde_json::to_vec(&token)?)?;
    let peer_token: SyncEnvelope = serde_json::from_slice(&read_frame(&mut stream)?)?;
    let ok = shared
        .verify_peer(&peer_token)
        .map_err(WireError::Protocol)?;
    if !ok {
        return Err(WireError::HandshakeFailed);
    }

    // 4. Sealed full-mirror swap + local reconcile.
    let my_env = seal_set(
        &shared.box_,
        &session.device_id,
        crate::sync::SyncScope::Messages,
        &session.set,
    )
    .map_err(WireError::Protocol)?;
    write_frame(&mut stream, &serde_json::to_vec(&my_env)?)?;
    let peer_env: SyncEnvelope = serde_json::from_slice(&read_frame(&mut stream)?)?;
    let remote: SyncSet = open_set(&shared.box_, &peer_env, None).map_err(WireError::Protocol)?;
    let diff = reconcile(&session.set, &remote);
    let outcome = ExchangeOutcome {
        peer_device: shared.peer_device.clone(),
        peer_fingerprint: peer_fp,
        applied: diff.apply.len(),
        pushed: diff.push.len(),
        conflicts: diff.conflicts.len(),
    };
    session.reconcile_with(&remote);
    Ok(outcome)
}

/// P51.31 — the optional pre-exchange auth frame a password-gated server
/// requires before any handshake byte. Only present on the wire when a
/// password is configured on BOTH ends (user-pass trusted-LAN-only); a
/// password-less server/client never emits it, so the ECDH wire is
/// unchanged for existing deployments.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthFrame {
    pub r#type: String, // "auth"
    pub password: String,
}

pub fn write_auth_frame(stream: &mut TcpStream, password: &str) -> Result<(), WireError> {
    write_frame(
        stream,
        &serde_json::to_vec(&AuthFrame {
            r#type: "auth".into(),
            password: password.into(),
        })?,
    )
}

/// P51.31 — read one frame and verify it is an auth frame matching
/// `password`. Any other payload (a legacy hello from a password-less
/// client) is a failed auth.
fn read_auth_frame(stream: &mut TcpStream, password: &str) -> Result<(), WireError> {
    let raw = read_frame(stream)?;
    let frame: AuthFrame = serde_json::from_slice(&raw).map_err(|_| WireError::HandshakeFailed)?;
    if frame.r#type != "auth" {
        return Err(WireError::HandshakeFailed);
    }
    // Constant-time-ish comparison (len-equal; the password is a passphrase,
    // not a MAC key — the ECDH confirm token remains the real key proof).
    let ok = frame.password.len() == password.len()
        && frame
            .password
            .as_bytes()
            .iter()
            .zip(password.bytes())
            .all(|(a, b)| *a == b);
    if !ok {
        return Err(WireError::HandshakeFailed);
    }
    Ok(())
}

/// A client-side sync against a listening peer. `password` is required when
/// the peer gate is password-gated (P51.31 user-pass trusted-LAN-only).
pub fn sync_with_peer(
    addr: SocketAddr,
    session: &mut SyncSession,
    password: Option<&str>,
) -> Result<ExchangeOutcome, WireError> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    if let Some(pw) = password {
        write_auth_frame(&mut stream, pw)?;
    }
    run_exchange(stream, session)
}

/// A live sync server: accept loop on a background thread; every accepted
/// connection runs one handshake+exchange against the shared session.
pub struct SyncServer {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    outcomes: Arc<Mutex<Vec<ExchangeOutcome>>>,
}

impl SyncServer {
    /// Bind + spawn the accept loop. `on_synced` (optional) is invoked after
    /// each successful exchange with the session lock held — the persistence
    /// hook for callers that keep state on disk.
    pub fn start(
        bind: SocketAddr,
        session: Arc<Mutex<SyncSession>>,
        on_synced: Option<SyncObserver>,
        // P51.31 — optional user-pass gate. When set, every connection must
        // present a matching `AuthFrame` before the ECDH handshake runs;
        // anything else (legacy hello, wrong password) is dropped without
        // touching the session.
        password: Option<String>,
    ) -> Result<Self, WireError> {
        let listener = TcpListener::bind(bind)?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let outcomes: Arc<Mutex<Vec<ExchangeOutcome>>> = Arc::new(Mutex::new(Vec::new()));

        let stop_c = Arc::clone(&stop);
        let outcomes_c = Arc::clone(&outcomes);
        let thread = std::thread::spawn(move || {
            loop {
                if stop_c.load(Ordering::Relaxed) {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _peer)) => {
                        let session = Arc::clone(&session);
                        let outcomes_conn = Arc::clone(&outcomes_c);
                        let on_synced_conn = on_synced.clone();
                        let gate = password.clone();
                        std::thread::spawn(move || {
                            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                            let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
                            // P51.31 — the auth gate runs BEFORE the handshake;
                            // a failed gate never locks or mutates the session.
                            if let Some(pw) = gate {
                                if read_auth_frame(&mut stream, &pw).is_err() {
                                    return; // drop silently — no session contact
                                }
                            }
                            let mut guard = session.lock().unwrap_or_else(|e| e.into_inner());
                            match run_exchange(stream, &mut guard) {
                                Ok(outcome) => {
                                    outcomes_conn
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .push(outcome);
                                    if let Some(cb) = on_synced_conn {
                                        cb(&guard);
                                    }
                                }
                                Err(_) => { /* failed/handshake refused — drop conn */ }
                            }
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            addr,
            stop,
            thread: Some(thread),
            outcomes,
        })
    }

    /// Outcomes of exchanges completed while serving.
    pub fn outcomes(&self) -> Vec<ExchangeOutcome> {
        self.outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Signal the accept loop to stop and join it.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for SyncServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncScope;

    fn seed(session: &mut SyncSession, key: &str, rev: u64) {
        session.upsert(SyncScope::Memory, key, rev, key.as_bytes().to_vec());
    }

    #[test]
    fn frame_round_trip_over_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let got = read_frame(&mut s).unwrap();
            write_frame(&mut s, &got).unwrap();
        });
        let mut c = TcpStream::connect(addr).unwrap();
        let payload = br#"{"hello":"world","n":42}"#.to_vec();
        write_frame(&mut c, &payload).unwrap();
        assert_eq!(read_frame(&mut c).unwrap(), payload);
        t.join().unwrap();
    }

    #[test]
    fn oversized_frame_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        let t = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let err = read_frame(&mut s).unwrap_err();
            let _ = tx.send(matches!(err, WireError::FrameTooLarge(_)));
        });
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(&(MAX_FRAME_BYTES + 1).to_be_bytes()).unwrap();
        c.flush().unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap());
        let _ = c.shutdown(std::net::Shutdown::Both);
        t.join().unwrap();
    }

    #[test]
    fn e2e_server_client_exchange_converges() {
        let mut server_session = SyncSession::new("dev-server", [1u8; 32]);
        seed(&mut server_session, "alpha", 3);
        seed(&mut server_session, "shared", 2);

        let mut client_session = SyncSession::new("dev-client", [2u8; 32]);
        seed(&mut client_session, "beta", 5);
        seed(&mut client_session, "shared", 2); // same rev + payload → no conflict

        let shared = Arc::new(Mutex::new(server_session));
        let persisted = Arc::new(AtomicBool::new(false));
        let persisted_c = Arc::clone(&persisted);
        let hook: Arc<dyn Fn(&SyncSession) + Send + Sync> =
            Arc::new(move |_| persisted_c.store(true, Ordering::Relaxed));

        let server = SyncServer::start(
            "127.0.0.1:0".parse().unwrap(),
            Arc::clone(&shared),
            Some(hook),
            None,
        )
        .unwrap();

        let outcome = sync_with_peer(server.addr, &mut client_session, None).unwrap();
        assert_eq!(outcome.peer_device, "dev-server");
        assert_eq!(outcome.applied, 1, "client applies server-only 'alpha'");
        assert_eq!(outcome.pushed, 1, "client has server-missing 'beta'");
        assert_eq!(outcome.conflicts, 0);
        assert_eq!(outcome.peer_fingerprint.len(), 16);

        // Server side converged too (its conn thread reconciled against the
        // client's set): 'beta' present, counts recorded, persist hook fired.
        {
            let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
            assert!(guard.set.get(SyncScope::Memory, "beta").is_some());
            assert_eq!(guard.set.live_count(), 3);
        }
        assert!(persisted.load(Ordering::Relaxed));
        let outcomes = server.outcomes();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].peer_device, "dev-client");

        server.stop();
    }

    #[test]
    fn tombstones_propagate_over_transport() {
        let mut a = SyncSession::new("a", [3u8; 32]);
        seed(&mut a, "gone", 1);
        a.delete(SyncScope::Memory, "gone", 2);
        let mut b = SyncSession::new("b", [4u8; 32]);
        seed(&mut b, "gone", 1);

        let server = SyncServer::start(
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(Mutex::new(a)),
            None,
            None,
        )
        .unwrap();
        let _outcome = sync_with_peer(server.addr, &mut b, None).unwrap();
        let item = b.set.get(SyncScope::Memory, "gone").unwrap();
        assert!(item.tombstone && item.rev == 3);
        server.stop();
    }

    #[test]
    fn password_gate_blocks_wrong_and_missing_but_passes_match() {
        // Server gated with a password; the client must present the same
        // passphrase BEFORE the ECDH handshake (P51.31).
        let mut server_session = SyncSession::new("auth-server", [5u8; 32]);
        seed(&mut server_session, "secret-item", 1);
        let server = SyncServer::start(
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(Mutex::new(server_session)),
            None,
            Some("lan-pass".into()),
        )
        .unwrap();

        // Wrong password → handshake refused, client sees an error.
        let mut bad = SyncSession::new("bad-client", [6u8; 32]);
        assert!(sync_with_peer(server.addr, &mut bad, Some("nope")).is_err());
        // Missing password (legacy client) → refused.
        let mut bare = SyncSession::new("bare-client", [7u8; 32]);
        assert!(sync_with_peer(server.addr, &mut bare, None).is_err());
        // The gate failure never touched the session: still zero outcomes.
        assert!(server.outcomes().is_empty());

        // Correct password → full exchange runs.
        let mut good = SyncSession::new("good-client", [8u8; 32]);
        seed(&mut good, "local-item", 2);
        let outcome = sync_with_peer(server.addr, &mut good, Some("lan-pass")).unwrap();
        assert_eq!(outcome.peer_device, "auth-server");
        assert_eq!(outcome.applied, 1);
        // The connection worker records the outcome asynchronously after the
        // client receives its reply. Wait briefly for that durable projection
        // instead of racing the worker thread.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while server.outcomes().is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(server.outcomes().len(), 1);
        server.stop();
    }

    #[test]
    fn no_password_server_ignores_auth_wire_changes() {
        // A password-less server keeps the legacy wire: a client that sends
        // an auth frame anyway must be refused (its first frame is treated as
        // the hello and fails parsing) — honest, never silently accepted.
        let mut server_session = SyncSession::new("open-server", [9u8; 32]);
        seed(&mut server_session, "item", 1);
        let server = SyncServer::start(
            "127.0.0.1:0".parse().unwrap(),
            Arc::new(Mutex::new(server_session)),
            None,
            None,
        )
        .unwrap();
        let mut client = SyncSession::new("legacy-client", [10u8; 32]);
        assert!(sync_with_peer(server.addr, &mut client, Some("stray-pass")).is_err());
        assert!(server.outcomes().is_empty());
        server.stop();
    }

    #[test]
    fn fingerprint_is_stable_hex() {
        let f1 = fingerprint(&[1u8; 32]);
        let f2 = fingerprint(&[1u8; 32]);
        let f3 = fingerprint(&[2u8; 32]);
        assert_eq!(f1, f2);
        assert_ne!(f1, f3);
        assert_eq!(f1.len(), 16);
        assert!(f1.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
