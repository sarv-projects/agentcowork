//! P7.8 — arg-filtered seccomp policy DSL (doc 64 S5 — chromium
//! `bpf_network_policy_linux.cc`). A policy is a list of syscall rules; each
//! rule can nest `Switch`/`Case` filters on syscall arguments (e.g. level 1:
//! syscall number, level 2: `SOL_SOCKET` level, level 3: `optname`), with a
//! `CrashSIGSYS` default at every level — a violation kills the worker in
//! dev rather than silently continuing.
//!
//! This module is the declarative policy model: building, validating, and
//! testing the exact filter structure. The BPF program assembly + install is
//! the apply-time seam (like the sandbox profile's `apply`), on Linux with
//! the libseccomp backend.

use serde::{Deserialize, Serialize};

/// The action a filter takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Allow the syscall.
    Allow,
    /// Refuse with an errno (e.g. `EPERM`).
    Errno(i32),
    /// Crash the worker with SIGSYS — the dev-time default for anything not
    /// explicitly allowed (chromium `Default(CrashSIGSYS)`).
    CrashSIGSYS,
}

/// An argument filter at one level of a nested switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArgFilter {
    /// `arg == value` (e.g. `level == SOL_SOCKET`).
    Eq { arg: u8, value: u64 },
    /// `(arg & mask) == value` (bitfield compare, e.g. `O_NOFOLLOW`).
    MaskEq { arg: u8, mask: u64, value: u64 },
    /// Any value (default case in a nested switch).
    Any,
}

/// One branch of a nested `Switch(level).Case(...)` chain: the full filter
/// path (one [`ArgFilter`] per nesting level) and the leaf action. Multiple
/// branches on one rule are sibling `Case` arms at the top level; the
/// implicit `Default(CrashSIGSYS)` at every level is the policy's `default`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branch {
    pub args: Vec<ArgFilter>,
    pub action: Action,
}

/// One syscall rule: the syscall number plus its case branches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyscallRule {
    /// Linux syscall number (e.g. 41 = socket, 42 = connect, 257 = openat).
    pub nr: i64,
    /// `Switch(nr).Case(arg-filters, action)` branches, checked in order;
    /// the first fully-matching branch decides, else the policy default.
    pub branches: Vec<Branch>,
}

/// The full policy: the syscall rules + the fallback for anything unmatched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeccompPolicy {
    pub name: String,
    pub rules: Vec<SyscallRule>,
    /// Default action for syscalls with no rule (fail-closed: crash).
    pub default: Action,
}

/// Errors from building a policy.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SeccompError {
    #[error("policy `{0}` must be fail-closed — default cannot be Allow")]
    DefaultNotFailClosed(String),
    #[error("rule for syscall {nr} has no branches")]
    NoBranches { nr: i64 },
    #[error("branch for syscall {nr} has an empty filter path")]
    EmptyBranch { nr: i64 },
}

/// Linux syscall numbers used by the profiles (x86_64).
pub mod nr {
    pub const READ: i64 = 0;
    pub const WRITE: i64 = 1;
    pub const CLOSE: i64 = 3;
    pub const FSTAT: i64 = 5;
    pub const POLL: i64 = 7;
    pub const MMAP: i64 = 9;
    pub const MUNMAP: i64 = 11;
    pub const BRK: i64 = 12;
    pub const OPENAT: i64 = 257;
    pub const NEWFSTATAT: i64 = 262;
    pub const SOCKET: i64 = 41;
    pub const CONNECT: i64 = 42;
    pub const SENDTO: i64 = 44;
    pub const RECVFROM: i64 = 45;
    pub const SENDMSG: i64 = 46;
    pub const RECVMSG: i64 = 47;
    pub const EXECVE: i64 = 59;
    pub const CLONE: i64 = 56;
    pub const EXIT: i64 = 60;
}

/// Socket levels / optnames used by the network-worker profile
/// (chromium's `bpf_network_policy_linux.cc` vocabulary).
pub const SOL_SOCKET: u64 = 1;
pub const SO_ERROR: u64 = 4;

impl SeccompPolicy {
    /// Validate fail-closed structure: default must not be Allow; every
    /// rule has a non-empty case chain whose leaf is the last filter.
    pub fn validate(&self) -> Result<(), SeccompError> {
        if self.default == Action::Allow {
            return Err(SeccompError::DefaultNotFailClosed(self.name.clone()));
        }
        for rule in &self.rules {
            if rule.branches.is_empty() {
                return Err(SeccompError::NoBranches { nr: rule.nr });
            }
            if rule.branches.iter().any(|b| b.args.is_empty()) {
                return Err(SeccompError::EmptyBranch { nr: rule.nr });
            }
        }
        Ok(())
    }

    /// The chromium `bpf_network_policy_linux.cc` pattern: socket-level
    /// nested switches — `Switch(level).Case(SOL_SOCKET, Switch(optname).
    /// Case(SO_ERROR, Allow).Default(CrashSIGSYS)).Default(CrashSIGSYS)`.
    pub fn network_worker() -> Self {
        SeccompPolicy {
            name: "network-worker".into(),
            rules: vec![
                // socket(domain, type, protocol) — chromium's
                // `Switch(domain).Case(AF_INET, Allow).Case(AF_INET6, Allow)
                // .Default(CrashSIGSYS)`; anything but AF_INET/AF_INET6
                // crashes the worker.
                SyscallRule {
                    nr: nr::SOCKET,
                    branches: vec![
                        Branch {
                            args: vec![ArgFilter::Eq { arg: 0, value: 2 }],
                            action: Action::Allow,
                        },
                        Branch {
                            args: vec![ArgFilter::Eq { arg: 0, value: 10 }],
                            action: Action::Allow,
                        },
                    ],
                },
                // connect/sendto/recvfrom/sendmsg/recvmsg — allowed.
                SyscallRule {
                    nr: nr::CONNECT,
                    branches: vec![Branch {
                        args: vec![ArgFilter::Any],
                        action: Action::Allow,
                    }],
                },
                SyscallRule {
                    nr: nr::SENDTO,
                    branches: vec![Branch {
                        args: vec![ArgFilter::Any],
                        action: Action::Allow,
                    }],
                },
                SyscallRule {
                    nr: nr::RECVFROM,
                    branches: vec![Branch {
                        args: vec![ArgFilter::Any],
                        action: Action::Allow,
                    }],
                },
                SyscallRule {
                    nr: nr::SENDMSG,
                    branches: vec![Branch {
                        args: vec![ArgFilter::Any],
                        action: Action::Allow,
                    }],
                },
                SyscallRule {
                    nr: nr::RECVMSG,
                    branches: vec![Branch {
                        args: vec![ArgFilter::Any],
                        action: Action::Allow,
                    }],
                },
            ],
            default: Action::CrashSIGSYS,
        }
    }

    /// Does the policy allow a given syscall with the given arg values?
    /// The first fully-matching branch (in case order) decides; no branch
    /// match falls through to the fail-closed default.
    pub fn allows(&self, nr: i64, args: &[u64]) -> bool {
        let Some(rule) = self.rules.iter().find(|r| r.nr == nr) else {
            return self.default == Action::Allow;
        };
        for branch in &rule.branches {
            let matched = branch.args.iter().enumerate().all(|(level, filter)| {
                let _value = args.get(level).copied().unwrap_or(0);
                match filter {
                    ArgFilter::Eq { arg, value: want } => {
                        args.get(*arg as usize).copied() == Some(*want)
                    }
                    ArgFilter::MaskEq {
                        arg,
                        mask,
                        value: want,
                    } => args
                        .get(*arg as usize)
                        .map(|v| v & *mask == *want)
                        .unwrap_or(false),
                    ArgFilter::Any => true,
                }
            });
            if matched {
                return branch.action == Action::Allow;
            }
        }
        self.default == Action::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_worker_validates() {
        SeccompPolicy::network_worker().validate().unwrap();
    }

    #[test]
    fn network_worker_allows_socket_connect_send() {
        let p = SeccompPolicy::network_worker();
        assert!(p.allows(nr::SOCKET, &[2, 1, 6])); // AF_INET
        assert!(p.allows(nr::SOCKET, &[10, 1, 6])); // AF_INET6
        assert!(p.allows(nr::CONNECT, &[3, 0, 0]));
        assert!(p.allows(nr::SENDTO, &[3, 0, 0, 0, 0, 0]));
        // Anything else crashes (fail-closed).
        assert!(!p.allows(nr::SOCKET, &[1, 1, 6])); // AF_UNIX not allowed
        assert!(!p.allows(nr::OPENAT, &[0, 0, 0])); // no fs at all
        assert!(!p.allows(nr::EXECVE, &[0, 0, 0]));
    }

    #[test]
    fn fail_closed_validation_rejects_allow_default() {
        let mut p = SeccompPolicy::network_worker();
        p.default = Action::Allow;
        assert!(matches!(
            p.validate(),
            Err(SeccompError::DefaultNotFailClosed(_))
        ));
    }

    #[test]
    fn mask_eq_bitfield_compare() {
        let p = SeccompPolicy {
            name: "t".into(),
            rules: vec![SyscallRule {
                nr: 257,
                branches: vec![Branch {
                    // flags & O_NOFOLLOW(0x20000) must be 0
                    args: vec![ArgFilter::MaskEq {
                        arg: 2,
                        mask: 0x20000,
                        value: 0,
                    }],
                    action: Action::Allow,
                }],
            }],
            default: Action::CrashSIGSYS,
        };
        p.validate().unwrap();
        assert!(p.allows(257, &[0, 0, 0]));
        assert!(!p.allows(257, &[0, 0, 0x20000]));
    }
}
