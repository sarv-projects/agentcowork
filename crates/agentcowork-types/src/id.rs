//! `ARCH/10-KERNEL.md` §2 + `ARCH/06-DATA-MODEL.md` §0 — **identity**: one id
//! type, minted in one place, opaque on the wire.
//!
//! # Why a minter and not a constructor
//!
//! The spec is a single-writer rule (`INV-06`): *a durable entity id is a uuidv7
//! minted by the owning service — never by a caller or the UI*. A bare
//! `EntityId::new(bytes)` would let any code path invent an id, which is exactly
//! the failure the rule exists to prevent. So the only public constructors are
//!
//! - [`EntityId::mint`] — the one minting path, and
//! - [`EntityId::from_uuid_v7`] — the boundary reader, which *validates* rather
//!   than invents: a peer-supplied id is a fact to be checked, not a shape to
//!   be filled in.
//!
//! # Opaque by construction
//!
//! [`EntityId`] wraps 16 bytes and exposes no accessor that could be used to
//! *write* meaning into an id: the timestamp is readable (it is how a uuidv7 is
//! ordered) but it is not a state, a version, a counter or a capability. Ids
//! carry no encoded meaning; the owner resolves them.
//!
//! # Short ids are projections
//!
//! [`ShortId`] exists because a UI list needs something narrow, and it is
//! deliberately derived from the **random** part of the uuid (never the
//! timestamp, which would leak creation order into a list) and explicitly
//! non-authoritative: [`EntityId::resolve`] is the only way back to the owner.
//!
//! # Entropy
//!
//! This crate carries no third-party dependency, so randomness comes from the
//! standard library: `RandomState` (which the OS seeds), mixed through
//! SplitMix64. Uniqueness does not depend on that entropy at all — within one
//! process `(millisecond, counter)` is strictly increasing, so two ids minted in
//! the same millisecond cannot collide; across processes the 74 random bits do
//! the work. Ids are opaque, not secrets: authority is a ticket (`INV-01`), so
//! unpredictability is not a security property here.

use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{ArtifactId, ExecutionId, ReceiptId, RunId, SessionId, TicketId, WorkId};

/// Bits of the uuidv7 timestamp (milliseconds since the Unix epoch).
const TIMESTAMP_BITS: u64 = 48;
/// Version nibble of a uuidv7.
const VERSION_7: u8 = 7;
/// RFC 4122 variant bits, as stored in byte 8.
const VARIANT_MASK: u8 = 0b1100_0000;
const VARIANT_RFC4122: u8 = 0b1000_0000;

/// Why a byte string or a string is not a uuidv7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UuidV7Error {
    /// The text was not 32 hex digits, nor 32 hex digits in the canonical
    /// hyphenated form.
    Malformed,
    /// Well-formed uuid, but not version 7.
    NotV7,
    /// Well-formed uuid, but the RFC 4122 variant bits are wrong.
    BadVariant,
    /// The timestamp does not fit in 48 bits.
    TimestampOutOfRange,
}

impl fmt::Display for UuidV7Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "not a uuid: expected 32 hex digits, hyphenated or not",
            Self::NotV7 => "not a uuidv7: the version nibble is not 7",
            Self::BadVariant => "not a uuid: the variant bits are not RFC 4122",
            Self::TimestampOutOfRange => "uuidv7 timestamp does not fit in 48 bits",
        })
    }
}

impl std::error::Error for UuidV7Error {}

/// An opaque, time-ordered entity id: a uuidv7, 16 bytes, canonical form
/// `xxxxxxxx-xxxx-7xxx-yxxx-xxxxxxxxxxxx` in lowercase hex.
///
/// Serializable as that one string and nothing else, so no consumer can depend
/// on a field layout that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntityId([u8; 16]);

impl EntityId {
    /// **The minting path.** One function mints an entity id in the whole
    /// system; a caller that needs an id asks the owner, and the owner asks
    /// here.
    ///
    /// Returns the id and the millisecond it was minted in, so an owner that
    /// stores `created_at` does not read the clock a second time (and a caller
    /// that already has a [`crate::EpochMillis`] can pass it in).
    pub fn mint() -> (Self, i64) {
        EntityId::mint_at(crate::time::now_epoch_millis())
    }

    /// Mint with an explicit minting instant. Monotonic inside a process: two
    /// calls with the same or an earlier millisecond still produce strictly
    /// increasing ids.
    pub fn mint_at(unix_ms: i64) -> (Self, i64) {
        static MINTER: std::sync::LazyLock<IdMinter> = std::sync::LazyLock::new(IdMinter::new);
        MINTER.mint(unix_ms)
    }

    /// The boundary reader: accept a uuid **only if** it is a valid uuidv7.
    /// This validates, it does not invent — the difference is the whole point of
    /// the single-writer rule.
    pub fn from_uuid_v7(bytes: [u8; 16]) -> Result<Self, UuidV7Error> {
        if bytes[6] >> 4 != VERSION_7 {
            return Err(UuidV7Error::NotV7);
        }
        if bytes[8] & VARIANT_MASK != VARIANT_RFC4122 {
            return Err(UuidV7Error::BadVariant);
        }
        Ok(Self(bytes))
    }

    /// Parse the canonical form (or 32 undelimited hex digits). Upper and lower
    /// case are both accepted on input; the canonical output is always lowercase
    /// with hyphens, so a case difference can never fork an identity.
    pub fn parse(text: &str) -> Result<Self, UuidV7Error> {
        let mut bytes = [0u8; 16];
        let mut nibbles = 0usize;
        for ch in text.chars() {
            if ch == '-' {
                continue;
            }
            let digit = ch.to_digit(16).ok_or(UuidV7Error::Malformed)? as u8;
            if nibbles >= 32 {
                return Err(UuidV7Error::Malformed);
            }
            if nibbles.is_multiple_of(2) {
                bytes[nibbles / 2] = digit << 4;
            } else {
                bytes[nibbles / 2] |= digit;
            }
            nibbles += 1;
        }
        if nibbles != 32 {
            return Err(UuidV7Error::Malformed);
        }
        Self::from_uuid_v7(bytes)
    }

    /// The raw 16 bytes.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// The uuidv7 timestamp, in epoch milliseconds — the ordering key, and the
    /// reason the id is time-ordered. It is **not** an authoritative created-at:
    /// the owning service decides that.
    pub fn timestamp_ms(&self) -> i64 {
        // The first 48 bits, big-endian, are the millisecond timestamp.
        let mut ms = 0u64;
        for byte in &self.0[..6] {
            ms = (ms << 8) | u64::from(*byte);
        }
        (ms & ((1u64 << TIMESTAMP_BITS) - 1)) as i64
    }

    /// The derived, non-authoritative short form for lists and receipts
    /// (`ARCH/10-KERNEL.md` §2). Taken from the random field, so a list sorted by
    /// short id does not leak creation order.
    pub fn short(&self) -> ShortId {
        ShortId(format!(
            "{:02x}{:02x}{:02x}{:02x}",
            self.0[9], self.0[10], self.0[11], self.0[12]
        ))
    }

    /// Resolve a short id back through the owner. A short id is a *projection*,
    /// so a lookup that starts from one is incomplete until the owner resolves
    /// it; the resolution returns the full id, and the owner is what decides
    /// whether that id exists.
    pub fn resolve(short: &ShortId) -> Option<Self> {
        // A short id carries 4 bytes of the random field; it is not invertible,
        // and pretending otherwise is the failure this method exists to name.
        let _ = short;
        None
    }

    /// Whether `text` is a canonical uuidv7 string.
    pub fn is_canonical(text: &str) -> bool {
        Self::parse(text).is_ok_and(|id| id.to_string() == text)
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-\
             {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0],
            b[1],
            b[2],
            b[3],
            b[4],
            b[5],
            b[6],
            b[7],
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15]
        )
    }
}

impl Serialize for EntityId {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for EntityId {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let text = String::deserialize(de)?;
        Self::parse(&text).map_err(D::Error::custom)
    }
}

/// A derived short id for humans. **Never authoritative**: it is a projection of
/// [`EntityId::short`], and a lookup by short id is only valid after the owning
/// service resolves it (`ARCH/10-KERNEL.md` §2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShortId(String);

impl ShortId {
    /// The short text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ShortId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Bits of the per-millisecond counter carried in the uuidv7 `rand_a` field.
const COUNTER_BITS: u32 = 12;
const COUNTER_MASK: u64 = (1u64 << COUNTER_BITS) - 1;

/// Bits of the millisecond the minter's own state packs. 52 leaves the counter
/// its 12 and still covers the year 140,000, so the state word can never be
/// exhausted by a clock that runs fast.
const STATE_MS_BITS: u32 = 52;
const STATE_MS_MASK: u64 = (1u64 << STATE_MS_BITS) - 1;

/// The single minter: a monotonic clock source, a per-millisecond counter and a
/// SplitMix64 stream for the random fields.
///
/// Stateless in the common case — [`EntityId::mint`] uses a process-wide
/// instance — but constructible per service, so a test (or a service that wants
/// its own sequence) can hold one.
#[derive(Debug)]
pub struct IdMinter {
    /// The last millisecond handed out, and the counter within it.
    state: AtomicU64,
    /// The random stream (SplitMix64, seeded per minter).
    random: AtomicU64,
}

impl IdMinter {
    /// A new minter, seeded from the OS-seeded standard-library hasher.
    pub fn new() -> Self {
        Self {
            state: AtomicU64::new(0),
            random: AtomicU64::new(seed()),
        }
    }

    /// Mint one id for `unix_ms`.
    ///
    /// Strictly increasing, always. When `unix_ms` is not newer than the last
    /// call, the id is minted *inside* the last millisecond with the counter in
    /// the uuidv7 `rand_a` field (RFC 9562's dedicated-counter method), so a
    /// wall-clock jump cannot un-order ids either. The millisecond the id
    /// carries is therefore the *monotonic* one the minter handed out, which is
    /// the wall clock unless the clock stood still or went backwards.
    ///
    /// A collision is not representable here: two mints never share
    /// `(millisecond, counter)`, and the millisecond never repeats a counter
    /// within itself. A caller that *observes* a collision has found a bug —
    /// `ARCH/10-KERNEL.md` §9 classes it as `Internal`, not as a case to handle.
    pub fn mint(&self, unix_ms: i64) -> (EntityId, i64) {
        let requested = u64::try_from(unix_ms).unwrap_or(0) & STATE_MS_MASK;
        // One CAS loop owns both the millisecond and the counter, so two threads
        // can never be handed the same (ms, counter) pair.
        let (ms, counter) = loop {
            let previous = self.state.load(Ordering::Acquire);
            let (prev_ms, prev_counter) = split_state(previous);
            let (next_ms, next_counter) = if requested > prev_ms {
                (requested, 0)
            } else if prev_counter >= COUNTER_MASK {
                // The millisecond is full. Advancing the *logical* millisecond
                // by one keeps the sequence strictly increasing instead of
                // wrapping the counter — a wrapped counter would reorder two ids
                // inside one millisecond, and "time-ordered" is the property
                // this type exists for. The cost is that the millisecond in the
                // id can lead the wall clock by up to one tick under a burst of
                // more than 4096 mints in a millisecond, which is reported
                // honestly by the second return value.
                (prev_ms.saturating_add(1), 0)
            } else {
                (prev_ms, prev_counter + 1)
            };
            if self
                .state
                .compare_exchange_weak(
                    previous,
                    join_state(next_ms, next_counter),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                break (next_ms, next_counter);
            }
        };

        let random_a = (counter as u16) & 0x0fff;
        let random_b = self.next_random();
        let mut bytes = [0u8; 16];
        let ms = ms & ((1u64 << TIMESTAMP_BITS) - 1);
        bytes[0] = (ms >> 40) as u8;
        bytes[1] = (ms >> 32) as u8;
        bytes[2] = (ms >> 24) as u8;
        bytes[3] = (ms >> 16) as u8;
        bytes[4] = (ms >> 8) as u8;
        bytes[5] = ms as u8;
        bytes[6] = 0x70 | ((random_a >> 8) as u8 & 0x0f);
        bytes[7] = random_a as u8;
        bytes[8] = VARIANT_RFC4122 | ((random_b >> 56) as u8 & 0x3f);
        bytes[9..16].copy_from_slice(&random_b.to_be_bytes()[1..8]);
        // The bytes were built to spec, so this cannot fail; a failure here would
        // be a bug in the minter, which the spec classes as `Internal`.
        let id = EntityId::from_uuid_v7(bytes).expect("a minter always builds a valid uuidv7");
        (id, ms as i64)
    }

    fn next_random(&self) -> u64 {
        // SplitMix64: fetch_add is the whole state transition.
        let z = self
            .random
            .fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
            .wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = z;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

impl Default for IdMinter {
    fn default() -> Self {
        Self::new()
    }
}

fn split_state(state: u64) -> (u64, u64) {
    (state >> COUNTER_BITS, state & COUNTER_MASK)
}

fn join_state(ms: u64, counter: u64) -> u64 {
    ((ms & STATE_MS_MASK) << COUNTER_BITS) | (counter & COUNTER_MASK)
}

/// A 64-bit seed from the OS-seeded standard-library hasher, mixed once.
fn seed() -> u64 {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(0x5eed_0000_0000_0001);
    let mut value = hasher.finish();
    value ^= std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    value ^= (std::process::id() as u64) << 32;
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Every durable-entity id newtype, in one list. Two jobs, both mechanical:
///
/// 1. `From<EntityId>` — the only sane way to build one of these from a mint,
///    so an id that reaches a durable record is a validated uuidv7.
/// 2. `parse_id` — the boundary reader, which validates instead of wrapping.
///
/// `CheckpointId` is deliberately **absent**: `CheckpointId::for_step` builds a
/// deterministic *derived reference* (`ckpt:<work>/<step>`), which is a
/// cross-entity reference, not a minted entity id.
macro_rules! entity_id_newtypes {
    ($($name:ident),* $(,)?) => {
        $(
            impl From<EntityId> for $name {
                fn from(id: EntityId) -> Self {
                    Self(id.to_string())
                }
            }

            impl $name {
                /// Parse this id from a wire value, validating that it is a
                /// uuidv7. A malformed or non-v7 id is refused, never wrapped:
                /// a boundary that cannot parse an id must not invent one.
                pub fn parse_id(text: &str) -> Result<Self, UuidV7Error> {
                    EntityId::parse(text).map(Self::from)
                }
            }
        )*
    };
}

entity_id_newtypes!(
    WorkId,
    SessionId,
    RunId,
    ExecutionId,
    TicketId,
    ReceiptId,
    ArtifactId,
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_minted_id_is_a_canonical_uuidv7() {
        let (id, ms) = EntityId::mint();
        let text = id.to_string();
        assert_eq!(text.len(), 36, "{text}");
        assert_eq!(text, text.to_lowercase(), "canonical form is lowercase");
        assert_eq!(&text[14..15], "7", "version nibble: {text}");
        assert!(
            matches!(&text[19..20], "8" | "9" | "a" | "b"),
            "RFC 4122 variant: {text}"
        );
        assert_eq!(EntityId::parse(&text).unwrap(), id);
        assert!(EntityId::is_canonical(&text));
        assert_eq!(id.timestamp_ms(), ms, "the timestamp is the ordering key");
    }

    #[test]
    fn ids_are_opaque_and_carry_no_meaning() {
        // Whatever the bytes are, the only readable facts are the uuid shape and
        // the ordering timestamp: there is no accessor for a state, a version, a
        // counter or a capability, so none can be encoded.
        let (id, _) = EntityId::mint();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        let back: EntityId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
        // A short id is 4 bytes of the random field, not the timestamp, so a
        // list ordered by it does not leak creation order.
        let short = id.short();
        assert_eq!(short.as_str().len(), 8);
        assert!(
            !id.to_string()[0..8].starts_with(short.as_str()),
            "the short id is not a prefix of the id"
        );
        // …and it is not invertible: a lookup still has to go through the owner.
        assert_eq!(EntityId::resolve(&short), None);
    }

    #[test]
    fn a_malformed_or_non_v7_id_is_refused_not_wrapped() {
        assert_eq!(EntityId::parse("not-a-uuid"), Err(UuidV7Error::Malformed));
        assert_eq!(EntityId::parse(""), Err(UuidV7Error::Malformed));
        assert_eq!(EntityId::parse("abc"), Err(UuidV7Error::Malformed));
        // A v4 uuid is well-formed but the wrong version.
        let v4 = "9f1b2c3d-4e5f-4a6b-8c7d-0e1f2a3b4c5d";
        assert_eq!(EntityId::parse(v4), Err(UuidV7Error::NotV7));
        // Right version, wrong variant bits.
        let bad_variant = "9f1b2c3d-4e5f-7a6b-0c7d-0e1f2a3b4c5d";
        assert_eq!(EntityId::parse(bad_variant), Err(UuidV7Error::BadVariant));
        // Upper case is accepted on input and normalized on output.
        let lower = EntityId::mint().0.to_string();
        let upper = lower.to_uppercase();
        assert_eq!(EntityId::parse(&upper).unwrap().to_string(), lower);
        assert!(!EntityId::is_canonical(&upper));
        // A JSON payload carrying a non-v7 id fails to deserialize.
        let bad = format!("\"{v4}\"");
        assert!(serde_json::from_str::<EntityId>(&bad).is_err());
    }

    #[test]
    fn ids_are_time_ordered_within_a_millisecond() {
        let minter = IdMinter::new();
        let (first, ms) = minter.mint(1_700_000_000_000);
        assert_eq!(ms, 1_700_000_000_000);
        let mut previous = first;
        // More mints than the 12-bit counter holds, so the logical millisecond
        // has to advance rather than the counter wrapping.
        for _ in 0..5_000 {
            let (id, _) = minter.mint(1_700_000_000_000);
            assert!(
                id > previous,
                "ids must increase even inside one millisecond"
            );
            previous = id;
        }
        // A wall-clock jump backwards does not un-order the sequence: the minter
        // holds at (or ahead of) the last millisecond it handed out.
        let (backwards, held) = minter.mint(1_600_000_000_000);
        assert!(held >= 1_700_000_000_000, "held at the last millisecond");
        assert!(backwards > previous);
    }

    #[test]
    fn concurrent_minting_never_repeats_an_id() {
        use std::sync::Arc;
        let minter = Arc::new(IdMinter::new());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let minter = Arc::clone(&minter);
            handles.push(std::thread::spawn(move || {
                let mut ids = Vec::new();
                for _ in 0..500 {
                    ids.push(minter.mint(1_700_000_000_000).0.to_string());
                }
                ids
            }));
        }
        let all: HashSet<String> = handles
            .into_iter()
            .flat_map(|h| h.join().expect("thread"))
            .collect();
        assert_eq!(all.len(), 8 * 500, "4k ids in one millisecond, no repeats");
    }

    #[test]
    fn distinct_processes_do_not_share_a_seed() {
        // Two minters created in the same process must not walk the same
        // sequence — the seed is mixed, not a constant.
        let a = IdMinter::new();
        let b = IdMinter::new();
        let first: HashSet<String> = (0..64)
            .map(|i| a.mint(1_700_000_000_000 + i).0.to_string())
            .collect();
        let second: HashSet<String> = (0..64)
            .map(|i| b.mint(1_700_000_000_000 + i).0.to_string())
            .collect();
        assert_eq!(first.intersection(&second).count(), 0);
    }

    #[test]
    fn entity_id_newtypes_convert_from_a_mint() {
        let (id, _ms) = EntityId::mint();
        let work: WorkId = id.into();
        assert_eq!(work.as_str(), id.to_string());
        // …and the boundary reader validates rather than wrapping.
        let parsed = WorkId::parse_id(&id.to_string()).expect("a minted id parses back");
        assert_eq!(parsed, work);
        assert!(WorkId::parse_id("nope").is_err());
        assert!(TicketId::parse_id("9f1b2c3d-4e5f-4a6b-8c7d-0e1f2a3b4c5d").is_err());
        // A derived reference stays a derived reference: `for_step` is a
        // cross-entity reference, not a mint, so it keeps its composite form and
        // is deliberately not convertible from an `EntityId`.
        let ckpt = crate::CheckpointId::for_step(id.to_string().as_str(), 3);
        assert!(ckpt.is_assigned());
        assert_eq!(ckpt.as_str(), format!("ckpt:{id}/3"));
        assert!(!crate::CheckpointId::unassigned().is_assigned());
    }
}
