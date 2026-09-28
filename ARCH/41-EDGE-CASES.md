# 41 — Edge Cases

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P4). The consolidated edge-case catalog. Each row names the scenario and the **required behavior**; the owning module doc carries detail. New edge cases discovered during review get a row here + a reference in the owning doc — no silent fixes.
> **P7 pass (2026-09-26):** coverage extended end-to-end — kernel (`10`), agent/model plane (`15`/`18`), runtime (`19`), code/search/comms (`26`–`28`), effect verification (`34`) and multi-agent coexistence get their own families; existing IDs and rows are unchanged.
> **P7 memory merge (2026-09-26):** +12 entries — memory-grade store integrity, mutation-vs-injection semantics and multi-agent memory boundaries (EDGE-157/158, EDGE-170–179).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Rule:** an edge case is resolved when the owning doc states the behavior; deferred cases carry an explicit trigger.

## A. Work & scheduling (`11`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-001 | Crash mid-run; queue lost | Queue is a projection — rebuild from log + checkpoints; runs resume per matrix. |
| EDGE-002 | App closed with detached work | Policy decision recorded (keep/suspend/stop); on start: rehydrate, adopt/monitor, reconcile strays. |
| EDGE-003 | Budget exhausted mid-run | Pause + surface; never silently kill or overrun (DEC-031). |
| EDGE-004 | Cancel during approval wait | `cancel_requested` honored at next boundary; approval records the cancellation. |
| EDGE-005 | Worker limits exceeded | Admission rejected with named limit; no silent trimming. |
| EDGE-006 | Two sessions write the same workspace | Leases (`25` §6): queue / rebase / ask — never silent overwrite. |
| EDGE-007 | Background load starves foreground | Priority + starvation guard (DEC-031). |
| EDGE-008 | Interrupt mid-tool | Cooperative stop at boundary; tool finishes or aborts per environment contract; state consistent. |
| EDGE-009 | Workspace moved/renamed between queue and resume | Resume re-resolves workspace identity before any write (`25` §5); an unresolvable root yields a typed `NotFound` + re-point guidance — queued work is never replayed against a guessed path (`11` §4, `10` §3). |

## B. Capability & providers (`13`/`14`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-010 | Provider restarted mid-session | Epoch bump ⇒ handles `InvalidState` ⇒ re-resolve (no silent retry on a changed runtime). |
| EDGE-011 | All providers for a capability unhealthy | `Unavailable` + retryable; `guidance` if setup could enable one. |
| EDGE-012 | Capability version deprecated mid-run | Run continues on its pinned version; new calls warn with migration path. |
| EDGE-013 | Ambiguous provider resolution | Deterministic tie-break + audit entry; never random. |
| EDGE-014 | MCP era mismatch | One retry in the other era; permanent mismatch ⇒ provider incompatible with reason (DEC-030). |
| EDGE-015 | MCP capability drift (tool removed) | Discovery diff updates descriptors; calls to removed capabilities fail typed. |
| EDGE-016 | Capability returns `guidance` mid-plan | Agent relays `next_action`; plan continues or pauses for user action — not a failure. |
| EDGE-017 | Effect retried after timeout | Idempotency key dedupe; no key ⇒ `needs_attention` (never blind re-fire). |
| EDGE-018 | Provider returns a schema-violating result | Adapter validates against the descriptor/contract schema before anything applies; malformed output is a typed failure, the provider is marked degraded, and nothing partial is applied; audited (`14` §2, §7; `13` §3; `10` §3). |
| EDGE-019 | Network loss mid-request / mid-stream | Watchdogs abort with a typed reason; retry only per the single-owner rule (request-start or pre-content; a user abort vetoes); post-content failures are handled at the turn level; idempotency keys prevent duplicate effects (`14` §7, `18` §4, DEC-034). |

## C. Context & memory (`16`/`17`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-020 | Overflow recovery loops | Bounded retries → escalate with reason; never infinity, never “new conversation”. |
| EDGE-021 | Pruned item needed later | `reconstructable` flag prevents loss; non-reconstructable items are never pruned. |
| EDGE-022 | Recall serves stale/conflicting memory | `superseded_by` filters superseded; staleness annotation + verify-against-live-state framing. |
| EDGE-023 | Secret enters memory pipeline | Rejected at validate + logged; if slipped: forget + suppression + corpus test. |
| EDGE-024 | Forgotten item re-extracted | Suppression digest blocks re-extraction **and import** (TEPA revocation); the digest is keyed, and scope wipes retain suppressions (`17` §7, DEC-039). |
| EDGE-025 | Pinned context exceeds ceiling | Ceiling enforced with warning; pins are bounded, not absolute. |
| EDGE-026 | Model switch changes tokenizer | Conservative re-estimate + feasibility recheck; degrade gracefully. |
| EDGE-027 | Zero relevant memory | Zero tokens in the **relevant** block (INV-22) — abstention is correct behavior; the always-on block is separately budgeted and exists only for pinned items. |
| EDGE-028 | Memory import malformed, oversized or hostile | Bounded read; schema + hash + secret + **suppression** scan re-run before landing; id/scope remapping follows the declared rules (project identity is not assumed portable); all-or-nothing transaction as `source='import'`; rejected items surfaced — never partially merged (`17` §4, §5, §13). |
| EDGE-029 | Concurrent extraction jobs contest one scope / store access | Job lease + debounce serialize extractors per scope; a **locked** store backpressures per call (bounded busy timeout + backoff) without disabling memory or failing a turn; a **corrupt** store disables memory for the session with a warning and a surfaced repair path (`17` §3, §5, §8; EDGE-172/173). |

## D. Trust & security (`12`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-030 | Policy engine error | **Fail closed** — DENY with reason; audited. |
| EDGE-031 | Vault unavailable | Credentialed call fails typed `Unavailable`; no plaintext fallback, ever. |
| EDGE-032 | Egress engine down | Outbound fails closed; offline capabilities unaffected. |
| EDGE-033 | Ticket replay / stale epoch | `InvalidState` + audit; re-authorize through the normal path. |
| EDGE-034 | Approval times out | Per-class policy (default deny); escalation field respected. |
| EDGE-035 | External agent requests out-of-scope path | Interception + deny + audit (not un-discovery). |
| EDGE-036 | Catastrophic op requested in Full Access | Irreducible gate still applies (`12` §3). |
| EDGE-037 | Policy scopes disagree | Innermost decision unless an outer ceiling forbids; conflicts logged. |
| EDGE-038 | Guard decision exceeds its deadline (slow/hung policy engine) | Bounded decision deadline; timeout ⇒ **DENY** with reason + audit (fail closed); an implicit allow is never returned (`12` §11, INV-04). |
| EDGE-039 | Concurrent executions race one bounded-use ticket | Ticket `uses` is decremented atomically at validation; the losing execution fails `InvalidState`; both outcomes audited — a ticket can never be spent twice (`12` §4, INV-03). |

## E. World & files (`21`/`25`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-040 | Windows `dev/ino` zeroed (current bug) | Code-phase fix to MFT identity; until fixed, identity degrades to guarded path-hash with warnings. |
| EDGE-041 | File deleted then recreated | Incarnation change ⇒ new identity; dedup/lease checks re-key. |
| EDGE-042 | Watcher overflow (`IN_Q_OVERFLOW` etc.) | Scoped rescan + freshness anomaly event — never a silent gap. |
| EDGE-043 | USN journal deleted/truncated | Cursor discarded; rescan that volume; epoch reset recorded. |
| EDGE-044 | Elevated UI unreachable (no UIAccess) | Mark regions unknown; guidance; no blind input. |
| EDGE-045 | UIA provider hangs | Per-call budget + worker isolation; partial tree marked partial. |
| EDGE-046 | Symlink/junction swap (TOCTOU) | Canonical path re-validated at use; deny on mismatch. |
| EDGE-047 | Cross-origin iframe blocks AX | Partial snapshot + skip; escalate to vision only if critical. |
| EDGE-048 | CAPTCHA / bot check | Surface to the user; **no evasion tooling** (DEC-016). |
| EDGE-049 | Permission denied during scan | Scoped skip + surfaced count (metadata-mode honesty). |

## F. Domains (`22`–`24`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-050 | Crash mid-Office-write | Staging → fsync → atomic swap → op-log replay; original intact until swap. |
| EDGE-051 | Engine cannot represent an edit (fidelity) | Typed `guidance` naming the limitation; never a silent lossy path. |
| EDGE-052 | Attach to user browser fails | Fall back to managed Chromium with a surfaced note. |
| EDGE-053 | PDF redact “completes” but text remains | Required: post-op extraction check proving removal; current code annotates (code-phase P0 fix). |
| EDGE-054 | Vision mislocates an element | Verify after action; bounded retries; `needs_attention` on repeat failure. |
| EDGE-055 | Vision model unavailable when vision scheduled | Degrade with recorded gap; dangerous classes require human confirmation instead. |
| EDGE-056 | Capture/UI read meets protected fields (passwords) | `IsPassword`/protected fields are excluded or masked before any output; screenshots remain capture-consent-gated; protected values never enter context (`21` §5, `24` §4, §7). |
| EDGE-057 | User cancels mid-domain operation (Office batch / browser action) | Cooperative stop at the next boundary; Office batches stay all-or-nothing; browser actions settle or abort without interleaved input; partial effects are receipted/verified, never hidden (`11` §6, `22` §4, `23` §4). |
| EDGE-058 | User and agent drive the same browser tab (takeover) | User takeover suspends agent input and the agent yields; actions serialize on the shared surface — no interleaved typing/clicking; resume is explicit (`23` §2). |
| EDGE-059 | External render/convert engine absent (LibreOffice-class) | Typed `guidance` naming the missing engine/format limit; declared format limits surface instead of a silent lossy path; the native engine stays primary (`22` §2, §7). |

## G. Workflow (`20`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-060 | Wake missed (OS sleep/reboot) | Misfire policy: skip+record default; latest-missed optional; bounded grace. |
| EDGE-061 | Crash mid-node | Resume matrix: reuse settled; idempotent retry same key; keyless ⇒ `needs_attention`. |
| EDGE-062 | Keyless side effect interrupted | `needs_attention` with evidence; never fabricated completion. |
| EDGE-063 | Version upgrade while run in flight | Impossible — runs pinned; explicit audited upgrade only. |
| EDGE-064 | Trigger storm | Per-workflow concurrency (default Skip) + queue depth backpressure. |
| EDGE-065 | DST/clock jump | IANA zone resolution; `wake_at` is “not before”; late wakes governed by misfire. |
| EDGE-066 | Parent cancels with nested runs | Parent-close policy (terminate/cancel/abandon) explicit per edge. |
| EDGE-067 | Approval rejected | Condition edge (reject branch) or run fails with reason; recorded. |
| EDGE-068 | Malformed or unsupported workflow definition | Publish gate rejects with node-level typed errors (IR schema + policy + capability census); drafts never trigger or execute (`20` §2, §9). |
| EDGE-069 | Occurrence claim race (duplicate app instance / two loops) | Unique idempotency key + one-transaction claim ⇒ exactly-once run; the losing claim no-ops and records; leases/heartbeats resolve stale owners (`20` §4, DEC-033). |

## H. Events & channels (`30`/`32`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-070 | Poison event | Quarantine + reconciliation entry; stream continues. |
| EDGE-071 | Consumer lag | Lag marker; pull-based catch-up from store. |
| EDGE-072 | Duplicate delivery | Consumers idempotent by event id. |
| EDGE-073 | Channel disconnects mid-approval | Request waits durably; re-surfaces on attach. |
| EDGE-074 | Send outcome uncertain | `needs_attention` + receipt of the attempt; **never duplicate sends**. |
| EDGE-075 | Surface crash | Isolated; Core and other surfaces unaffected. |
| EDGE-076 | Event storm saturates the bus | Bounded subscriber queues with lag markers and pull-based catch-up; producers backpressure; memory never grows unbounded (`30` §4, `11` §3). |
| EDGE-077 | External agent disconnects mid-run (ACP/API drop) | Work continues as durable Work; the gateway session is held; re-attach replays the filtered stream from the last ack — no orphaned internal state (`32` §3, §7; `30` §4; `11` §4). |
| EDGE-078 | Two surfaces act on one session concurrently | One foreground lane per session; surfaces are projections and cannot fork state; ordering derives from the sequence log — no dual-writer (`11` §3, `32` §1, INV-06). |
| EDGE-079 | External-agent event vocabulary changed underneath a client | The projection is a declared stable subset with deprecation windows; clients get a typed version error naming the supported window — never a silently missing stream (`30` §3, §6; `32` §8). |

## I. Extensions (`31`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-090 | Plugin crash loop | Auto-disable + audit; Core unaffected. |
| EDGE-091 | Skill’s capability requirement missing | `guidance` mode; activation never grants permissions. |
| EDGE-092 | Plugin vs Core contract skew | Rejected with compat-window explanation. |
| EDGE-093 | Skill instructions contain injection | Treated as untrusted content: review gate + provenance + injection hygiene. |
| EDGE-094 | Hostile skill/plugin package (traversal paths, oversized archive) | Extraction is bounded and confined to declared roots (pathfloor); manifest + provenance pass the review gate; rejection is typed — nothing lands outside the package (`31` §5, `25` §7). |
| EDGE-095 | A skill's required capability is revoked/uninstalled mid-run | Requirements are rechecked at invocation; affected calls fail typed (`NotFound`/`guidance`); activation neither grants nor retains permissions (`31` §3, `13` §9). |
| EDGE-096 | Plugin update lands while its code executes | In-flight execution finishes on its loaded version; the new version applies at next activation; breaking skew is rejected with the compat window — no hot-swap mid-call (`31` §5). |

## J. Data integrity (`29`/`30`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-100 | GC meets receipt pin | Receipt-pinned versions are never collected. |
| EDGE-101 | Artifact location moved/deleted | `unresolved` + digest retained; receipts keep the digest; re-link path. |
| EDGE-102 | Audit chain gap detected | Report + reconcile; never silent. |
| EDGE-103 | Import collides with existing hashes | Import validated (hash re-check + secret scan); conflicts surfaced, not auto-merged. |
| EDGE-104 | Disk full / write failure during artifact or receipt write | Typed failure; no partial version or receipt becomes visible; a mandatory-receipt effect cannot complete (INV-07) and pauses with reason (`29` §3, §8). |
| EDGE-105 | Artifact gateway request crosses a workspace/project boundary | v1 is workspace-scoped: cross-workspace access is denied typed with no path leakage (explicit export only); gateway permissions are checked per verb (`29` §5, INV-11). |
| EDGE-106 | Event retention prunes a range a projection still needs | Critical evidence lives in receipts/audit (not pruned with events); projections rebuild from store + checkpoints; a real gap is reported, never silently answered (`30` §4, §7; `11` §4). |

## K. Kernel (`10`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-110 | Config layer malformed / carries unknown keys or secrets | Fail closed for that layer only; fall back to the previous valid layer with a surfaced warning + audit event; unknown keys yield migration notes; secrets in config are rejected (vault refs only) — never silently accepted (`10` §4, §9; INV-02). |
| EDGE-111 | Store migration fails part-way at boot | The dependent feature is blocked cleanly; no read or write runs against a half-migrated store; the exact migration + error is reported (`10` §6, §9). |
| EDGE-112 | Wall clock jumps backward/forward mid-run | Kernel **timers/schedules**: monotonic durations and timers are unaffected; schedules re-evaluate conservatively from persisted times; no wall-clock delta drives kernel expiry or ordering (`10` §5, §9). Memory recency/TTL/lease skew is governed separately by its clamp and anchor policy (`17` §7, EDGE-177). |
| EDGE-113 | Boundary payload is invalid UTF-8 / non-canonical JSON / float-unsafe number | Typed boundary error; no lossy coercion; ids and sizes stay integer-safe; canonical JSON on every emit (`10` §3, §6). |
| EDGE-114 | Duplicate uuidv7 id minted | Treated as `Internal` with the collision recorded — single-writer minting makes this a bug, never a silent merge or reuse (`10` §2, §9). |

## L. Agent runtime & model plane (`15`/`18`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-120 | Agent reaches “done” with success conditions unmet | The completion contract is the only success exit; verification runs before done; otherwise the loop continues or blocks with a typed reason — reading/editing a file or finishing one subtask never ends the run (`15` §4, DEC-022, INV-19). |
| EDGE-121 | Input arrives mid-step / a wake is delivered twice | Admission boundaries (`next-turn`/`next-step`) hold injected inputs until a wake; the durable inbox projection orders and dedupes; inputs never interleave mid-step (`15` §3, `11` §4). |
| EDGE-122 | Loop makes no progress across N steps | Stuck detector fires → escalation through the approval/question primitive; bounded retries — never an unbounded spin (`15` §8, `11` §9). |
| EDGE-123 | Failover crosses providers with in-flight provider-bound state | The fallback chain is declared at selection; signed reasoning blocks, cache breakpoints and in-flight tool-call ids are never replayed onto another provider; the step restarts cleanly or escalates — invalidation rules remain explicit open work (`18` §3, §4; `OQ-MODEL-01`). |
| EDGE-124 | Provider stream emits unknown or malformed events | The adapter maps into the typed union; unknown events are dropped with a diagnostic and malformed known events fail typed — consumers never crash and never branch on provider id (`18` §4, DEC-034). |
| EDGE-125 | No model satisfies a required capability (vision/tools/window) | `GuidanceRequired` / `RequiresUserAction` naming the missing requirement — never a silent capability downgrade (`18` §3, §8). |

## M. Runtime & environments (`19`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-130 | Process exceeds declared environment limits | Enforced limits throttle/stop the process with a typed reason; work is marked interrupted and resumes per idempotency; no silent unbounded run (`19` §2, §9). |
| EDGE-131 | OS crash/reboot leaves process trees orphaned | Start-time rehydration adopts registered detached processes and reconciles strays (re-attach or reap); every decision audited — no unregistered orphans (`19` §7, §9; DEC-031). |
| EDGE-132 | Agent step and user terminal touch the same target | User terminals stay user-owned (the agent never drives them); agent terminals are policy-scoped; closing a held terminal cancels the dependent step and records it (`19` §3, `11` §6). |
| EDGE-133 | App updated/moved while detached work runs | Rehydration re-resolves the executable; a binary/version change is adopted/monitored or reconciled per policy with the decision recorded — never silently mismatched (`19` §7). |
| EDGE-134 | Sandbox backend becomes unavailable mid-session | New spawns follow the fallback ladder → deny with reason (explicit audited unconfined mode only by policy); running confined processes stop or continue under their recorded confinement with the gap surfaced (`19` §4, §9). |

## N. Code, search & comms (`26`/`27`/`28`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-140 | Indexer meets huge/generated/malformed sources | Declared size caps + ignore rules; parser failure is per-file and bounded; partial maps ship with freshness labels; the indexer never crashes the workspace (`26` §2, §8). |
| EDGE-141 | Build/test killed by timeout or cancellation | Bounded output persists to an artifact; the result is typed interrupted/failed; the process tree is cleaned up under `19` — no orphaned test runners (`26` §7, `19` §3). |
| EDGE-142 | Worktree merge conflict / parent workspace advanced | Merge is an explicit review step (diff + tests + receipt); conflicts resolve by queue/rebase/ask — never silent overwrite or force (`26` §6, `25` §6, DEC-029). |
| EDGE-143 | Search serves stale or partial source state | Freshness flag + degraded notice on results; scope and sensitivity filters apply at query time, before scoring; abstention is a valid outcome (`27` §5, §7). |
| EDGE-144 | `web.fetch` targets an internal host / cross-host redirect | SSRF floor (no localhost/no-dot/private/link-local/metadata; resolve-then-check); cross-host redirects are surfaced, never silently followed; Guard deny is typed (`28` §3, DEC-037, INV-05). |
| EDGE-145 | Message attachment is huge/hostile/unscannable | Attachments move through the artifact gateway under connector permissions; size/type caps + scan rules gate them (`OQ-COMMS-03`); until that policy lands uploads stay user-gated — never auto-opened (`28` §5, `29` §5). |

## O. Multi-agent coexistence (`15`/`32`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-150 | Integration would replace or duplicate an external agent's native tools/session/auth | **Native-first**: the bound agent keeps its own loop, tools, sessions, model auth and permissions; Core augments through the projection only where the native path is absent or worse; no second tool registry and no forced pivot onto Core tools (`15` §2, DEC-025, `32` §3). |
| EDGE-151 | External agent private history vs Core-owned state | Agent/provider-private transcripts and settings stay private and outside Core; Core projects only its own session/event records; memory extraction never reads histories we do not own (`17` §5, `30` §6, INV-11). |
| EDGE-152 | Agent-local config/permission disagrees with Core policy | Core's scope chain is the outer ceiling — external agent settings may narrow but never widen it; the Trust decision stands and conflicts are logged (`12` §3, §8; DEC-028). |
| EDGE-153 | Several agents share one workspace/project | One child session per subagent; overlapping writers hold workspace leases (queue/rebase/ask); tool/persona/session scope stays per binding (`15` §7, `25` §6, DEC-029). |
| EDGE-154 | Cancelled/closed child returns a late completion | Wake-suppression gate: a cancelled child never wakes the parent and never re-buffers after teardown; receipt delivery is at-most-once per parent incarnation; queued spawns are swept (`15` §7, DEC-036). |
| EDGE-155 | External agent reaches outside its projection (path/tool/store) | Interception, not un-discovery: deny + audit at the Core boundary; the projection never widens implicitly; the session is flagged (`12` §8, INV-11). |
| EDGE-156 | External agent process crashes/restarts mid-run | Work state stays durable (log + checkpoints); the binding re-establishes through the gateway; effects re-authorize through tickets/idempotency with no duplicate application (`11` §4, `32` §3, `12` §4). |
| EDGE-157 | External agent attempts a memory write (or Core reaches a native memory/config dir) | v1 exposure is **recall-only**: no write path is projected, and Core never writes or mutates an external agent's native memory/config/session store — deny + audit (`17` §4–§5, DEC-025/043, EDGE-150/151). |
| EDGE-158 | Subagent completes its work | The parent context receives a **receipt, not a transcript**; receipt content is untrusted data (scanned, no-authority header); child-session harvesting follows the declared policy — Core-owned only, keyed to the child session/task with parent linkage, never auto-promoted (`15` §7, `17` §5, DEC-029/036/043). |

## P. Effect verification (`34`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-160 | Verifier unavailable or crashes at receipt time | The effect is marked **unverified** in the receipt with the gap recorded; dangerous classes block or require human confirmation — never a silent pass (`34` §7, INV-19). |
| EDGE-161 | A false pass is discovered after the fact | Sampled audits + drift metrics; the miss becomes a new check/regression; prior receipts are not retro-trusted (`34` §7). |
| EDGE-162 | Reconciliation shows a mismatch needing repair | Repair is a new work item/operation with its own ticket and receipt; verification never mutates the effect in place (`34` §6, `29` §3). |
| EDGE-163 | Verify–repair cycle makes no progress | Bounded attempts, then escalation / `needs_attention` with evidence — never an infinite loop (`34` §7). |

## Q. Memory store integrity (`17`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-170 | Forget meets supersede pointers and import | No dangling `superseded_by` and no resurrection: deleting a superseding item collapses its chain and never fails; import re-checks suppression and cannot resurrect a forgotten hash; scope-wipe suppression semantics are explicit (suppressions retained; content stays un-extractable) (`17` §3/§7, DEC-039). |
| EDGE-171 | Mid-session forget/edit/pin/disable vs the frozen injection block | The next turn reflects it: forget/edit/supersede/pin/unpin/disable/wipe invalidate the always-on block, the cache-bust is accepted, and a deleted item is never injected again (`16` §5/§6, `17` §6). |
| EDGE-172 | Memory DB corrupt (vs merely locked) | Corruption is not treated as a transient lock: memory disables for the session with a warning, the store is quarantined, and a repair path (export readable rows → recreate → re-import validated) is surfaced + audited; chat is unaffected (`17` §8). |
| EDGE-173 | Two processes/surfaces write memory (desktop + CLI/detached) | Single-writer ownership (lease + heartbeat; stale-lease reclaim audited) or serialized WAL with bounded `busy_timeout`/backoff: no lost writes, no turn failure (`17` §3/§8, `11` §3, `32` §6). |
| EDGE-174 | FTS desync detected | `integrity-check`/row-count parity detects; `rebuild` restores the external-content index; verification runs **after** rebuild; an unrestored index is surfaced (`17` §3/§8). |
| EDGE-175 | Oversized extraction output or entry | The per-item cap is enforced at validate; oversize is rejected with no partial write; one giant item can never consume the injection budget (`17` §5/§6). |
| EDGE-176 | Hostile content harvested into memory | The item records a provenance trust tier; instruction-shaped or scope-widening candidates are rejected or downgraded without authority; recall text is quoted data that grants no authority (`17` §5/§6, DEC-036/037). |
| EDGE-177 | Clock jump vs memory recency/TTL/leases | Ordering and expiry never invert: recency deltas clamp against non-monotonic observations, TTL uses the persisted anchor, leases fail safe, and skew events are recorded — aligned with `EDGE-112` (`17` §3/§6/§7). |
| EDGE-178 | Project re-key (move/clone/rename/worktree/path reuse) | Scope follows the stable project identity (`DM-024`), not the path: explicit re-key/mapping rules apply, worktrees share the parent identity, and no recall path returns an out-of-set identity (`17` §2.1, `25` §5, DEC-040). |
| EDGE-179 | Source artifact deleted | Provenance renders “source unavailable”; injection never dereferences the ref; the item stays usable (`17` §3/§6, `29`, DEC-032). |

## R. Mission and ecosystem amendment (`35`–`38`, `46`)

| ID | Scenario | Required behavior |
|---|---|---|
| EDGE-180 | Agent session disappears while Mission active | Rebuild from Mission events/checkpoints; replace session without losing contract/node. |
| EDGE-181 | Two planners patch same graph version | Compare-and-swap rejects stale patch; rebase or review. |
| EDGE-182 | Work fails after external side effect may have happened | Reconcile observed state; no blind retry or false completion. |
| EDGE-183 | User changes a requirement during parallel Work | Version contract; impact and steer only affected branches. |
| EDGE-184 | Completed artifact's input changes | Mark dependent evidence/artifacts stale; selective revalidation. |
| EDGE-185 | 90-day resume with changed repository/API/credentials | Fingerprint and reconcile before mutation; scoped blocker when unavailable. |
| EDGE-186 | Agent repeats same failed strategy while heartbeat remains healthy | No-progress bound escalates; heartbeat does not count as progress. |
| EDGE-187 | Discovered agent has native MCP/skills/plugins | Inventory read-only; preserve config; label native policy owner. |
| EDGE-188 | Host MCP exists globally but Work has no grant | Not mounted/started for that Work; catalog metadata only. |
| EDGE-189 | Two MCPs expose the same tool name | Namespace by owner/provider; no silent shadow. |
| EDGE-190 | Channel-B tool call has placeholder/missing agent identity | Refuse until real binding/session/work/grant is supplied. |
| EDGE-191 | Agent reports native operation as done | Show agent-reported/observed provenance; no Core ticket claim. |
| EDGE-192 | Child agent lacks requested browser or isolation capability | Typed refusal; parent may choose another worker or replan. |
| EDGE-193 | External workflow callback beats persisted wait state | Persist signal and reconcile after pause; no stranded waitpoint or duplicate continuation. |
| EDGE-194 | Local machine shuts down during detached Mission | Durable pause; cloud continues only if explicitly configured and assigned. |
| EDGE-195 | Captured workflow contains a secret or unstable UI step | Redact/reject unsafe candidate; require review and robust fallback. |

## Extension rule

New edge cases are appended with the next free ID in their family, with: scenario · required behavior · owning doc. A case that changes an authority doc escalates to a `DEC`.
