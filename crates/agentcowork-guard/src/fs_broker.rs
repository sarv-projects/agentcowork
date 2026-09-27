//! P7.8 — FS syscall broker (doc 64 S4 — chromium `syscall_broker/`:
//! BrokerHost/Client + command enum + simple-message framing). A sandboxed
//! worker (e.g. the rquickjs script-eval worker) never touches the
//! filesystem syscalls itself; it sends a [`BrokerRequest`] to the host, the
//! host runs the canonicalized allowlist check (path floor + the sandbox
//! profile's path rules), and returns either a validated handle or a
//! denial. The worker cannot open anything the broker did not validate.
//!
//! The message seam is a plain [`BrokerTransport`] so the same logic runs
//! in-process (tests) or over `agentcowork-ipc` (the runtime wiring).

use crate::pathfloor::{FloorVerdict, canonicalize_no_follow, enforce_floor};
use crate::sandbox::{PathAccess, SandboxProfile};
use serde::{Deserialize, Serialize};

/// Filesystem operations the worker may request (the command enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrokerOp {
    OpenRead,
    OpenWrite,
    Create,
    Stat,
    Access,
    Readlink,
}

/// A broker request (simple-message framing: one op + one path).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrokerRequest {
    pub op: BrokerOp,
    pub path: String,
}

/// The broker's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrokerResponse {
    /// Validated — the worker may proceed (host returns a real handle over
    /// the transport; in-process the decision is the handle).
    Allowed,
    /// Refused with a reason (never a syscall error — a policy denial).
    Denied { reason: String },
}

/// The transport seam between worker and host.
pub trait BrokerTransport {
    fn request(&mut self, req: &BrokerRequest) -> Result<BrokerResponse, String>;
}

/// The broker host: holds the sandbox profile (path rules) and decides
/// every request against the canonicalized allowlist.
#[derive(Debug, Clone)]
pub struct BrokerHost {
    profile: SandboxProfile,
}

impl BrokerHost {
    pub fn new(profile: SandboxProfile) -> Self {
        Self { profile }
    }

    /// The core decision — also used directly by in-process tests and by
    /// [`BrokerTransport`] implementations over IPC.
    pub fn handle(&self, req: &BrokerRequest) -> BrokerResponse {
        // Path floor first: `..` / symlink escapes are always refused.
        let roots: Vec<&str> = self
            .profile
            .paths
            .iter()
            .map(|r| r.prefix.as_str())
            .collect();
        match enforce_floor(&req.path, &roots) {
            FloorVerdict::Allowed => {}
            v => {
                return BrokerResponse::Denied {
                    reason: format!("path floor refused: {v:?}"),
                };
            }
        }
        // Then the profile's per-path access rule for the operation.
        let access = match req.op {
            BrokerOp::OpenRead | BrokerOp::Stat | BrokerOp::Access | BrokerOp::Readlink => {
                PathAccess::ReadOnly
            }
            BrokerOp::OpenWrite | BrokerOp::Create => PathAccess::ReadWrite,
        };
        if !self.profile.check_path(&req.path, access) {
            return BrokerResponse::Denied {
                reason: format!(
                    "path `{}` not allowed for {:?} by profile `{}`",
                    req.path, req.op, self.profile.name
                ),
            };
        }
        // Create requires an existing parent that is writable (chromium's
        // "add-if-exists" semantics) — the canonicalized parent must be
        // inside a write-capable prefix.
        if req.op == BrokerOp::Create {
            let parent = canonicalize_no_follow(
                std::path::Path::new(&req.path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default()
                    .as_str(),
            );
            if !self.profile.check_path(&parent, PathAccess::AddIfExists) {
                return BrokerResponse::Denied {
                    reason: format!("create parent `{parent}` not writable"),
                };
            }
        }
        BrokerResponse::Allowed
    }
}

/// An in-process broker transport (direct call — the same decisions an IPC
/// transport would make).
#[derive(Debug, Clone, Default)]
pub struct InProcessBroker {
    pub host: Option<BrokerHost>,
}

impl BrokerTransport for InProcessBroker {
    fn request(&mut self, req: &BrokerRequest) -> Result<BrokerResponse, String> {
        let host = self.host.as_ref().ok_or("no broker host")?;
        Ok(host.handle(req))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::profiles;

    fn host() -> BrokerHost {
        BrokerHost::new(profiles::worker("/tmp/broker-scratch"))
    }

    #[test]
    fn allows_read_write_inside_scratch() {
        let h = host();
        assert_eq!(
            h.handle(&BrokerRequest {
                op: BrokerOp::OpenRead,
                path: "/tmp/broker-scratch/a/b.txt".into(),
            }),
            BrokerResponse::Allowed
        );
        assert_eq!(
            h.handle(&BrokerRequest {
                op: BrokerOp::OpenWrite,
                path: "/tmp/broker-scratch/out.pdf".into(),
            }),
            BrokerResponse::Allowed
        );
    }

    #[test]
    fn denies_outside_scratch() {
        let h = host();
        for path in ["/etc/passwd", "/home/user/secret.txt", "/tmp/other/x"] {
            let r = h.handle(&BrokerRequest {
                op: BrokerOp::OpenRead,
                path: path.into(),
            });
            assert!(
                matches!(r, BrokerResponse::Denied { .. }),
                "{path} should be denied"
            );
        }
    }

    #[test]
    fn denies_floor_escapes() {
        let h = host();
        // `..` escape above the root.
        let r = h.handle(&BrokerRequest {
            op: BrokerOp::OpenRead,
            path: "/tmp/broker-scratch/../../etc/passwd".into(),
        });
        assert!(matches!(r, BrokerResponse::Denied { .. }));
    }

    #[test]
    fn create_requires_writable_parent() {
        let h = host();
        assert_eq!(
            h.handle(&BrokerRequest {
                op: BrokerOp::Create,
                path: "/tmp/broker-scratch/new.txt".into(),
            }),
            BrokerResponse::Allowed
        );
        // Creating inside the read-only /usr/share base is denied.
        let r = h.handle(&BrokerRequest {
            op: BrokerOp::Create,
            path: "/usr/share/x/new.txt".into(),
        });
        assert!(matches!(r, BrokerResponse::Denied { .. }));
    }

    #[test]
    fn network_profile_denies_everything() {
        let h = BrokerHost::new(profiles::network());
        for op in [BrokerOp::OpenRead, BrokerOp::Stat, BrokerOp::Create] {
            let r = h.handle(&BrokerRequest {
                op,
                path: "/tmp/x".into(),
            });
            assert!(matches!(r, BrokerResponse::Denied { .. }));
        }
    }

    #[test]
    fn in_process_transport_round_trip() {
        let mut t = InProcessBroker { host: Some(host()) };
        assert_eq!(
            t.request(&BrokerRequest {
                op: BrokerOp::OpenRead,
                path: "/tmp/broker-scratch/x.txt".into(),
            })
            .unwrap(),
            BrokerResponse::Allowed
        );
        assert!(matches!(
            t.request(&BrokerRequest {
                op: BrokerOp::OpenRead,
                path: "/etc/shadow".into(),
            })
            .unwrap(),
            BrokerResponse::Denied { .. }
        ));
    }
}
