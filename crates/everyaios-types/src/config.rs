//! `ARCH/10-KERNEL.md` §4 — the **pure** half of configuration: the schema
//! version, the snapshot a run is pinned to, and the key rules a validated
//! document must satisfy.
//!
//! The kernel's job here is narrow on purpose. It owns the three facts that
//! every config surface needs and that no surface may decide for itself:
//!
//! - **The schema version.** One number, stamped in every file, checked at load.
//!   A file written by a newer build is *refused*, not partially read: a
//!   half-understood config is a config whose author believes a setting is on
//!   when this build never applied it.
//! - **The snapshot.** The reproducibility record (`ARCH/10-KERNEL.md` §4: config
//!   changes that affect running work are versioned into that work's record).
//!   A [`ConfigSnapshot`] is what a `Work` embeds so a resumed or reproduced run
//!   can be told *which* configuration produced it.
//! - **The key rules.** Which key names are secret-shaped (so a config store can
//!   refuse them), and what a refusal owes the author (a migration note, never a
//!   silent drop).
//!
//! What this module deliberately does **not** own: the layer ladder
//! (`ConfigLayer` in the shell's config module), any particular entry's schema,
//! or any IO. Those belong to the surface that reads the file; the kernel states
//! the rules they must satisfy.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The config schema version this build writes and fully understands.
///
/// Bumped when a **breaking** change lands: a removed key, a retyped field, a
/// changed default that alters behaviour. An additive, backward-compatible field
/// does not bump it — that is what `migration note` reporting is for.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// Why a config document was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigRefusal {
    /// The document declares a schema version this build does not know. Read
    /// nothing: a partially-understood config is a config whose author believes a
    /// setting is in force when it never was.
    SchemaTooNew {
        /// What the file says.
        declared: u32,
        /// What this build understands.
        supported: u32,
    },
    /// A key this build does not define. Never silently accepted
    /// (`ARCH/10-KERNEL.md` §4); a *warning* with a migration note, or a refusal
    /// for an entry that owns its own validation (see the shell's config module).
    UnknownKey {
        /// The key as it was written.
        key: String,
    },
    /// A key whose name is secret-shaped. Config stores carry **vault
    /// references**, never values (`INV-02`); a credential written here is a
    /// credential in a plaintext store with a backup policy nobody chose.
    SecretShapedKey {
        /// The key as it was written.
        key: String,
    },
    /// The document is not parseable as a config table at all.
    Malformed {
        /// The parser's own words.
        detail: String,
    },
}

impl fmt::Display for ConfigRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SchemaTooNew {
                declared,
                supported,
            } => write!(
                f,
                "this config declares schema v{declared}; this build understands up to \
                 v{supported}. It was written by a newer build, so reading part of it would \
                 apply settings this build does not know. Upgrade the build, or restore a \
                 config this build wrote."
            ),
            Self::UnknownKey { key } => write!(
                f,
                "`{key}` is not a key this build defines. It is not being applied, and it is \
                 not being ignored silently either: remove it, or check the spelling against \
                 the documented configuration schema."
            ),
            Self::SecretShapedKey { key } => write!(
                f,
                "`{key}` looks like a credential. Config carries vault *references*, never \
                 values (INV-02). Store the secret in the vault and put its reference here."
            ),
            Self::Malformed { detail } => {
                write!(f, "this config is not readable: {detail}")
            }
        }
    }
}

impl std::error::Error for ConfigRefusal {}

impl From<ConfigRefusal> for crate::error::BoundaryError {
    /// A refused config is `InvalidState`: what is on disk is not a document this
    /// build can act on, and re-reading it unchanged cannot help. It carries the
    /// refusal text as its message, so a caller sees *why* without opening the
    /// file.
    fn from(refusal: ConfigRefusal) -> Self {
        Self::new(crate::error::ErrorCode::InvalidState, refusal.to_string())
    }
}

/// Key names that are refused anywhere in a config document.
///
/// A suffix test on the separator-stripped name, for the same reason the error
/// scrubber uses one: real files write `apiKey`, `api_key` and `API-KEY`, and a
/// check that only knows one spelling is a check that misses.
const SECRET_KEY_NAMES: [&str; 14] = [
    "apikey",
    "token",
    "accesstoken",
    "refreshtoken",
    "secret",
    "clientsecret",
    "password",
    "passwd",
    "pwd",
    "credential",
    "credentials",
    "privatekey",
    "authorization",
    "bearer",
];

/// Whether a key name is secret-shaped, and therefore refused in a config store.
///
/// Refused rather than warned: unlike an unknown key (which may be a forward-
/// compatible addition), a credential written into config is a fact that cannot
/// be walked back by upgrading, and the vault is the only place a credential
/// belongs (`INV-02`).
pub fn is_secret_shaped_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect();
    if normalized.is_empty() {
        return false;
    }
    SECRET_KEY_NAMES.iter().any(|name| {
        normalized.ends_with(name) || name.ends_with(&normalized) && normalized.len() >= 3
    })
}

/// A configuration snapshot: the identity of one resolved configuration, for a
/// `Work` record to pin.
///
/// Two fields, both necessary:
///
/// - `schema_version` — which *shape* of schema produced these values. A value
///   is meaningless without it: `retention_days = 7` under v1 and under a
///   future v2 need not mean the same thing.
/// - `hash` — *which* values. Two runs with the same schema and different hashes
///   had different config, and a reproduction that silently used the newer one
///   would be a reproduction of something else.
///
/// The hash is a **fingerprint**, not an authorization token: it detects drift,
/// it does not resist a chosen-input collision. Anything that must resist that
/// uses the kernel's crypto crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    /// The schema version the values were resolved under.
    pub schema_version: u32,
    /// A stable fingerprint of the resolved values.
    pub hash: String,
}

impl ConfigSnapshot {
    /// A snapshot from a resolved configuration's fingerprint.
    pub fn new(hash: impl Into<String>) -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            hash: hash.into(),
        }
    }

    /// Whether this snapshot matches the schema this build understands. A
    /// mismatch is not a soft warning: a pinned run cannot be reproduced against
    /// a schema it was not resolved under.
    pub fn is_reproducible_here(&self) -> bool {
        self.schema_version == CONFIG_SCHEMA_VERSION
    }

    /// Whether two snapshots name the same configuration.
    pub fn is_same_config(&self, other: &ConfigSnapshot) -> bool {
        self.schema_version == other.schema_version && self.hash == other.hash
    }
}

impl fmt::Display for ConfigSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "config@schema-v{}#{:12}", self.schema_version, self.hash)
    }
}

/// Check the document-level rules on a parsed config table: the declared schema
/// version, and the top-level keys.
///
/// Returns **every** refusal it finds, not just the first, so a user fixing a
/// file sees the whole list rather than one problem per edit.
pub fn check_document(table: &(impl toml_compat::Table + ?Sized)) -> Vec<ConfigRefusal> {
    let mut refusals = Vec::new();
    if let Some(declared) = table.integer("schema_version")
        && u32::try_from(declared).map_or(true, |version| version > CONFIG_SCHEMA_VERSION)
    {
        refusals.push(ConfigRefusal::SchemaTooNew {
            declared: u32::try_from(declared).unwrap_or(u32::MAX),
            supported: CONFIG_SCHEMA_VERSION,
        });
    }
    for key in table.keys() {
        if is_secret_shaped_key(&key) {
            refusals.push(ConfigRefusal::SecretShapedKey { key });
        }
    }
    refusals
}

/// The slice of a TOML table the kernel's document check needs.
///
/// Declared here rather than pulled in as a dependency: the kernel is
/// IO-free and dependency-light, and a structural read of a table (keys, one
/// integer) is a two-method contract that `toml::Table`, `toml::Value` and any
/// future reader satisfy alike. A config surface calls [`check_document`] with
/// its own `toml::Table`.
pub mod toml_compat {
    /// The read side of a config table.
    pub trait Table {
        /// Every top-level key, in document order.
        fn keys(&self) -> Vec<String>;
        /// One key's value as an integer, if it is an integer.
        fn integer(&self, key: &str) -> Option<i64>;
    }

    impl Table for toml::Table {
        fn keys(&self) -> Vec<String> {
            self.keys().cloned().collect()
        }

        fn integer(&self, key: &str) -> Option<i64> {
            self.get(key)?.as_integer()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct FakeTable(BTreeMap<String, i64>);

    impl toml_compat::Table for FakeTable {
        fn keys(&self) -> Vec<String> {
            self.0.keys().cloned().collect()
        }
        fn integer(&self, key: &str) -> Option<i64> {
            self.0.get(key).copied()
        }
    }

    /// The real reader, so the trait is proven against the type every config
    /// surface actually parses into — a trait that only a fake satisfies is a
    /// trait nothing uses.
    #[test]
    fn the_document_check_reads_a_real_toml_table() {
        let parsed: toml::Table = toml::from_str(
            r#"
schema_version = 1
retention_days = 7
"#,
        )
        .expect("a config table parses");
        assert!(check_document(&parsed).is_empty());

        let ahead: toml::Table = toml::from_str("schema_version = 99").expect("parses");
        assert_eq!(
            check_document(&ahead),
            vec![ConfigRefusal::SchemaTooNew {
                declared: 99,
                supported: CONFIG_SCHEMA_VERSION
            }]
        );

        let credential: toml::Table = toml::from_str(r#"api_key = "sk-live-x""#).expect("parses");
        assert_eq!(
            check_document(&credential),
            vec![ConfigRefusal::SecretShapedKey {
                key: "api_key".into()
            }]
        );
    }

    fn table(pairs: &[(&str, i64)]) -> FakeTable {
        FakeTable(pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect())
    }

    #[test]
    fn the_schema_version_is_checked_at_load_not_assumed() {
        // Current or older: no refusal.
        assert!(check_document(&table(&[("schema_version", 1)])).is_empty());
        assert!(check_document(&table(&[("schema_version", 0)])).is_empty());
        assert!(check_document(&table(&[])).is_empty());

        // Newer: refused, with both numbers named.
        let refusals = check_document(&table(&[("schema_version", 2)]));
        assert_eq!(
            refusals,
            vec![ConfigRefusal::SchemaTooNew {
                declared: 2,
                supported: CONFIG_SCHEMA_VERSION
            }]
        );
        let shown = refusals[0].to_string();
        assert!(shown.contains("v2"), "{shown}");
        assert!(shown.contains("newer build"), "{shown}");
        // And it is a typed, non-retryable refusal at a boundary.
        let wire = crate::error::BoundaryError::from(refusals[0].clone());
        assert_eq!(wire.code, crate::error::ErrorCode::InvalidState);
        assert!(!wire.retryable);
    }

    #[test]
    fn a_secret_shaped_key_is_refused_and_never_warned() {
        for key in [
            "api_key",
            "apiKey",
            "API-KEY",
            "openai_api_key",
            "token",
            "accessToken",
            "secret",
            "client_secret",
            "password",
            "private_key",
            "bearer",
        ] {
            assert!(is_secret_shaped_key(key), "{key} must be refused");
        }
        // The registered config keys are not secret-shaped, so a real document
        // is not refused by this rule.
        for key in [
            "data_dir",
            "vault_path",
            "retention_days",
            "controlPlaneRateLimit",
            "model_aliases",
            "primary_chief",
            "terminal",
            "local",
            "schema_version",
        ] {
            assert!(!is_secret_shaped_key(key), "{key} must be allowed");
        }
    }

    #[test]
    fn a_credential_in_a_config_document_is_refused_by_document_check() {
        let refusals = check_document(&table(&[("schema_version", 1), ("openai_api_key", 0)]));
        assert_eq!(refusals.len(), 1);
        assert_eq!(
            refusals[0],
            ConfigRefusal::SecretShapedKey {
                key: "openai_api_key".into()
            }
        );
        let shown = refusals[0].to_string();
        assert!(shown.contains("vault"), "{shown}");
        assert!(shown.contains("INV-02"), "{shown}");
    }

    #[test]
    fn every_refusal_is_reported_not_just_the_first() {
        let refusals = check_document(&table(&[
            ("schema_version", 9),
            ("api_key", 0),
            ("password", 0),
        ]));
        assert_eq!(refusals.len(), 3, "{refusals:?}");
    }

    #[test]
    fn a_snapshot_pins_both_the_shape_and_the_values() {
        let pinned = ConfigSnapshot::new("fnv1a64:0123456789abcdef");
        assert_eq!(pinned.schema_version, CONFIG_SCHEMA_VERSION);
        assert!(pinned.is_reproducible_here());
        // Same values, same snapshot: a reproduction can be verified.
        assert!(pinned.is_same_config(&ConfigSnapshot::new("fnv1a64:0123456789abcdef")));
        // Different values: the drift is visible.
        assert!(!pinned.is_same_config(&ConfigSnapshot::new("fnv1a64:fedcba9876543210")));
        // Same values, different schema: *not* the same configuration, because a
        // value is meaningless without the shape it was resolved under.
        let older = ConfigSnapshot {
            schema_version: CONFIG_SCHEMA_VERSION - 1,
            hash: pinned.hash.clone(),
        };
        assert!(!pinned.is_same_config(&older));
        assert!(!older.is_reproducible_here());
        assert!(pinned.to_string().contains("schema-v1"));
    }

    #[test]
    fn a_snapshot_round_trips_through_a_work_record() {
        let pinned = ConfigSnapshot::new("abc123");
        let json = serde_json::to_string(&pinned).unwrap();
        assert!(json.contains("\"schema_version\":1"), "{json}");
        let back: ConfigSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, pinned);
    }
}
