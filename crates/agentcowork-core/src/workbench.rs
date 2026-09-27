//! P71.10 (ADR-0008) — Work/Run-owned resource-lease and fencing coordinator.
//!
//! The coordinator answers one question: *which Work/Run may exercise a
//! resource generation right now?* It owns lease records, the monotonic
//! per-resource fence, generation tracking, and the durable lifecycle facts
//! the owner persists on the existing Work event stream. It is not a
//! permission system (Guard authorizes), not a capability registry, not a
//! scheduler, and not a second event log: every mutation emits a
//! [`LeaseLifecycleFact`] the owner persists and recovery replays.
//!
//! Bearer custody (ADR-0008 §2.1): possession material lives in
//! [`LeaseBearer`], which is private to this module — no `Serialize`, a
//! redacted `Debug`, zeroed on drop. `acquire` hands the bearer to the
//! in-Rust owner once; every later exercise presents `(handle, bearer)` and
//! a forged or copied handle without the bearer is refused. Public surfaces
//! expose only [`LeaseAttachment`] (id, generation, fence, status).
//!
//! Recovery (RECOVERY.md §6.1): a crashed holder's lease is `uncertain`,
//! never free; reclaim verifies process/resource identity and the absence of
//! a live effect, then fences before any replacement. Unknown post-effect
//! outcomes stay [`PostEffectOutcome::Uncertain`] through reconcile.
//!
//! This module does not touch delegation, edit, or shadow paths (the P64
//! lane owns `cua.rs`, `governor.rs`, `tools.rs`, `execution.rs`).

use std::collections::HashMap;

use agentcowork_types::workbench::{
    BindingRef, DraftRef, LEASE_EVENT_ACQUIRED, LEASE_EVENT_ACTOR_REVALIDATED, LEASE_EVENT_EXPIRED,
    LEASE_EVENT_RECLAIM_PENDING, LEASE_EVENT_RECLAIMED, LEASE_EVENT_RELEASED, LEASE_EVENT_RENEWED,
    LEASE_EVENT_REVOKED, LEASE_EVENT_UNCERTAIN, LeaseAccess, LeaseAttachment, LeaseLifecycleFact,
    LeaseState, LensState, OwnerContext, PostEffectOutcome, ProjectionFreshness, QuestionRef,
    QueueItemRef, ReceiptRef, ResourceKind, ReviewRef, RunRef, SessionWorkbenchProjection,
    TypedResourceKey, WorkRef, WorkbenchResourceRef,
};
use agentcowork_types::{AgentBindingId, EventEnvelope, LeaseId, SessionId, SessionKind};
use thiserror::Error;

// ────────────────────────────────────────────────────────────────────────
// Private bearer custody
// ────────────────────────────────────────────────────────────────────────

/// Opaque internal handle. Meaningful only beside its [`LeaseBearer`] inside
/// Rust; never serialized, never logged, never sent over IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LeaseHandle(u64);

/// Possession material for one lease. Held only by the Rust-private owner:
/// the struct is public so it can appear in public signatures, but the field
/// is private and there is no public constructor — only [`WorkRunLeaseCoordinator::acquire`]
/// (via [`AcquiredLease`]/[`TakeoverOutcome`]) can mint one. Deliberately no
/// `Serialize` (it can never cross IPC/logs), a redacted `Debug`, and zeroed
/// memory on drop.
///
/// Cloning is confined to this module: the coordinator keeps one custody
/// copy and hands the possession half to the in-Rust owner at acquire time.
#[derive(Clone)]
pub struct LeaseBearer(Box<[u8; 32]>);

impl LeaseBearer {
    fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = Box::new([0u8; 32]);
        rand::thread_rng().fill_bytes(&mut *bytes);
        // A zero bearer proves nothing; regenerate rather than issue one.
        if bytes.iter().all(|b| *b == 0) {
            bytes[0] = 1;
        }
        Self(bytes)
    }

    /// Fresh bearer for a replayed lease. Nobody holds the matching half,
    /// so a rebuilt lease is unexercisable until re-acquired — fail-closed.
    fn fresh_unheld() -> Self {
        Self::generate()
    }

    fn matches(&self, other: &LeaseBearer) -> bool {
        self.0
            .iter()
            .zip(other.0.iter())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }
}

impl std::fmt::Debug for LeaseBearer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LeaseBearer([redacted])")
    }
}

impl Drop for LeaseBearer {
    fn drop(&mut self) {
        for b in self.0.iter_mut() {
            *b = 0;
        }
    }
}

// ────────────────────────────────────────────────────────────────────────
// Errors: typed contention, staleness, and custody failures
// ────────────────────────────────────────────────────────────────────────

/// Every refusal a lease boundary can produce. Stale generation and stale
/// fence are refused — never silently retargeted at a newer resource.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LeaseError {
    /// A second incompatible holder exists. No second lease, no copied
    /// handle, no last-writer-wins write: wait, or release and re-acquire.
    #[error("lease conflict on {resource_key}: {access} refused by holder {holder}")]
    Conflict {
        resource_key: String,
        access: String,
        holder: String,
    },
    /// The physical version moved (or was never observed at this generation).
    #[error("stale resource {resource_key}: observed generation {observed}, current {current}")]
    StaleResource {
        resource_key: String,
        observed: u64,
        current: u64,
    },
    /// The lease authority moved (revocation, reclaim, takeover, restart).
    #[error("stale lease {lease_id}: fence {fence} is not current {current}")]
    StaleLease {
        lease_id: String,
        fence: u64,
        current: u64,
    },
    #[error("unknown lease {lease_id}")]
    UnknownLease { lease_id: String },
    #[error("unknown lease handle")]
    UnknownHandle,
    /// A copied or forged handle without the bearer proves nothing.
    /// The wording names the proof, never the material: error strings are
    /// scanned for secret-shaped keys alongside projections and logs.
    #[error("lease possession proof mismatch for {lease_id}")]
    BearerMismatch { lease_id: String },
    /// The lease is not live (released, expired, revoked, reclaimed…).
    #[error("lease {lease_id} is {state}, not exercisable")]
    NotActive { lease_id: String, state: String },
    /// Act attempted by a binding that is not the lease's actor — e.g. the
    /// parked binding after a switch, or another Session's binding.
    #[error("lease {lease_id} is bound to another actor binding")]
    WrongBinding { lease_id: String },
    /// The holder is gone or unverified; reconcile before acting.
    #[error("lease {lease_id} holder is not live; reconcile first")]
    HolderNotLive { lease_id: String },
    /// Wall-clock lifetime elapsed. Sweep first, then re-acquire.
    #[error("lease {lease_id} expired")]
    Expired { lease_id: String },
    /// Reclaim requires reconciliation first — a crashed holder is never
    /// assumed harmless.
    #[error("lease {lease_id} must be reconciled before reclaim")]
    MustReconcileFirst { lease_id: String },
    /// Reclaim requires proof the process is dead and no effect is live.
    #[error("lease {lease_id} reclaim refused: holder may still be live")]
    HolderMayBeLive { lease_id: String },
    /// A replayed fact would roll the fence backwards.
    #[error("stale lease fact for {lease_id}: fence {fence} behind {current}")]
    StaleFact {
        lease_id: String,
        fence: u64,
        current: u64,
    },
    #[error("invalid lease request: {0}")]
    InvalidRequest(String),
}

// ────────────────────────────────────────────────────────────────────────
// Records and coordinator state
// ────────────────────────────────────────────────────────────────────────

struct LeaseRecord {
    lease: agentcowork_types::workbench::ResourceLease,
    bearer: LeaseBearer,
    /// The holder process is believed alive. False after replay/rebuild,
    /// binding loss, or crash — exercise refuses until revalidated.
    holder_live: bool,
    /// Original TTL, so renewal extends by the same bound.
    ttl_ms: u64,
}

/// What `acquire` hands the private owner: the internal handle, the bearer
/// (once — it is never re-issued), the safe attachment for projections, and
/// the durable fact the owner must persist on the Work event stream.
#[derive(Debug)]
pub struct AcquiredLease {
    pub handle: LeaseHandle,
    pub bearer: LeaseBearer,
    pub attachment: LeaseAttachment,
    pub fact: LeaseLifecycleFact,
}

/// What `takeover` returns: the replacement lease plus every revocation fact
/// that fenced the old holders out, in order.
#[derive(Debug)]
pub struct TakeoverOutcome {
    pub handle: LeaseHandle,
    pub bearer: LeaseBearer,
    pub attachment: LeaseAttachment,
    pub facts: Vec<LeaseLifecycleFact>,
}

/// Reconciling an uncertain lease: either the settled effect advances it to
/// reclaim-pending, or the unknown outcome keeps it uncertain. The fact is
/// boxed: the enum stays small while the durable record rides the heap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileOutcome {
    AdvancedToReclaim(Box<LeaseLifecycleFact>),
    StillUncertain,
}

/// Cursor-rebuild report. Malformed or unknown envelopes are skipped and
/// counted — never fabricated into lease state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebuildReport {
    pub cursor: u64,
    pub applied: usize,
    pub skipped: usize,
}

/// Owned inputs for one projection build. Canonical summaries come from the
/// Work/Event/Receipt spine; lease attachments come from this coordinator.
#[derive(Debug, Clone)]
pub struct ProjectionInput {
    pub session_id: SessionId,
    pub session_kind: SessionKind,
    pub active_owner: Option<OwnerContext>,
    pub event_cursor: u64,
    pub freshness: Option<ProjectionFreshness>,
    pub works: Vec<WorkRef>,
    pub runs: Vec<RunRef>,
    pub bindings: Vec<BindingRef>,
    pub lenses: Vec<LensState>,
    pub resource_refs: Vec<WorkbenchResourceRef>,
    pub drafts: Vec<DraftRef>,
    pub prompt_queue: Vec<QueueItemRef>,
    pub pending_questions: Vec<QuestionRef>,
    pub pending_reviews: Vec<ReviewRef>,
    pub receipt_refs: Vec<ReceiptRef>,
}

impl Default for ProjectionInput {
    fn default() -> Self {
        Self {
            session_id: SessionId::new(""),
            session_kind: SessionKind::Interactive,
            active_owner: None,
            event_cursor: 0,
            freshness: None,
            works: Vec::new(),
            runs: Vec::new(),
            bindings: Vec::new(),
            lenses: Vec::new(),
            resource_refs: Vec::new(),
            drafts: Vec::new(),
            prompt_queue: Vec::new(),
            pending_questions: Vec::new(),
            pending_reviews: Vec::new(),
            receipt_refs: Vec::new(),
        }
    }
}

/// The Work/Run-owned fencing coordinator (ADR-0008 §2.3).
///
/// One coordinator per supervisor. All time is caller-supplied (`now_ms`)
/// so fencing and expiry are deterministic under test.
pub struct WorkRunLeaseCoordinator {
    leases: HashMap<String, LeaseRecord>,
    handle_index: HashMap<u64, String>,
    /// Canonical resource key → current monotonic fence (0 = never fenced).
    fences: HashMap<String, u64>,
    /// Canonical resource key → current known physical generation.
    generations: HashMap<String, u64>,
    /// Binding id → provider-handle generation (restarts rotate it).
    provider_generations: HashMap<String, u64>,
    /// Session id → attached lease ids. Lookup scope only.
    session_attachments: HashMap<String, Vec<String>>,
    next_handle: u64,
    next_lease_seq: u64,
}

impl Default for WorkRunLeaseCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

/// Redacted by construction: counts and ids only, never bearers or handles.
impl std::fmt::Debug for WorkRunLeaseCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkRunLeaseCoordinator")
            .field("leases", &self.leases.len())
            .field("fences", &self.fences.len())
            .field("generations", &self.generations.len())
            .field("sessions", &self.session_attachments.len())
            .finish()
    }
}

impl WorkRunLeaseCoordinator {
    pub fn new() -> Self {
        Self {
            leases: HashMap::new(),
            handle_index: HashMap::new(),
            fences: HashMap::new(),
            generations: HashMap::new(),
            provider_generations: HashMap::new(),
            session_attachments: HashMap::new(),
            next_handle: 1,
            next_lease_seq: 1,
        }
    }

    // ── introspection (safe: attachments only) ──

    pub fn lease_count(&self) -> usize {
        self.leases.len()
    }

    pub fn current_fence(&self, key: &TypedResourceKey) -> u64 {
        self.fences.get(&key.canonical_key()).copied().unwrap_or(0)
    }

    pub fn current_generation(&self, key: &TypedResourceKey) -> Option<u64> {
        self.generations.get(&key.canonical_key()).copied()
    }

    pub fn provider_generation(&self, binding: &AgentBindingId) -> u64 {
        self.provider_generations
            .get(binding.as_str())
            .copied()
            .unwrap_or(0)
    }

    pub fn attachment_for(&self, lease_id: &LeaseId) -> Option<LeaseAttachment> {
        self.leases
            .get(lease_id.as_str())
            .map(|r| r.lease.attachment())
    }

    pub fn attachments_for(&self, session_id: &SessionId) -> Vec<LeaseAttachment> {
        self.session_attachments
            .get(session_id.as_str())
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.leases.get(id).map(|r| r.lease.attachment()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn lease_state(&self, lease_id: &LeaseId) -> Option<LeaseState> {
        self.leases.get(lease_id.as_str()).map(|r| r.lease.state)
    }

    /// The lease currently recorded against a canonical resource key.
    ///
    /// The delegation path keys a lease by the child Work rather than by a
    /// Session, so it needs a resource→lease lookup. Non-secret: it returns the
    /// opaque id only, and a resource with no lease is `None` rather than a
    /// synthesized one.
    pub fn lease_id_for_resource(&self, canonical_key: &str) -> Option<LeaseId> {
        self.leases
            .values()
            .find(|r| r.lease.resource.canonical_key() == canonical_key)
            .map(|r| r.lease.lease_id.clone())
    }

    /// The engine publishes the physical version it observes. Recovery and
    /// adapters call this; admission compares against it.
    pub fn note_resource_generation(&mut self, key: &TypedResourceKey, generation: u64) {
        self.generations.insert(key.canonical_key(), generation);
    }

    // ── admission ──

    fn live_holders(&self, canonical: &str, now_ms: u64) -> Vec<&LeaseRecord> {
        self.leases
            .values()
            .filter(|r| {
                r.lease.resource.canonical_key() == canonical
                    && r.lease.state.is_live()
                    && r.holder_live
                    && now_ms <= r.lease.expires_at_ms
            })
            .collect()
    }

    fn mint_lease_id(&mut self, now_ms: u64) -> LeaseId {
        let seq = self.next_lease_seq;
        self.next_lease_seq += 1;
        LeaseId::new(format!("lease-{now_ms}-{seq}"))
    }

    fn mint_handle(&mut self) -> LeaseHandle {
        let handle = LeaseHandle(self.next_handle);
        self.next_handle += 1;
        handle
    }

    /// Admit a lease request (ADR-0008 §2.3 rules 1–4). Compatible
    /// observation/view leases share the resource generation; an edit,
    /// action, or control lease is exclusive against every incompatible
    /// lease. A refused writer gets a typed conflict — never a second lease.
    #[allow(clippy::too_many_arguments)]
    pub fn acquire(
        &mut self,
        owner: OwnerContext,
        resource: TypedResourceKey,
        access: LeaseAccess,
        scope_fingerprint: &str,
        observed_generation: u64,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<AcquiredLease, LeaseError> {
        owner.validate().map_err(LeaseError::InvalidRequest)?;
        resource.validate().map_err(LeaseError::InvalidRequest)?;
        if scope_fingerprint.is_empty() {
            return Err(LeaseError::InvalidRequest(
                "scope_fingerprint must not be empty".into(),
            ));
        }
        if ttl_ms == 0 {
            return Err(LeaseError::InvalidRequest(
                "lease lifetime must be bounded and non-zero".into(),
            ));
        }
        if observed_generation != resource.generation {
            return Err(LeaseError::InvalidRequest(format!(
                "observed generation {observed_generation} disagrees with resource generation {}",
                resource.generation
            )));
        }
        let canonical = resource.canonical_key();
        match self.generations.get(&canonical).copied() {
            Some(current) if current != observed_generation => {
                return Err(LeaseError::StaleResource {
                    resource_key: canonical,
                    observed: observed_generation,
                    current,
                });
            }
            Some(_) => {}
            None => {
                self.generations
                    .insert(canonical.clone(), observed_generation);
            }
        }

        let holders = self.live_holders(&canonical, now_ms);
        if let Some(blocker) = holders
            .iter()
            .find(|h| !access.compatible_with(h.lease.access))
        {
            return Err(LeaseError::Conflict {
                resource_key: canonical,
                access: access.as_str().into(),
                holder: blocker.lease.lease_id.as_str().into(),
            });
        }

        // Fence: shared observers reuse the current authority value; an
        // exclusive holder advances it so any earlier holder goes stale.
        let fence = if access.is_exclusive() {
            self.fences.get(&canonical).copied().unwrap_or(0) + 1
        } else if self.fences.get(&canonical).copied().unwrap_or(0) == 0 {
            1
        } else {
            self.fences[&canonical]
        };
        self.fences.insert(canonical.clone(), fence);

        let lease_id = self.mint_lease_id(now_ms);
        let handle = self.mint_handle();
        let lease = agentcowork_types::workbench::ResourceLease {
            lease_id: lease_id.clone(),
            resource: TypedResourceKey {
                generation: observed_generation,
                ..resource.clone()
            },
            owner_work_id: owner.work_id.clone(),
            owner_run_id: owner.run_id.clone(),
            actor_binding_id: owner.binding_id.clone(),
            session_scope: owner.session_id.clone(),
            access,
            resource_generation: observed_generation,
            fence,
            scope_fingerprint: scope_fingerprint.into(),
            state: LeaseState::Active,
            issued_at_ms: now_ms,
            expires_at_ms: now_ms + ttl_ms,
            last_heartbeat_ms: now_ms,
        };
        lease.validate().map_err(LeaseError::InvalidRequest)?;
        let attachment = lease.attachment();
        let fact = LeaseLifecycleFact::new(
            lease_id.clone(),
            LEASE_EVENT_ACQUIRED,
            fence,
            observed_generation,
            now_ms,
        )
        .with_detail(format!(
            "access={} fence={fence} generation={observed_generation}",
            access.as_str()
        ))
        .with_acquire_context(
            access,
            owner.clone(),
            lease.resource.clone(),
            scope_fingerprint,
        );
        fact.validate().map_err(LeaseError::InvalidRequest)?;
        // One bearer, two halves: the coordinator's custody copy and the
        // possession half handed to the in-Rust owner. A copied handle
        // without this bearer proves nothing.
        let bearer = LeaseBearer::generate();
        self.handle_index
            .insert(handle.0, lease_id.as_str().to_string());
        self.session_attachments
            .entry(owner.session_id.as_str().to_string())
            .or_default()
            .push(lease_id.as_str().to_string());
        self.leases.insert(
            lease_id.as_str().to_string(),
            LeaseRecord {
                lease,
                bearer: bearer.clone(),
                holder_live: true,
                ttl_ms,
            },
        );
        Ok(AcquiredLease {
            handle,
            bearer,
            attachment,
            fact,
        })
    }

    fn record_mut(
        &mut self,
        handle: &LeaseHandle,
        bearer: &LeaseBearer,
    ) -> Result<&mut LeaseRecord, LeaseError> {
        let id = self
            .handle_index
            .get(&handle.0)
            .ok_or(LeaseError::UnknownHandle)?;
        let record = self
            .leases
            .get_mut(id)
            .ok_or_else(|| LeaseError::UnknownLease {
                lease_id: id.clone(),
            })?;
        if !record.bearer.matches(bearer) {
            return Err(LeaseError::BearerMismatch {
                lease_id: record.lease.lease_id.as_str().into(),
            });
        }
        Ok(record)
    }

    fn check_exercisable(
        &self,
        record: &LeaseRecord,
        binding: &AgentBindingId,
        observed_generation: u64,
        now_ms: u64,
    ) -> Result<(), LeaseError> {
        let id = record.lease.lease_id.as_str().to_string();
        let canonical = record.lease.resource.canonical_key();
        // Fence first: revocation, reclaim, takeover, and restart all advance
        // it, so an evicted holder's next observation/action/commit reads as
        // stale even when its wall-clock timeout has not elapsed (ADR-0008
        // §2.3 rule 5). A normally released lease keeps its fence and falls
        // through to the state check below.
        let current_fence = self.fences.get(&canonical).copied().unwrap_or(0);
        if record.lease.fence != current_fence {
            return Err(LeaseError::StaleLease {
                lease_id: id,
                fence: record.lease.fence,
                current: current_fence,
            });
        }
        if !record.lease.state.is_live() {
            // Wall-clock expiry reports as expiry, not a generic refusal:
            // the owner sweeps, then re-acquires.
            if record.lease.state == LeaseState::Expired {
                return Err(LeaseError::Expired { lease_id: id });
            }
            return Err(LeaseError::NotActive {
                lease_id: id,
                state: record.lease.state.as_str().into(),
            });
        }
        if now_ms > record.lease.expires_at_ms {
            return Err(LeaseError::Expired { lease_id: id });
        }
        if !record.holder_live {
            return Err(LeaseError::HolderNotLive { lease_id: id });
        }
        if record.lease.actor_binding_id != *binding {
            return Err(LeaseError::WrongBinding { lease_id: id });
        }
        let current_gen = self
            .generations
            .get(&canonical)
            .copied()
            .unwrap_or(observed_generation);
        if observed_generation != record.lease.resource_generation
            || observed_generation != current_gen
        {
            return Err(LeaseError::StaleResource {
                resource_key: canonical,
                observed: observed_generation,
                current: current_gen,
            });
        }
        Ok(())
    }

    /// The pre-effect gate (ADR-0008 §2.2): revalidate identity, generation,
    /// lease state, fence, and actor binding before any checkpoint, renewal,
    /// or receipt commit. Stale is refused, never retargeted.
    pub fn validate_exercise(
        &self,
        handle: &LeaseHandle,
        bearer: &LeaseBearer,
        binding: &AgentBindingId,
        observed_generation: u64,
        now_ms: u64,
    ) -> Result<LeaseAttachment, LeaseError> {
        let id = self
            .handle_index
            .get(&handle.0)
            .ok_or(LeaseError::UnknownHandle)?;
        let record = self
            .leases
            .get(id)
            .ok_or_else(|| LeaseError::UnknownLease {
                lease_id: id.clone(),
            })?;
        if !record.bearer.matches(bearer) {
            return Err(LeaseError::BearerMismatch {
                lease_id: id.clone(),
            });
        }
        self.check_exercisable(record, binding, observed_generation, now_ms)?;
        Ok(record.lease.attachment())
    }

    /// Renew within the same generation and owner tuple. A stale generation
    /// or fenced lease is refused; renewal never repairs staleness.
    pub fn renew(
        &mut self,
        handle: &LeaseHandle,
        bearer: &LeaseBearer,
        observed_generation: u64,
        now_ms: u64,
    ) -> Result<(LeaseAttachment, LeaseLifecycleFact), LeaseError> {
        // Immutable check phase: resolve, custody, liveness, generation,
        // fence, and actor — before any mutation.
        let id = self
            .handle_index
            .get(&handle.0)
            .ok_or(LeaseError::UnknownHandle)?
            .clone();
        let (binding, canonical, fence, generation, ttl) = {
            let record = self
                .leases
                .get(&id)
                .ok_or_else(|| LeaseError::UnknownLease {
                    lease_id: id.clone(),
                })?;
            if !record.bearer.matches(bearer) {
                return Err(LeaseError::BearerMismatch {
                    lease_id: id.clone(),
                });
            }
            if !record.lease.state.is_live() {
                if record.lease.state == LeaseState::Expired {
                    return Err(LeaseError::Expired {
                        lease_id: id.clone(),
                    });
                }
                return Err(LeaseError::NotActive {
                    lease_id: id.clone(),
                    state: record.lease.state.as_str().into(),
                });
            }
            if now_ms > record.lease.expires_at_ms {
                return Err(LeaseError::Expired {
                    lease_id: id.clone(),
                });
            }
            if !record.holder_live {
                return Err(LeaseError::HolderNotLive {
                    lease_id: id.clone(),
                });
            }
            (
                record.lease.actor_binding_id.clone(),
                record.lease.resource.canonical_key(),
                record.lease.fence,
                record.lease.resource_generation,
                record.ttl_ms,
            )
        };
        let current_gen = self
            .generations
            .get(&canonical)
            .copied()
            .unwrap_or(observed_generation);
        if observed_generation != generation || observed_generation != current_gen {
            return Err(LeaseError::StaleResource {
                resource_key: canonical,
                observed: observed_generation,
                current: current_gen,
            });
        }
        let current_fence = self.fences.get(&canonical).copied().unwrap_or(0);
        if fence != current_fence {
            return Err(LeaseError::StaleLease {
                lease_id: id.clone(),
                fence,
                current: current_fence,
            });
        }
        // Mutation phase.
        let record = self
            .leases
            .get_mut(&id)
            .ok_or_else(|| LeaseError::UnknownLease {
                lease_id: id.clone(),
            })?;
        if record.lease.actor_binding_id != binding {
            return Err(LeaseError::WrongBinding { lease_id: id });
        }
        record.lease.expires_at_ms = now_ms + ttl;
        record.lease.last_heartbeat_ms = now_ms;
        let attachment = record.lease.attachment();
        let fact = LeaseLifecycleFact::new(
            record.lease.lease_id.clone(),
            LEASE_EVENT_RENEWED,
            record.lease.fence,
            observed_generation,
            now_ms,
        );
        fact.validate().map_err(LeaseError::InvalidRequest)?;
        Ok((attachment, fact))
    }

    /// Liveness evidence only — not authority, not a fact, not persisted.
    pub fn heartbeat(
        &mut self,
        handle: &LeaseHandle,
        bearer: &LeaseBearer,
        now_ms: u64,
    ) -> Result<LeaseAttachment, LeaseError> {
        let record = self.record_mut(handle, bearer)?;
        if !record.lease.state.is_live() {
            return Err(LeaseError::NotActive {
                lease_id: record.lease.lease_id.as_str().into(),
                state: record.lease.state.as_str().into(),
            });
        }
        record.lease.last_heartbeat_ms = now_ms;
        Ok(record.lease.attachment())
    }

    /// Normal release: a distinct durable fact. The fence is unchanged; the
    /// next exclusive acquire advances it.
    pub fn release(
        &mut self,
        handle: &LeaseHandle,
        bearer: &LeaseBearer,
        now_ms: u64,
    ) -> Result<LeaseLifecycleFact, LeaseError> {
        let record = self.record_mut(handle, bearer)?;
        if !record.lease.state.is_live() {
            return Err(LeaseError::NotActive {
                lease_id: record.lease.lease_id.as_str().into(),
                state: record.lease.state.as_str().into(),
            });
        }
        record.lease.state = LeaseState::Released;
        record.holder_live = false;
        let fact = LeaseLifecycleFact::new(
            record.lease.lease_id.clone(),
            LEASE_EVENT_RELEASED,
            record.lease.fence,
            record.lease.resource_generation,
            now_ms,
        );
        fact.validate().map_err(LeaseError::InvalidRequest)?;
        Ok(fact)
    }

    fn bump_fence(fences: &mut HashMap<String, u64>, canonical: &str) -> u64 {
        let next = fences.get(canonical).copied().unwrap_or(0) + 1;
        fences.insert(canonical.to_string(), next);
        next
    }

    /// Revoke by authority (takeover, logout/scope change, Guard decision).
    /// Revocation advances the fence before anything else acts, so the old
    /// holder's next observation/action/commit is stale even if its
    /// wall-clock timeout has not elapsed.
    pub fn revoke(
        &mut self,
        lease_id: &LeaseId,
        detail: &str,
        now_ms: u64,
    ) -> Result<LeaseLifecycleFact, LeaseError> {
        let record =
            self.leases
                .get_mut(lease_id.as_str())
                .ok_or_else(|| LeaseError::UnknownLease {
                    lease_id: lease_id.as_str().into(),
                })?;
        if !record.lease.state.can_transition(LeaseState::Revoked) {
            return Err(LeaseError::NotActive {
                lease_id: lease_id.as_str().into(),
                state: record.lease.state.as_str().into(),
            });
        }
        record.lease.state = LeaseState::Revoked;
        record.holder_live = false;
        let canonical = record.lease.resource.canonical_key();
        let fence = Self::bump_fence(&mut self.fences, &canonical);
        // The record keeps the fence it was issued at; the resource fence
        // advances past it. The old holder's next exercise therefore reads
        // as stale (ADR-0008 §2.3 rule 5), while the fact carries the new
        // fence so replay fences before granting.
        let fact = LeaseLifecycleFact::new(
            lease_id.clone(),
            LEASE_EVENT_REVOKED,
            fence,
            record.lease.resource_generation,
            now_ms,
        )
        .with_detail(detail.to_string());
        fact.validate().map_err(LeaseError::InvalidRequest)?;
        Ok(fact)
    }

    /// Lease replacement / user takeover: revoke every live incompatible
    /// holder, advance the fence, then issue the new lease. Facts are
    /// ordered revocations-first so replay fences before granting.
    #[allow(clippy::too_many_arguments)]
    pub fn takeover(
        &mut self,
        owner: OwnerContext,
        resource: TypedResourceKey,
        access: LeaseAccess,
        scope_fingerprint: &str,
        observed_generation: u64,
        now_ms: u64,
        ttl_ms: u64,
    ) -> Result<TakeoverOutcome, LeaseError> {
        owner.validate().map_err(LeaseError::InvalidRequest)?;
        let canonical = resource.canonical_key();
        let live_ids: Vec<LeaseId> = self
            .live_holders(&canonical, now_ms)
            .iter()
            .filter(|h| !access.compatible_with(h.lease.access))
            .map(|h| h.lease.lease_id.clone())
            .collect();
        let mut facts = Vec::with_capacity(live_ids.len() + 1);
        for id in live_ids {
            facts.push(self.revoke(&id, "takeover: fence advanced before replacement", now_ms)?);
        }
        let acquired = self.acquire(
            owner,
            resource,
            access,
            scope_fingerprint,
            observed_generation,
            now_ms,
            ttl_ms,
        )?;
        facts.push(acquired.fact);
        Ok(TakeoverOutcome {
            handle: acquired.handle,
            bearer: acquired.bearer,
            attachment: acquired.attachment,
            facts,
        })
    }

    /// Sweep wall-clock expiry into durable `Expired` facts.
    pub fn sweep_expired(&mut self, now_ms: u64) -> Vec<LeaseLifecycleFact> {
        let due: Vec<LeaseId> = self
            .leases
            .values()
            .filter(|r| r.lease.state == LeaseState::Active && now_ms > r.lease.expires_at_ms)
            .map(|r| r.lease.lease_id.clone())
            .collect();
        let mut facts = Vec::new();
        for id in due {
            if let Some(record) = self.leases.get_mut(id.as_str()) {
                record.lease.state = LeaseState::Expired;
                record.holder_live = false;
                let fact = LeaseLifecycleFact::new(
                    id,
                    LEASE_EVENT_EXPIRED,
                    record.lease.fence,
                    record.lease.resource_generation,
                    now_ms,
                );
                if fact.validate().is_ok() {
                    facts.push(fact);
                }
            }
        }
        facts
    }

    // ── binding switch, park, provider restart (ADR-0008 §2.4) ──

    /// Parking or switching a binding changes only the binding. The Work/Run
    /// lease survives; the coordinator revalidates `actor_binding_id` as part
    /// of the switch and the old actor can no longer exercise the lease.
    /// Provider-private state is never copied — only the actor id changes.
    pub fn revalidate_actor_binding(
        &mut self,
        lease_id: &LeaseId,
        expected_old: &AgentBindingId,
        new_binding: &AgentBindingId,
        observed_generation: u64,
        now_ms: u64,
    ) -> Result<LeaseLifecycleFact, LeaseError> {
        let record =
            self.leases
                .get_mut(lease_id.as_str())
                .ok_or_else(|| LeaseError::UnknownLease {
                    lease_id: lease_id.as_str().into(),
                })?;
        if !record.lease.state.is_live() {
            return Err(LeaseError::NotActive {
                lease_id: lease_id.as_str().into(),
                state: record.lease.state.as_str().into(),
            });
        }
        if record.lease.actor_binding_id != *expected_old {
            return Err(LeaseError::WrongBinding {
                lease_id: lease_id.as_str().into(),
            });
        }
        if new_binding.as_str().is_empty() {
            return Err(LeaseError::InvalidRequest(
                "new actor binding must not be empty".into(),
            ));
        }
        let canonical = record.lease.resource.canonical_key();
        let current_gen = self
            .generations
            .get(&canonical)
            .copied()
            .unwrap_or(observed_generation);
        if observed_generation != record.lease.resource_generation
            || observed_generation != current_gen
        {
            return Err(LeaseError::StaleResource {
                resource_key: canonical,
                observed: observed_generation,
                current: current_gen,
            });
        }
        record.lease.actor_binding_id = new_binding.clone();
        record.lease.last_heartbeat_ms = now_ms;
        let fact = LeaseLifecycleFact::new(
            lease_id.clone(),
            LEASE_EVENT_ACTOR_REVALIDATED,
            record.lease.fence,
            observed_generation,
            now_ms,
        )
        .with_detail(format!(
            "actor={}->{}",
            expected_old.as_str(),
            new_binding.as_str()
        ));
        fact.validate().map_err(LeaseError::InvalidRequest)?;
        Ok(fact)
    }

    /// A provider restart rotates the provider-handle generation. It creates
    /// no new Session, Work, or Run. Provider-bound leases of that binding
    /// become `uncertain` with an advanced fence; continuation is labelled
    /// restarted, never native resume.
    pub fn note_provider_restart(
        &mut self,
        binding: &AgentBindingId,
        now_ms: u64,
    ) -> Vec<LeaseLifecycleFact> {
        let next = self.provider_generation(binding) + 1;
        self.provider_generations
            .insert(binding.as_str().to_string(), next);
        let affected: Vec<LeaseId> = self
            .leases
            .values()
            .filter(|r| {
                r.lease.actor_binding_id == *binding
                    && r.lease.resource.kind == ResourceKind::ProviderHandle
                    && r.lease.state == LeaseState::Active
            })
            .map(|r| r.lease.lease_id.clone())
            .collect();
        let mut facts = Vec::new();
        for id in affected {
            if let Some(record) = self.leases.get_mut(id.as_str()) {
                record.lease.state = LeaseState::Uncertain;
                record.holder_live = false;
                let canonical = record.lease.resource.canonical_key();
                // Record keeps its issued fence; the resource fence moves
                // past it so the old handle reads as stale, not resumable.
                let fence = Self::bump_fence(&mut self.fences, &canonical);
                let fact = LeaseLifecycleFact::new(
                    id,
                    LEASE_EVENT_UNCERTAIN,
                    fence,
                    record.lease.resource_generation,
                    now_ms,
                )
                .with_detail(format!("provider_handle_generation={next} restarted"));
                if fact.validate().is_ok() {
                    facts.push(fact);
                }
            }
        }
        facts
    }

    /// A physical resource is gone or replaced (tab closed, target window
    /// replaced, file deleted): revoke every live lease on that key so no
    /// holder silently retargets at whatever appears under a reused name.
    /// The lens then reports unavailable/stale with a safe reason.
    pub fn retire_resource(
        &mut self,
        key: &TypedResourceKey,
        detail: &str,
        now_ms: u64,
    ) -> Vec<LeaseLifecycleFact> {
        let canonical = key.canonical_key();
        let affected: Vec<LeaseId> = self
            .leases
            .values()
            .filter(|r| {
                r.lease.resource.canonical_key() == canonical && !r.lease.state.is_terminal()
            })
            .map(|r| r.lease.lease_id.clone())
            .collect();
        let mut facts = Vec::new();
        for id in affected {
            if let Ok(fact) = self.revoke(&id, detail, now_ms) {
                facts.push(fact);
            }
        }
        facts
    }

    /// Logout or configuration-scope change: identity and Work survive, but
    /// leases issued under the old scope fingerprint are revoked and fenced.
    /// No silent scope widening — re-auth creates a new generation/lease.
    pub fn invalidate_scope_fingerprint(
        &mut self,
        scope_fingerprint: &str,
        now_ms: u64,
    ) -> Vec<LeaseLifecycleFact> {
        let affected: Vec<LeaseId> = self
            .leases
            .values()
            .filter(|r| {
                r.lease.scope_fingerprint == scope_fingerprint && !r.lease.state.is_terminal()
            })
            .map(|r| r.lease.lease_id.clone())
            .collect();
        let mut facts = Vec::new();
        for id in affected {
            if let Ok(fact) = self.revoke(
                &id,
                "scope invalidated: logout or config-scope change",
                now_ms,
            ) {
                facts.push(fact);
            }
        }
        facts
    }

    // ── crash recovery (RECOVERY.md §6.1) ──

    /// Crash while a lease is held: the lease is `uncertain`, not free.
    /// Recovery reconciles the in-flight effect before any retry or reclaim.
    pub fn mark_holder_lost(
        &mut self,
        lease_id: &LeaseId,
        now_ms: u64,
    ) -> Result<LeaseLifecycleFact, LeaseError> {
        let record =
            self.leases
                .get_mut(lease_id.as_str())
                .ok_or_else(|| LeaseError::UnknownLease {
                    lease_id: lease_id.as_str().into(),
                })?;
        if record.lease.state != LeaseState::Active {
            return Err(LeaseError::NotActive {
                lease_id: lease_id.as_str().into(),
                state: record.lease.state.as_str().into(),
            });
        }
        record.lease.state = LeaseState::Uncertain;
        record.holder_live = false;
        let fact = LeaseLifecycleFact::new(
            lease_id.clone(),
            LEASE_EVENT_UNCERTAIN,
            record.lease.fence,
            record.lease.resource_generation,
            now_ms,
        )
        .with_detail("holder lost: crash or kill while lease held".to_string());
        fact.validate().map_err(LeaseError::InvalidRequest)?;
        Ok(fact)
    }

    /// Reconcile an uncertain lease against the post-effect outcome. A
    /// settled effect advances the lease to reclaim-pending; an unknown
    /// outcome keeps it uncertain — never success, failure, or cancellation
    /// by inference.
    pub fn reconcile(
        &mut self,
        lease_id: &LeaseId,
        outcome: PostEffectOutcome,
        now_ms: u64,
    ) -> Result<ReconcileOutcome, LeaseError> {
        let record =
            self.leases
                .get_mut(lease_id.as_str())
                .ok_or_else(|| LeaseError::UnknownLease {
                    lease_id: lease_id.as_str().into(),
                })?;
        match record.lease.state {
            LeaseState::Uncertain => {
                if outcome.is_settled() {
                    record.lease.state = LeaseState::ReclaimPending;
                    let fact = LeaseLifecycleFact::new(
                        lease_id.clone(),
                        LEASE_EVENT_RECLAIM_PENDING,
                        record.lease.fence,
                        record.lease.resource_generation,
                        now_ms,
                    )
                    .with_detail(format!("effect outcome={}", outcome.as_str()));
                    fact.validate().map_err(LeaseError::InvalidRequest)?;
                    Ok(ReconcileOutcome::AdvancedToReclaim(Box::new(fact)))
                } else {
                    Ok(ReconcileOutcome::StillUncertain)
                }
            }
            LeaseState::ReclaimPending => Ok(ReconcileOutcome::StillUncertain),
            other => Err(LeaseError::NotActive {
                lease_id: lease_id.as_str().into(),
                state: other.as_str().into(),
            }),
        }
    }

    /// Reclaim a reconciled lease. Verifies process/resource identity and the
    /// absence of a live effect first; then fences before any replacement.
    /// Never assumes a crashed holder is harmless.
    pub fn reclaim(
        &mut self,
        lease_id: &LeaseId,
        process_dead: bool,
        no_live_effect: bool,
        now_ms: u64,
    ) -> Result<LeaseLifecycleFact, LeaseError> {
        let record =
            self.leases
                .get_mut(lease_id.as_str())
                .ok_or_else(|| LeaseError::UnknownLease {
                    lease_id: lease_id.as_str().into(),
                })?;
        if record.lease.state != LeaseState::ReclaimPending {
            return Err(LeaseError::MustReconcileFirst {
                lease_id: lease_id.as_str().into(),
            });
        }
        if !process_dead || !no_live_effect {
            return Err(LeaseError::HolderMayBeLive {
                lease_id: lease_id.as_str().into(),
            });
        }
        record.lease.state = LeaseState::Reclaimed;
        let canonical = record.lease.resource.canonical_key();
        // Fence before any replacement: the record keeps its issued fence
        // and the fact carries the new one.
        let fence = Self::bump_fence(&mut self.fences, &canonical);
        let fact = LeaseLifecycleFact::new(
            lease_id.clone(),
            LEASE_EVENT_RECLAIMED,
            fence,
            record.lease.resource_generation,
            now_ms,
        )
        .with_detail("reclaimed after verified holder death and no live effect".to_string());
        fact.validate().map_err(LeaseError::InvalidRequest)?;
        Ok(fact)
    }

    // ── event-cursor rebuild (no second log) ──

    /// Apply one durable fact. Idempotent: replaying the same fact (same
    /// fence and target state) reports `Ok(false)`. A fact that would roll
    /// the fence backwards is refused. When `recovery` is set, active-ish
    /// facts land as `Uncertain` with a dead holder — a restart must never
    /// resurrect a live lease.
    pub fn apply_fact(
        &mut self,
        fact: &LeaseLifecycleFact,
        recovery: bool,
    ) -> Result<bool, LeaseError> {
        fact.validate().map_err(LeaseError::InvalidRequest)?;
        let target = fact.records_state().ok_or_else(|| {
            LeaseError::InvalidRequest(format!("fact kind `{}` records no state", fact.kind))
        })?;
        let id = fact.lease_id.as_str().to_string();
        if let Some(record) = self.leases.get_mut(&id) {
            let current_fence = self
                .fences
                .get(&record.lease.resource.canonical_key())
                .copied()
                .unwrap_or(0);
            if fact.fence < current_fence {
                return Err(LeaseError::StaleFact {
                    lease_id: id,
                    fence: fact.fence,
                    current: current_fence,
                });
            }
            if record.lease.state == target && record.lease.fence == fact.fence {
                return Ok(false);
            }
            if !record.lease.state.can_transition(target) {
                return Err(LeaseError::InvalidRequest(format!(
                    "illegal replay transition {} -> {}",
                    record.lease.state.as_str(),
                    target.as_str()
                )));
            }
            record.lease.state = target;
            record.lease.fence = fact.fence;
            if target != LeaseState::Active {
                record.holder_live = false;
            }
            if fact.fence > current_fence {
                self.fences
                    .insert(record.lease.resource.canonical_key(), fact.fence);
            }
            return Ok(true);
        }
        // Unknown lease: only an acquire with full context can rebuild it.
        if fact.kind != LEASE_EVENT_ACQUIRED {
            return Err(LeaseError::UnknownLease { lease_id: id });
        }
        let owner = fact.owner.clone().ok_or_else(|| {
            LeaseError::InvalidRequest(format!(
                "acquire fact for unknown lease `{id}` lacks owner context"
            ))
        })?;
        let resource = fact.resource.clone().ok_or_else(|| {
            LeaseError::InvalidRequest(format!(
                "acquire fact for unknown lease `{id}` lacks resource identity"
            ))
        })?;
        let access = fact.access.ok_or_else(|| {
            LeaseError::InvalidRequest(format!(
                "acquire fact for unknown lease `{id}` lacks access class"
            ))
        })?;
        owner.validate().map_err(LeaseError::InvalidRequest)?;
        resource.validate().map_err(LeaseError::InvalidRequest)?;
        let canonical = resource.canonical_key();
        self.generations
            .entry(canonical.clone())
            .or_insert(fact.generation);
        if fact.fence > self.fences.get(&canonical).copied().unwrap_or(0) {
            self.fences.insert(canonical.clone(), fact.fence);
        }
        // Leases are Work/Run-owned: the owner tuple comes from the fact.
        let lease = agentcowork_types::workbench::ResourceLease {
            lease_id: fact.lease_id.clone(),
            resource_generation: fact.generation,
            resource: TypedResourceKey {
                generation: fact.generation,
                ..resource
            },
            owner_work_id: owner.work_id.clone(),
            owner_run_id: owner.run_id.clone(),
            actor_binding_id: owner.binding_id.clone(),
            session_scope: owner.session_id.clone(),
            access,
            fence: fact.fence,
            scope_fingerprint: fact.scope_fingerprint.clone().unwrap_or_default(),
            state: if recovery && target == LeaseState::Active {
                LeaseState::Uncertain
            } else {
                target
            },
            issued_at_ms: fact.at_ms,
            expires_at_ms: fact.at_ms + 1,
            last_heartbeat_ms: fact.at_ms,
        };
        // A replayed lease is unexercisable until re-acquired: fresh bearer
        // nobody holds, dead holder. Fail-closed by construction.
        let session = lease.session_scope.as_str().to_string();
        self.session_attachments
            .entry(session)
            .or_default()
            .push(id.clone());
        self.leases.insert(
            id,
            LeaseRecord {
                lease,
                bearer: LeaseBearer::fresh_unheld(),
                holder_live: false,
                ttl_ms: 1,
            },
        );
        Ok(true)
    }

    /// Rebuild from canonical envelopes after `cursor`. Returns the new
    /// cursor (max sequence seen), with applied/skipped counts. Unknown
    /// event kinds and malformed payloads are skipped, never fabricated.
    pub fn rebuild_from_envelopes(
        &mut self,
        envelopes: &[EventEnvelope],
        cursor: u64,
        recovery: bool,
    ) -> RebuildReport {
        let mut report = RebuildReport {
            cursor,
            applied: 0,
            skipped: 0,
        };
        let mut ordered: Vec<&EventEnvelope> =
            envelopes.iter().filter(|e| e.seq > cursor).collect();
        ordered.sort_by_key(|e| e.seq);
        for env in ordered {
            report.cursor = report.cursor.max(env.seq);
            let Some(fact) = LeaseLifecycleFact::from_envelope(&env.kind, &env.payload) else {
                report.skipped += 1;
                continue;
            };
            match self.apply_fact(&fact, recovery) {
                Ok(true) => report.applied += 1,
                Ok(false) => report.applied += 1,
                Err(_) => report.skipped += 1,
            }
        }
        report
    }

    // ── session detach (delete Chat = detach the view) ──

    /// Deleting a Session detaches its projection and lookup scope.
    /// Surviving Work/Run leases and canonical records continue untouched;
    /// a later reattach rebuilds the projection from the event cursor.
    pub fn detach_session(&mut self, session_id: &SessionId) -> usize {
        self.session_attachments
            .remove(session_id.as_str())
            .map(|ids| ids.len())
            .unwrap_or(0)
    }

    // ── projection build (derived read model, never truth) ──

    /// Compose the per-Session projection from canonical summaries plus this
    /// coordinator's attachments, then validate the Session-keyed boundary.
    /// Cross-session records are refused, never borrowed.
    pub fn build_projection(
        &self,
        input: ProjectionInput,
    ) -> Result<SessionWorkbenchProjection, String> {
        let freshness = input.freshness.unwrap_or(ProjectionFreshness::Fresh);
        let session_id = input.session_id.clone();
        let projection = SessionWorkbenchProjection {
            schema_version: agentcowork_types::workbench::WORKBENCH_SCHEMA_VERSION,
            session_id,
            session_kind: input.session_kind,
            active_owner: input.active_owner,
            event_cursor: input.event_cursor,
            freshness,
            works: input.works,
            runs: input.runs,
            bindings: input.bindings,
            lenses: input.lenses,
            resource_refs: input.resource_refs,
            lease_attachments: self.attachments_for(&input.session_id),
            drafts: input.drafts,
            prompt_queue: input.prompt_queue,
            pending_questions: input.pending_questions,
            pending_reviews: input.pending_reviews,
            receipt_refs: input.receipt_refs,
        };
        projection.validate()?;
        Ok(projection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentcowork_types::workbench::{
        InteractionMode, LensAvailability, LensKind, ResourceAvailability, RunPhase,
    };
    use agentcowork_types::{ReceiptId, RunId, WorkId, WorkState};

    const NOW: u64 = 1_000_000;
    const TTL: u64 = 60_000;

    fn owner(session: &str, work: &str, run: &str, binding: &str) -> OwnerContext {
        OwnerContext::new(
            SessionId::new(session),
            WorkId::new(work),
            RunId::new(run),
            AgentBindingId::new(binding),
        )
    }

    fn doc_key(generation: u64) -> TypedResourceKey {
        TypedResourceKey::new(
            ResourceKind::OfficeDocument,
            "office:/docs/plan.docx",
            generation,
        )
    }

    fn acquire_edit(
        coord: &mut WorkRunLeaseCoordinator,
        session: &str,
        work: &str,
        run: &str,
        binding: &str,
        generation: u64,
    ) -> AcquiredLease {
        coord
            .acquire(
                owner(session, work, run, binding),
                doc_key(generation),
                LeaseAccess::Edit,
                "policy:v3",
                generation,
                NOW,
                TTL,
            )
            .expect("acquire must succeed")
    }

    // ── ADR-0008 §6 matrix: two Sessions view the same document ──

    #[test]
    fn two_sessions_may_share_observation_but_neither_gets_a_writer_handle() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let key = doc_key(7);
        let a = coord
            .acquire(
                owner("s-a", "w-a", "r-a", "b-a"),
                key.clone(),
                LeaseAccess::View,
                "policy:v3",
                7,
                NOW,
                TTL,
            )
            .unwrap();
        let b = coord
            .acquire(
                owner("s-b", "w-b", "r-b", "b-b"),
                key.clone(),
                LeaseAccess::Observe,
                "policy:v3",
                7,
                NOW,
                TTL,
            )
            .unwrap();
        // Compatible observers share the generation under one fence.
        assert_eq!(a.attachment.fence, b.attachment.fence);
        assert_eq!(a.attachment.generation, 7);

        // A later write makes the other view stale: takeover fences first.
        let taken = coord
            .takeover(
                owner("s-a", "w-a", "r-a", "b-a"),
                key.clone(),
                LeaseAccess::Edit,
                "policy:v3",
                7,
                NOW + 1,
                TTL,
            )
            .unwrap();
        assert_eq!(taken.facts.len(), 3); // two revocations + one acquire
        // The evicted observer's next exercise is stale, not silently moved.
        let err = coord
            .validate_exercise(
                &b.handle,
                &b.bearer,
                &AgentBindingId::new("b-b"),
                7,
                NOW + 2,
            )
            .expect_err("evicted observer must go stale");
        assert!(matches!(err, LeaseError::StaleLease { .. }), "got {err:?}");
        // The new writer exercises cleanly at the new fence.
        let att = coord
            .validate_exercise(
                &taken.handle,
                &taken.bearer,
                &AgentBindingId::new("b-a"),
                7,
                NOW + 2,
            )
            .unwrap();
        assert_eq!(att.fence, taken.attachment.fence);
    }

    // ── concurrent writer: exactly one holder, typed conflict ──

    #[test]
    fn concurrent_writer_gets_a_typed_conflict_never_a_second_lease() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let first = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        assert_eq!(coord.lease_count(), 1);
        let err = coord
            .acquire(
                owner("s-2", "w-2", "r-2", "b-2"),
                doc_key(3),
                LeaseAccess::Edit,
                "policy:v3",
                3,
                NOW,
                TTL,
            )
            .expect_err("second writer must conflict");
        match err {
            LeaseError::Conflict { holder, access, .. } => {
                assert_eq!(holder, first.attachment.lease_id.as_str());
                assert_eq!(access, "edit");
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        // No second lease was minted; no copied handle exists.
        assert_eq!(coord.lease_count(), 1);
    }

    // ── stale file generation refused before patch ──

    #[test]
    fn stale_generation_is_refused_never_retargeted() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        // The file moved on (new revision observed by the engine).
        coord.note_resource_generation(&doc_key(9), 4);
        // Exercise at the old digest is refused as stale.
        let err = coord
            .validate_exercise(
                &held.handle,
                &held.bearer,
                &AgentBindingId::new("b-1"),
                3,
                NOW + 5,
            )
            .expect_err("stale generation must refuse");
        assert!(
            matches!(err, LeaseError::StaleResource { .. }),
            "got {err:?}"
        );
        // A fresh lease cannot bypass the check either: admission sees it too.
        let err = coord
            .acquire(
                owner("s-1", "w-1", "r-1", "b-1"),
                doc_key(3),
                LeaseAccess::Edit,
                "policy:v3",
                3,
                NOW + 5,
                TTL,
            )
            .expect_err("admission at a stale generation must refuse");
        assert!(
            matches!(err, LeaseError::StaleResource { .. }),
            "got {err:?}"
        );
        // Re-observe at the new generation, release, and re-acquire cleanly.
        coord.release(&held.handle, &held.bearer, NOW + 6).unwrap();
        let fresh = coord
            .acquire(
                owner("s-1", "w-1", "r-1", "b-1"),
                doc_key(4),
                LeaseAccess::Edit,
                "policy:v3",
                4,
                NOW + 6,
                TTL,
            )
            .unwrap();
        assert_eq!(fresh.attachment.generation, 4);
    }

    // ── shared vs isolated browser profile ──

    #[test]
    fn isolated_browser_profile_cannot_be_inferred_or_attached() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let isolated = TypedResourceKey::new(ResourceKind::BrowserProfile, "profile:owner-w-1", 1);
        let held = coord
            .acquire(
                owner("s-1", "w-1", "r-1", "b-1"),
                isolated.clone(),
                LeaseAccess::Control,
                "policy:v3",
                1,
                NOW,
                TTL,
            )
            .unwrap();
        // Another Session guessing a different key gets its own isolated scope.
        let other = TypedResourceKey::new(ResourceKind::BrowserProfile, "profile:owner-w-2", 1);
        let other_held = coord
            .acquire(
                owner("s-2", "w-2", "r-2", "b-2"),
                other,
                LeaseAccess::Control,
                "policy:v3",
                1,
                NOW,
                TTL,
            )
            .unwrap();
        assert_ne!(
            held.attachment.resource_key,
            other_held.attachment.resource_key
        );
        // Attaching to the *same* key from another Session conflicts.
        let err = coord
            .acquire(
                owner("s-2", "w-2", "r-2", "b-2"),
                isolated,
                LeaseAccess::View,
                "policy:v3",
                1,
                NOW,
                TTL,
            )
            .expect_err("isolated profile must deny cross-session attach");
        assert!(matches!(err, LeaseError::Conflict { .. }), "got {err:?}");
    }

    #[test]
    fn shared_browser_tab_coexists_only_under_the_declared_shared_key() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let shared = TypedResourceKey::new(ResourceKind::BrowserTab, "profile:shared/tab:t", 5);
        for (session, work, binding) in [("s-1", "w-1", "b-1"), ("s-2", "w-2", "b-2")] {
            coord
                .acquire(
                    owner(session, work, "r-x", binding),
                    shared.clone(),
                    LeaseAccess::Observe,
                    "policy:shared-tab",
                    5,
                    NOW,
                    TTL,
                )
                .expect("shared observation must coexist");
        }
        // An action lease against the shared key is exclusive: conflict, not
        // a second handle.
        let err = coord
            .acquire(
                owner("s-1", "w-1", "r-1", "b-1"),
                shared,
                LeaseAccess::Action,
                "policy:shared-tab",
                5,
                NOW,
                TTL,
            )
            .expect_err("action against live observers must conflict");
        assert!(matches!(err, LeaseError::Conflict { .. }), "got {err:?}");
    }

    // ── physical Desktop target: observation vs action, birth mismatch ──

    #[test]
    fn desktop_action_is_exclusive_and_stale_after_target_replacement() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let target = TypedResourceKey::new(ResourceKind::DesktopTarget, "host:h/win:7/birth:11", 2);
        let observer = coord
            .acquire(
                owner("s-1", "w-1", "r-1", "b-1"),
                target.clone(),
                LeaseAccess::Observe,
                "policy:v3",
                2,
                NOW,
                TTL,
            )
            .unwrap();
        // Action while observed conflicts — even from the same Session.
        let err = coord
            .acquire(
                owner("s-1", "w-1", "r-1", "b-1"),
                target.clone(),
                LeaseAccess::Action,
                "policy:v3",
                2,
                NOW,
                TTL,
            )
            .expect_err("action must be exclusive");
        assert!(matches!(err, LeaseError::Conflict { .. }), "got {err:?}");
        // Takeover fences the observer out, then the actor takes the lease.
        let taken = coord
            .takeover(
                owner("s-1", "w-1", "r-1", "b-1"),
                target.clone(),
                LeaseAccess::Action,
                "policy:v3",
                2,
                NOW + 1,
                TTL,
            )
            .unwrap();
        drop(observer);
        // Target replaced (new process birth): the engine retires the old
        // key, so the action lease goes stale instead of retargeting.
        let retired = coord.retire_resource(
            &target,
            "desktop target replaced: process birth mismatch",
            NOW + 2,
        );
        assert_eq!(retired.len(), 1);
        let err = coord
            .validate_exercise(
                &taken.handle,
                &taken.bearer,
                &AgentBindingId::new("b-1"),
                2,
                NOW + 3,
            )
            .expect_err("old target generation must go stale");
        assert!(matches!(err, LeaseError::StaleLease { .. }), "got {err:?}");
    }

    // ── binding switch / park / reconnect ──

    #[test]
    fn binding_switch_keeps_work_run_leases_but_moves_the_actor() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-old", 3);
        coord
            .revalidate_actor_binding(
                &held.attachment.lease_id,
                &AgentBindingId::new("b-old"),
                &AgentBindingId::new("b-new"),
                3,
                NOW + 1,
            )
            .unwrap();
        // The old actor cannot act through the replacement.
        let err = coord
            .validate_exercise(
                &held.handle,
                &held.bearer,
                &AgentBindingId::new("b-old"),
                3,
                NOW + 2,
            )
            .expect_err("parked binding must not act");
        assert!(
            matches!(err, LeaseError::WrongBinding { .. }),
            "got {err:?}"
        );
        // The incoming binding exercises the same Work/Run lease.
        let att = coord
            .validate_exercise(
                &held.handle,
                &held.bearer,
                &AgentBindingId::new("b-new"),
                3,
                NOW + 2,
            )
            .unwrap();
        assert_eq!(att.lease_id, held.attachment.lease_id);
        // A switch that names the wrong outgoing binding is refused.
        let err = coord
            .revalidate_actor_binding(
                &held.attachment.lease_id,
                &AgentBindingId::new("b-impostor"),
                &AgentBindingId::new("b-newer"),
                3,
                NOW + 3,
            )
            .expect_err("impostor switch must fail");
        assert!(
            matches!(err, LeaseError::WrongBinding { .. }),
            "got {err:?}"
        );
    }

    // ── provider restart ──

    #[test]
    fn provider_restart_stales_old_handles_and_labels_continuation() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let handle_key =
            TypedResourceKey::new(ResourceKind::ProviderHandle, "binding:b-1/gen:1", 1);
        let held = coord
            .acquire(
                owner("s-1", "w-1", "r-1", "b-1"),
                handle_key,
                LeaseAccess::Action,
                "policy:v3",
                1,
                NOW,
                TTL,
            )
            .unwrap();
        // A file lease on the same Work is untouched by the restart.
        let file = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);

        let facts = coord.note_provider_restart(&AgentBindingId::new("b-1"), NOW + 1);
        assert_eq!(facts.len(), 1);
        assert_eq!(coord.provider_generation(&AgentBindingId::new("b-1")), 1);
        assert_eq!(
            coord.lease_state(&held.attachment.lease_id),
            Some(LeaseState::Uncertain)
        );
        // The old handle is stale; it is not smuggled across the restart.
        let err = coord
            .validate_exercise(
                &held.handle,
                &held.bearer,
                &AgentBindingId::new("b-1"),
                1,
                NOW + 2,
            )
            .expect_err("restarted handle must not exercise");
        assert!(
            matches!(
                err,
                LeaseError::NotActive { .. } | LeaseError::StaleLease { .. }
            ),
            "got {err:?}"
        );
        // Work identity and the file lease survive the restart.
        let att = coord
            .validate_exercise(
                &file.handle,
                &file.bearer,
                &AgentBindingId::new("b-1"),
                3,
                NOW + 2,
            )
            .unwrap();
        assert_eq!(att.lease_id, file.attachment.lease_id);
    }

    // ── user takeover fences before user-controlled action ──

    #[test]
    fn takeover_revokes_and_fences_before_the_new_holder_acts() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let agent = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-agent", 3);
        let fence_before = agent.attachment.fence;
        let taken = coord
            .takeover(
                owner("s-1", "w-1", "r-1", "b-user"),
                doc_key(3),
                LeaseAccess::Edit,
                "policy:takeover",
                3,
                NOW + 1,
                TTL,
            )
            .unwrap();
        assert!(taken.attachment.fence > fence_before);
        assert_eq!(
            coord.lease_state(&agent.attachment.lease_id),
            Some(LeaseState::Revoked)
        );
        let err = coord
            .validate_exercise(
                &agent.handle,
                &agent.bearer,
                &AgentBindingId::new("b-agent"),
                3,
                NOW + 2,
            )
            .expect_err("revoked agent must go stale");
        assert!(matches!(err, LeaseError::StaleLease { .. }), "got {err:?}");
    }

    // ── crash recovery: uncertain, reconcile, reclaim, fence ──

    #[test]
    fn crash_after_effect_before_receipt_stays_uncertain_until_reconciled() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        // Crash while the lease is held: uncertain, not free.
        let fact = coord
            .mark_holder_lost(&held.attachment.lease_id, NOW + 1)
            .unwrap();
        assert_eq!(fact.kind, LEASE_EVENT_UNCERTAIN);
        assert_eq!(
            coord.lease_state(&held.attachment.lease_id),
            Some(LeaseState::Uncertain)
        );
        // Reclaim without reconciliation is refused: the holder may be live.
        let err = coord
            .reclaim(&held.attachment.lease_id, true, true, NOW + 2)
            .expect_err("reclaim before reconcile must fail");
        assert!(
            matches!(err, LeaseError::MustReconcileFirst { .. }),
            "got {err:?}"
        );
        // An unknown outcome keeps the lease uncertain — never a verdict.
        let outcome = coord
            .reconcile(
                &held.attachment.lease_id,
                PostEffectOutcome::from_observation(None),
                NOW + 3,
            )
            .unwrap();
        assert_eq!(outcome, ReconcileOutcome::StillUncertain);
        assert_eq!(
            coord.lease_state(&held.attachment.lease_id),
            Some(LeaseState::Uncertain)
        );
        // A settled effect advances to reclaim-pending; reclaim still
        // verifies holder death and no live effect before fencing.
        let outcome = coord
            .reconcile(
                &held.attachment.lease_id,
                PostEffectOutcome::from_observation(Some(true)),
                NOW + 4,
            )
            .unwrap();
        assert!(matches!(outcome, ReconcileOutcome::AdvancedToReclaim(_)));
        let err = coord
            .reclaim(&held.attachment.lease_id, false, true, NOW + 5)
            .expect_err("unverified holder death must refuse reclaim");
        assert!(
            matches!(err, LeaseError::HolderMayBeLive { .. }),
            "got {err:?}"
        );
        let fence_before = coord.current_fence(&doc_key(3));
        let fact = coord
            .reclaim(&held.attachment.lease_id, true, true, NOW + 5)
            .unwrap();
        assert_eq!(fact.kind, LEASE_EVENT_RECLAIMED);
        assert!(fact.fence > fence_before);
        assert_eq!(
            coord.lease_state(&held.attachment.lease_id),
            Some(LeaseState::Reclaimed)
        );
    }

    #[test]
    fn expiry_is_a_backstop_not_authority_and_sweeps_to_a_durable_fact() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        let facts = coord.sweep_expired(NOW + TTL + 1);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].kind, LEASE_EVENT_EXPIRED);
        assert_eq!(
            coord.lease_state(&held.attachment.lease_id),
            Some(LeaseState::Expired)
        );
        // Renewal after expiry is refused; the owner re-acquires instead.
        let err = coord
            .renew(&held.handle, &held.bearer, 3, NOW + TTL + 2)
            .expect_err("renew after expiry must fail");
        assert!(matches!(err, LeaseError::Expired { .. }), "got {err:?}");
    }

    // ── session deletion with surviving Work ──

    #[test]
    fn session_deletion_detaches_the_view_while_work_leases_survive() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        assert_eq!(coord.attachments_for(&SessionId::new("s-1")).len(), 1);
        let detached = coord.detach_session(&SessionId::new("s-1"));
        assert_eq!(detached, 1);
        assert!(coord.attachments_for(&SessionId::new("s-1")).is_empty());
        // The Work-owned lease record survives the detach.
        assert_eq!(
            coord.lease_state(&held.attachment.lease_id),
            Some(LeaseState::Active)
        );
        // Reattachment rebuilds from the event cursor: the attachment view
        // returns once the session scope is re-registered via re-acquire or
        // replay; the lease itself was never deleted.
        assert_eq!(coord.lease_count(), 1);
    }

    // ── rebuild from the event cursor ──

    fn envelope(seq: u64, kind: &str, fact: &LeaseLifecycleFact) -> EventEnvelope {
        EventEnvelope {
            event_id: agentcowork_types::EventId::new(format!("ev-{seq}")),
            seq,
            work_id: WorkId::new("w-1"),
            run_id: Some(RunId::new("r-1")),
            step_id: None,
            kind: kind.into(),
            payload: fact.envelope_payload(),
            at_ms: NOW + seq,
            schema_version: agentcowork_types::CANONICAL_SCHEMA_VERSION,
        }
    }

    #[test]
    fn projection_rebuilds_from_the_event_cursor_without_a_second_log() {
        let mut source = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut source, "s-1", "w-1", "r-1", "b-1", 3);
        let renewed = source
            .renew(&held.handle, &held.bearer, 3, NOW + 10)
            .unwrap()
            .1;
        let released = source
            .release(&held.handle, &held.bearer, NOW + 20)
            .unwrap();
        let envelopes = vec![
            envelope(1, LEASE_EVENT_ACQUIRED, &held.fact),
            envelope(2, LEASE_EVENT_RENEWED, &renewed),
            envelope(3, LEASE_EVENT_RELEASED, &released),
            // Unknown kinds and gaps are skipped, never fabricated.
            EventEnvelope {
                event_id: agentcowork_types::EventId::new("ev-x"),
                seq: 4,
                work_id: WorkId::new("w-1"),
                run_id: None,
                step_id: None,
                kind: "chat.sent".into(),
                payload: serde_json::json!({}),
                at_ms: NOW + 4,
                schema_version: agentcowork_types::CANONICAL_SCHEMA_VERSION,
            },
        ];
        let mut rebuilt = WorkRunLeaseCoordinator::new();
        let report = rebuilt.rebuild_from_envelopes(&envelopes, 0, false);
        assert_eq!(report.applied, 3);
        assert_eq!(report.skipped, 1);
        assert_eq!(report.cursor, 4);
        assert_eq!(
            rebuilt.lease_state(&held.attachment.lease_id),
            Some(LeaseState::Released)
        );
        // Resume from the cursor applies only what follows it.
        let report = rebuilt.rebuild_from_envelopes(&envelopes, report.cursor, false);
        assert_eq!((report.applied, report.skipped), (0, 0));
        assert_eq!(report.cursor, 4);
    }

    #[test]
    fn recovery_rebuild_never_resurrects_a_live_lease() {
        let mut source = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut source, "s-1", "w-1", "r-1", "b-1", 3);
        let envelopes = vec![envelope(1, LEASE_EVENT_ACQUIRED, &held.fact)];
        let mut rebuilt = WorkRunLeaseCoordinator::new();
        let report = rebuilt.rebuild_from_envelopes(&envelopes, 0, true);
        assert_eq!(report.applied, 1);
        // Recovery lands active facts as uncertain with a dead holder.
        assert_eq!(
            rebuilt.lease_state(&held.attachment.lease_id),
            Some(LeaseState::Uncertain)
        );
        // The rebuilt lease carries a bearer nobody holds: there is no
        // handle to exercise it with, and replay never transfers one.
        assert!(rebuilt.handle_index.is_empty());
        // A backwards-fence replay is refused, never rolled back: revoke the
        // rebuilt lease (fence 1 -> 2), then replay the original acquire
        // fact (fence 1) and watch it bounce off the fence.
        rebuilt
            .revoke(&held.attachment.lease_id, "test fence advance", NOW + 5)
            .unwrap();
        let err = rebuilt
            .apply_fact(&held.fact, false)
            .expect_err("backwards fence must refuse");
        assert!(matches!(err, LeaseError::StaleFact { .. }), "got {err:?}");
    }

    // ── projection build: per-Session, honest about staleness ──

    #[test]
    fn projection_build_refuses_cross_session_records_and_reports_rebuilding() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let _held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        let input = ProjectionInput {
            session_id: SessionId::new("s-1"),
            session_kind: SessionKind::Interactive,
            active_owner: Some(owner("s-1", "w-1", "r-1", "b-1")),
            event_cursor: 3,
            freshness: None,
            works: vec![WorkRef {
                work_id: WorkId::new("w-1"),
                session_id: SessionId::new("s-1"),
                state: WorkState::Running,
            }],
            runs: vec![RunRef {
                run_id: RunId::new("r-1"),
                work_id: WorkId::new("w-1"),
                session_id: SessionId::new("s-1"),
                phase: RunPhase::Active,
                active_lease_ids: vec![
                    coord.attachments_for(&SessionId::new("s-1"))[0]
                        .lease_id
                        .clone(),
                ],
            }],
            bindings: vec![],
            lenses: vec![LensState {
                lens_id: "lens-1".into(),
                view_id: None,
                kind: LensKind::Office,
                open_order: 0,
                active: true,
                resource_ref_ids: vec![],
                resource_generation: 3,
                availability: LensAvailability::Available,
                interaction_mode: InteractionMode::ReadOnly,
                owner: owner("s-1", "w-1", "r-1", "b-1"),
            }],
            resource_refs: vec![WorkbenchResourceRef {
                session_id: SessionId::new("s-1"),
                key: doc_key(3),
                revision_digest: Some("sha256:abc".into()),
                availability: ResourceAvailability::Available,
            }],
            drafts: vec![],
            prompt_queue: vec![],
            pending_questions: vec![],
            pending_reviews: vec![],
            receipt_refs: vec![agentcowork_types::workbench::ReceiptRef {
                receipt_id: ReceiptId::new("rc-1"),
                work_id: WorkId::new("w-1"),
                session_id: SessionId::new("s-1"),
                uncertain: false,
            }],
        };
        let projection = coord.build_projection(input).unwrap();
        assert_eq!(projection.event_cursor, 3);
        assert_eq!(projection.freshness, ProjectionFreshness::Fresh);
        assert_eq!(projection.lease_attachments.len(), 1);

        // Switching Sessions restores only the new Session's records: a
        // build mixing s-2's lens into s-1's projection is refused.
        let mut mixed = ProjectionInput {
            session_id: SessionId::new("s-1"),
            session_kind: SessionKind::Interactive,
            active_owner: None,
            event_cursor: 0,
            freshness: Some(ProjectionFreshness::Rebuilding),
            ..Default::default()
        };
        mixed.lenses.push(LensState {
            lens_id: "lens-2".into(),
            view_id: None,
            kind: LensKind::Browser,
            open_order: 0,
            active: true,
            resource_ref_ids: vec![],
            resource_generation: 1,
            availability: LensAvailability::Available,
            interaction_mode: InteractionMode::Observe,
            owner: owner("s-2", "w-9", "r-9", "b-9"),
        });
        let err = coord
            .build_projection(mixed)
            .expect_err("mixed sessions must fail");
        assert!(err.contains("s-2"), "unexpected error: {err}");

        // A rebuilding projection is honest, not a fabricated clean state.
        let empty = ProjectionInput {
            session_id: SessionId::new("s-9"),
            session_kind: SessionKind::Automation,
            active_owner: None,
            event_cursor: 0,
            freshness: Some(ProjectionFreshness::Rebuilding),
            ..Default::default()
        };
        let projection = coord.build_projection(empty).unwrap();
        assert_eq!(projection.freshness, ProjectionFreshness::Rebuilding);
        assert!(projection.works.is_empty());
    }

    // ── custody and adversarial surfaces ──

    #[test]
    fn forged_or_copied_handles_prove_nothing_without_the_bearer() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        // A copied handle with a forged bearer is refused.
        let forged = LeaseBearer::generate();
        let err = coord
            .validate_exercise(
                &held.handle,
                &forged,
                &AgentBindingId::new("b-1"),
                3,
                NOW + 1,
            )
            .expect_err("forged bearer must fail");
        assert!(
            matches!(err, LeaseError::BearerMismatch { .. }),
            "got {err:?}"
        );
        // Another Session's binding cannot exercise even with… it has no
        // bearer at all, and the actor check would stop it regardless.
        let err = coord
            .validate_exercise(
                &held.handle,
                &held.bearer,
                &AgentBindingId::new("b-intruder"),
                3,
                NOW + 1,
            )
            .expect_err("cross-session binding must fail");
        assert!(
            matches!(err, LeaseError::WrongBinding { .. }),
            "got {err:?}"
        );
        // An unknown handle is refused, never resolved.
        let err = coord
            .validate_exercise(
                &LeaseHandle(9999),
                &held.bearer,
                &AgentBindingId::new("b-1"),
                3,
                NOW + 1,
            )
            .expect_err("unknown handle must fail");
        assert!(matches!(err, LeaseError::UnknownHandle), "got {err:?}");
    }

    #[test]
    fn no_bearer_material_reaches_projections_facts_errors_or_logs() {
        use agentcowork_types::workbench::projection_json_has_no_secrets;
        let mut coord = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        let revoked = coord
            .revoke(&held.attachment.lease_id, "takeover", NOW + 1)
            .unwrap();

        let attachment = serde_json::to_string(&held.attachment).unwrap();
        assert!(projection_json_has_no_secrets(&attachment), "{attachment}");
        let fact = serde_json::to_string(&held.fact.envelope_payload()).unwrap();
        assert!(projection_json_has_no_secrets(&fact), "{fact}");
        let revoked_fact = serde_json::to_string(&revoked.envelope_payload()).unwrap();
        assert!(
            projection_json_has_no_secrets(&revoked_fact),
            "{revoked_fact}"
        );

        // The bearer itself is redacted in Debug and absent from the
        // coordinator's Debug surface (counts only, no handles or bearers).
        let bearer_debug = format!("{:?}", held.bearer);
        assert_eq!(bearer_debug, "LeaseBearer([redacted])");
        let coord_debug = format!("{coord:?}");
        assert!(
            projection_json_has_no_secrets(&coord_debug),
            "{coord_debug}"
        );

        // Errors name ids and generations, never possession material.
        let err = LeaseError::BearerMismatch {
            lease_id: "lease-1".into(),
        };
        let rendered = err.to_string();
        assert!(projection_json_has_no_secrets(&rendered), "{rendered}");

        // A hostile caller cannot smuggle secrets through revoke detail.
        let err = coord
            .revoke(&held.attachment.lease_id, "bearer=deadbeef", NOW + 2)
            .expect_err("secret detail must be refused");
        assert!(matches!(err, LeaseError::InvalidRequest(_)), "got {err:?}");
    }

    #[test]
    fn logout_invalidates_scope_without_rewriting_identity() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        let facts = coord.invalidate_scope_fingerprint("policy:v3", NOW + 1);
        assert_eq!(facts.len(), 1);
        assert_eq!(
            coord.lease_state(&held.attachment.lease_id),
            Some(LeaseState::Revoked)
        );
        // Re-auth under a new scope creates a new lease generation; the old
        // holder stays fenced out.
        let err = coord
            .validate_exercise(
                &held.handle,
                &held.bearer,
                &AgentBindingId::new("b-1"),
                3,
                NOW + 2,
            )
            .expect_err("logged-out holder must stay fenced");
        assert!(matches!(err, LeaseError::StaleLease { .. }), "got {err:?}");
        let fresh = coord
            .acquire(
                owner("s-1", "w-1", "r-1", "b-1"),
                doc_key(3),
                LeaseAccess::Edit,
                "policy:v4",
                3,
                NOW + 2,
                TTL,
            )
            .unwrap();
        assert!(fresh.attachment.fence > held.attachment.fence);
    }

    #[test]
    fn renewal_stays_within_generation_and_never_repairs_staleness() {
        let mut coord = WorkRunLeaseCoordinator::new();
        let held = acquire_edit(&mut coord, "s-1", "w-1", "r-1", "b-1", 3);
        let (att, fact) = coord
            .renew(&held.handle, &held.bearer, 3, NOW + 10)
            .unwrap();
        assert_eq!(fact.kind, LEASE_EVENT_RENEWED);
        assert_eq!(att.fence, held.attachment.fence);
        // The file moved: renewal at the old generation is refused.
        coord.note_resource_generation(&doc_key(9), 4);
        let err = coord
            .renew(&held.handle, &held.bearer, 3, NOW + 11)
            .expect_err("renew at a stale generation must fail");
        assert!(
            matches!(err, LeaseError::StaleResource { .. }),
            "got {err:?}"
        );
    }
}
