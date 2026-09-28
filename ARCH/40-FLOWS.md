# 40 — Flows

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P4). The end-to-end sequence catalog. Every flow declares actors, steps, terminal states and failure branches; edge cases are detailed in `41-EDGE-CASES.md`.
> **P7 pass (2026-09-26):** line-checked; cross-references verified.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Rule:** a flow is authoritative only if it is consistent with the module docs it touches; conflicts escalate to a `DEC`.

---

### FLOW-01 — Interactive turn
**Actors:** user · surface (`32`) · Work (`11`) · agent plane (`15`) · capability plane (`13`).
**Steps:** work created (`session_turn`, foreground lane) → admission (turn boundary) → context assembly (`16`) → model step → tool calls scheduled (parallel/sequential) → results observed → continuation decision → completion contract satisfied → response streamed → run completed + events.
**Terminal:** `completed` · `failed` · `cancelled` · `waiting` (approval/question).
**Failure branches:** model error → bounded retry → surface reason; tool failure → recovery pipeline; user interrupt → step-boundary stop, session kept.

### FLOW-02 — Governed capability execution
**Actors:** agent/UI · `13` · `12` · `14` · `19` · `34` · `29` · `30`.
**Steps:** resolve capability → handle (epoch-checked) → Guard (`ALLOW/ASK/DENY`) → ticket (scoped, time-boxed) → execute via provider/environment → effect → verify (risk-proportional) → receipt recorded → event published → result returned.
**Terminal:** `completed` (+receipt) · `guidance` · `requires_user_action` · `failed`.
**Failure branches:** stale handle → re-resolve; provider crash → epoch bump + failover; ticket expired → re-authorize; verification fail → repair path (`15`) or `needs_attention`.

### FLOW-03 — Delegation (subagent)
**Actors:** main agent · `15` delegation · `11` · `19`/`26` (worktree) · child agent.
**Steps:** `delegate({worker, task, context_refs, limits})` → policy resolves worker (`DM-015`) → child session created (own context; `fork_context` optional) → workspace scope assigned (shared / worktree / sandbox) → child runs → **worker receipt** returned → aggregator merges → parent continues.
**Terminal:** `completed` · `failed` (receipt with blockers) · `cancelled` (parent cancel propagates).
**Failure branches:** child stuck → watchdog + stuck detector; write conflict → lease queue/rebase/ask; depth/parallel limits → admission rejected with reason.

### FLOW-04 — Workflow with agent node + approval
**Actors:** trigger source · Workflow Engine (`20`) · capability plane · agent plane (`15`) · approval (`12`) · user.
**Steps:** trigger fires → occurrence materialized + claimed (pinned version) → deterministic nodes execute via capabilities → `agent` node invokes the bound engine with node task + context refs → result → `approval` node → run `awaiting_approval` (durable) → user decides → resume → remaining nodes → outputs/artifacts → run completed + receipt.
**Terminal:** `completed` · `failed` · `cancelled` · `expired` (approval timeout per class).
**Failure branches:** agent node blocks → per-node retry policy; approval rejected → condition edge (reject branch) or run fails with reason; crash → resume matrix (`20` §4).

### FLOW-05 — Scheduled occurrence lifecycle
**Actors:** scheduler loop (`20` §4) · journal · trigger row.
**Steps:** `next_due_at` stored → current fenced trigger owner materializes occurrence (idempotency key) → atomic claim admits one logical run → run pinned → Work dispatch and effect reconciliation → compute next wake.
**Terminal:** run terminal state; trigger continues.
**Failure branches:** app closed at due time without local service/accepted cloud owner → misfire policy (skip+record default; latest-missed optional); trigger-owner lease expiry → fence old owner and reconcile journal before successor; effect acknowledgement lost → verify/idempotency or `needs_attention`, never blind re-fire (`20` §4, DEC-057).

### FLOW-06 — Background/detached work across app lifecycle
**Actors:** `11` lanes · `19` runtime · user.
**Steps:** background work admitted without blocking UI → detached run registered → app close records whether a healthy local service or accepted remote executor owns it; otherwise pause → app starts → registry reconciles liveness/environment → eligible runs resume per idempotency rules.
**Terminal:** per run; detached may outlive surfaces.
**Failure branches:** orphan process found → reap or re-attach (audited); stale lease → requeue; keyless side effect interrupted → `needs_attention`.

### FLOW-07 — Context overflow recovery
**Actors:** agent context control (`16`) · model router (`18`).
**Steps:** pre-turn feasibility check → over budget? prune cold items → structured checkpoint → compact (projection boundary) → retry **same step** → if model refuses again → escalate.
**Terminal:** turn continues; never “start a new conversation”.
**Failure branches:** bounded retries → block with surfaced reason; checkpoint missing → rebuild from work/events/artifacts.

### FLOW-08 — Memory write (extraction)
**Actors:** `17` pipeline · model router · trust.
**Steps:** settled boundary (idle/task end) → signal gate (cheap) → harvest (bounded) → one extract call (`ADD|SUPERSEDE|NONE`, ≤3 items) → deterministic validate (hash/secret/scope) → persist in one txn + FTS → audit event.
**Terminal:** items stored or nothing.
**Failure branches:** extract call fails → job retry/backoff; **turn unaffected**; secret detected → reject + log; suppression match → drop.

### FLOW-09 — Memory recall into context
**Actors:** Context Controller (`15`/`16`) · `17`.
**Steps:** recall(query, scopes, budget; caller scopes narrow the actor-derived ceiling) → filter (current, unexpired, sensitivity) → FTS5 BM25 candidates → relevance = −bm25 (higher = better; deterministic tie-break) → budget-fit (whole-item drop) → candidates returned → Controller decides inclusion → injection (non-touching; staleness-annotated).
**Terminal:** items injected or **abstention** (zero hits ⇒ zero tokens).
**Failure branches:** recall failure → proceed without memory; DB locked → bounded backoff, memory degrades or defers without disabling memory or failing the turn; corrupt store → memory disabled for the session with a surfaced warning (`17` §8, EDGE-172/173).

### FLOW-10 — External agent onboarding
**Actors:** external agent · Agent Gateway (`32`) · `12`/`13`/`16`/`29`/`30`.
**Steps:** connect (ACP/A2A/API) → identity + session established → capability projection computed (Installed × Available × Allowed × Relevant) → context projection built → workspace projection declared → artifact gateway + filtered event stream attached → agent operates inside its projection.
**Terminal:** session lifecycle managed; disconnect clean.
**Failure branches:** auth failure → denied + audit; projection leak attempt → denied + flag; version mismatch → typed error.

### FLOW-11 — External agent capability call
**Actors:** external agent · gateway · `13`/`12`/`14`.
**Steps:** agent requests a projected capability → gateway maps to internal invocation → Guard → ticket → execute → event filtered back to the agent.
**Terminal:** result or typed error (`NotFound` for unprojected capabilities).
**Failure branches:** out-of-scope path → interception (`12` §8); rate exceeded → backoff.

### FLOW-12 — MCP provider lifecycle
**Actors:** `14` adapter · `19` · server process.
**Steps:** discover era (stdio `server/discover` probe / HTTP 400-classify) → connect (modern first, legacy fallback) → enumerate capabilities → map to descriptors → execute via broker → health monitored.
**Terminal:** provider registered; healthy/degraded.
**Failure branches:** era mismatch → one retry other era → incompatible + reason; crash → epoch bump, handles invalidated, failover; schema drift → descriptor diff + event.

### FLOW-13 — Browser task
**Actors:** agent · `23` · `12`.
**Steps:** launch/park managed Chromium → snapshot (compact AX/DOM + refs) → act (trusted input) → verify (structured assertion/diff) → repeat; refs invalidated on navigation.
**Terminal:** task complete; browser parked/closed per scope.
**Failure branches:** stale ref → re-resolve; iframe blocked → partial + escalate; CAPTCHA → surface to user (no evasion); crash → environment restart + re-establish targets.

### FLOW-14 — Computer-use fallback ladder
**Actors:** agent · `24` · `21` observations · `18` vision models.
**Steps:** native API? → structured UI (UIA/AX/AT-SPI, epoch-scoped handles) → DOM/AX (browser) → CLI/MCP → OCR (local, empty trees) → vision (capped capture → act → observe) → raw input (gated).
**Terminal:** action completed + observed.
**Failure branches:** provider hang → per-call budget + isolation; ambiguous element → reject/re-read; elevation blocked → mark unknown + guidance; vision misfire → verify + bounded retries.

### FLOW-15 — Office edit with resident context
**Actors:** agent · `22` · `19` · `34`.
**Steps:** open → resident context + exclusive lease → L1 reads → L2 ops (batch atomic) → validate → commit (staging → **fsync** → atomic swap) → preview/verify → receipt.
**Terminal:** committed + receipt; document stays resident for the session.
**Failure branches:** crash mid-write → op-log replay; second writer → “in use”; engine limitation → typed guidance (no silent lossy path).

### FLOW-16 — Artifact lifecycle
**Actors:** producer (agent/workflow/user) · `29` · `34`.
**Steps:** create → version (immutable) + provenance → preview projection → verify (if external effect) → receipt → optional **explicit** library promotion (`saved_artifact`/`template`).
**Terminal:** artifact versioned; promotion only by user action.
**Failure branches:** GC vs receipt pin → receipt wins (never collected); location lost → `unresolved` + re-link path.

### FLOW-17 — Approval lifecycle
**Actors:** requester (agent/workflow) · `12` · channel (`32`) · user.
**Steps:** request (prompt, options, context, timeout) → routed to bound channel → user decides (approve/reject/edit/provide-data) → recorded once → resumed path; timeout per class (default deny).
**Terminal:** granted/denied/edited/expired.
**Failure branches:** channel offline → waits durably; edit → immutable original + edited draft both recorded; notification ≠ receipt.

### FLOW-18 — World event → workflow trigger
**Actors:** `21` collector (W1 file deltas, W2 process/window, W5 browser) · `30` · `20` trigger.
**Steps:** change observed → delta validated (cursor/epoch, overflow-safe) → world update + event → workflow trigger resolves → occurrence materialized → run starts.
**Terminal:** run per `20`; world state updated with freshness.
**Failure branches:** watcher overflow → scoped rescan + anomaly; event storm → concurrency policy + backpressure.

### FLOW-19 — Crash/restart recovery
**Actors:** Core (all stores) · scheduler · gateways.
**Steps:** boot → stores open + migrations → work queue rebuilt from log/checkpoints → runs resumed per matrix → environments/processes reconciled → world collectors resume from cursors → pending approvals re-surfaced.
**Terminal:** steady state; every gap recorded.
**Failure branches:** torn write → op-log/staging recovery; unknown process → reap/re-attach (audited); keyless effect → `needs_attention`.

### FLOW-20 — Credential use
**Actors:** `18`/`28`/`14` · vault (`12`).
**Steps:** caller requests `use(secret_ref, action)`-style operation → vault performs/attaches at call time → audit entry → value never returned to the caller.
**Terminal:** action performed; no credential exposure.
**Failure branches:** vault unavailable → typed `Unavailable` (no plaintext fallback); scope mismatch → denied + audit.

### FLOW-21 — Extension install & activation
**Actors:** user · `31` review gate · `12` · `19` sandbox.
**Steps:** install (file/folder) → manifest inspected → review gate (surfaces, permissions, provenance) → enable per scope → activation signal from task relevance → bounded instructions injected (`16`) → resources loaded on demand.
**Terminal:** installed + enabled; skill active only when relevant.
**Failure branches:** missing capability requirement → guidance mode; crash loop → auto-disable + audit; version skew → rejected with window.

### FLOW-22 — Project open / warm-up
**Actors:** UI · `11` · `25`/`26` · `21`.
**Steps:** workspace selected → identity resolved (`25` §5) → watchers attach → indexing (incremental) → RepoGraph builds → RepoMap projection prepared → rules loaded → ready; heavy indexing continues in background.
**Terminal:** ready state with freshness labels.
**Failure branches:** LSP absent → degrade to graph+grep; huge repo → bounded incremental indexing with partial map.

### FLOW-23 — Multi-surface handoff
**Actors:** surfaces (`32`) · `11` sessions · `12` approvals.
**Steps:** session lives in Core → another surface attaches → projections render → approvals/notifications route to the bound channel (the active surface); a missing channel waits durably → work continues regardless of surface.
**Terminal:** N surfaces, one session; no second brain.
**Failure branches:** surface crash → isolated; offline surface → approvals wait durably and re-surface.

### FLOW-24 — Dangerous effect verification
**Actors:** requester · `12` · `13`/domain · `34` · `29`.
**Steps:** dangerous capability (send/redact/delete/mass-write) → possible ASK approval → ticket → execute → **deep verification** (redact⇒extraction check; send⇒delivery receipt; delete⇒count+audit; mass-write⇒sample diff) → receipt with verification record; mismatch → repair/`needs_attention`.
**Terminal:** verified receipt or explicit unverified/unresolved state.
**Failure branches:** verifier unavailable → effect blocked (dangerous class) or human-confirmed; repeat mismatch → escalation.

## DEC-054 Mission and ecosystem flows

### FLOW-25 — Mission creation
**Actors:** user · Experience · Mission · Work. **Steps:** classify request complexity → capture original request ref → draft proportional GoalContract and requirements → user may correct → commit contract v1 and Mission event → create initial milestone proposal. **Terminal:** draft/active. **Failure:** ambiguity affecting irreversible work → dependent branch waits; simple chat stays Work-only.

### FLOW-26 — Versioned plan
**Actors:** lead agent · Mission. **Steps:** propose nodes/dependencies/acceptance → submit PlanPatch with base version → validate cycle, scope, budget, contract → atomically commit PlanVersion → publish readiness. **Terminal:** committed/rejected. **Failure:** stale base → typed conflict and rebase, never last-writer-wins.

### FLOW-27 — PlanNode dispatch
**Actors:** Mission · Work scheduler · agent/workflow. **Steps:** ready node → choose adapter/runtime by verified capabilities and budget → create Work with node/attempt refs → build bounded ContextPacket and grants → scheduler admits → record Work link. **Terminal:** running/waiting. **Failure:** no eligible adapter → blocked with missing capability, not a false launch.

### FLOW-28 — Worker replacement
**Actors:** Mission · Work · adapters. **Steps:** failed Work settles with receipt/failure class → node remains → reconcile side effects → create fresh attempt/session with prior evidence → resume. **Terminal:** retrying/blocked. **Failure:** ambiguous irreversible effect → needs_attention, no blind retry.

### FLOW-29 — Goal-contract change
**Actors:** user · Mission · Work. **Steps:** capture instruction → classify as information/priority/constraint/stop → commit new contract version → impact analysis → steer or interrupt affected active Work → retain unaffected branches. **Terminal:** active/waiting. **Failure:** contradictory input → ask at affected gate.

### FLOW-30 — Invalidation
**Actors:** Mission · Artifact · Evidence. **Steps:** changed requirement/input/assumption → traverse dependency closure → mark evidence/artifacts stale → reopen affected nodes → propose PlanPatch. **Terminal:** reconciled. **Failure:** unknown dependency → conservative review-needed flag.

### FLOW-31 — Dormant resume
**Actors:** user · Mission · Runtime. **Steps:** load latest contract/plan → rebuild projections → inspect environment fingerprint and external refs → invalidate drifted closure → replan ready nodes → dispatch. **Terminal:** active/waiting. **Failure:** missing auth/browser/profile → scoped blocker.

### FLOW-32 — Environment drift
**Actors:** Runtime · World · Mission. **Steps:** compare repo head/dirty hash, resource versions and provider epochs → emit drift report → verify affected assumptions/evidence → invalidate selectively. **Terminal:** current/review-needed. **Failure:** inaccessible resource → status unknown, no mutation.

### FLOW-33 — No-progress escalation
**Actors:** Work · Mission. **Steps:** fingerprint attempts and material delta → detect repeated failure → apply typed recovery ladder within budget → change strategy or stop branch. **Terminal:** retrying/blocked/no_progress. **Failure:** heartbeat alone never resets progress counter.

### FLOW-34 — Mission outcome verification
**Actors:** Mission · independent OutcomeEvaluator. **Steps:** load current criteria and evidence → deterministic checks → bounded semantic review where needed → record per-requirement result → Stop Controller decides. **Terminal:** completed/partial/blocked. **Failure:** self-report-only or stale proof → not_tested.

### FLOW-35 — Partial termination
**Actors:** Mission · user. **Steps:** identify unmet criteria and active side effects → settle children safely → create evidence bundle with partial results and recovery options → record terminal reason. **Terminal:** completed_partial/budget_exhausted/failed. **Failure:** pending effect cannot be called completed.

### FLOW-36 — Heterogeneous integration
**Actors:** lead · child agents · Mission. **Steps:** dispatch independent nodes to compatible agents → isolate writers/profiles → collect receipts → integration node resolves conflicts → independent verifier checks combined outcome. **Terminal:** integrated/blocked. **Failure:** incompatible outputs → explicit conflict/rework, not blind concatenation.

### FLOW-37 — Provider failover
**Actors:** Work · adapter registry · Mission. **Steps:** classify failure → checkpoint/reconcile → choose compatible alternate agent → rebuild packet → new attempt under same node. **Terminal:** resumed/blocked. **Failure:** required capability absent → no downgrade.

### FLOW-38 — Artifact dependency invalidation
**Actors:** Artifact · Mission. **Steps:** upstream version changes → traverse edges → mark downstream stale → offer/dispatch regeneration with grants → re-verify. **Terminal:** current/stale. **Failure:** inaccessible source → show uncertainty.

### FLOW-39 — Skill proposal from work
**Actors:** user · Work/Workflow · Skill store. **Steps:** opt-in capture → redact → separate deterministic/adaptive steps → draft skill/workflow → review grants → dry-run/evaluate → publish version. **Terminal:** published/rejected. **Failure:** secret or unscoped effect → reject.

### FLOW-40 — Controlled skill improvement
**Actors:** feedback · evaluator · skill store. **Steps:** draft versioned patch → run historical and side-effect regression evals → compare against current → policy/human promotion gate → pin new activations. **Terminal:** promoted/rejected. **Failure:** regression → old version remains active.

## DEC-055 Experience flows

### FLOW-41 — Composer reference and agent selection
**Actors:** Experience · agent registry · context · Trust. **Steps:** persist draft → `+`/`@` picks a scoped identity/version/selection → `/` resolves host/native namespace → agent picker shows probed readiness → conditional model and access chips show effective capability → send Work request. **Terminal:** accepted Work or typed setup need. **Failure:** stale selection/agent disconnect → keep draft and ask to refresh; no silent switch or command collision (`48` §3).

### FLOW-42 — Workbench edit from conversation
**Actors:** Experience · Artifact/Files · domain provider · Trust. **Steps:** open stable tab → select typed range → agent receives exact-version ref → preview edit/diff → check external version/lease → save atomically as new version → render/verify → update dependencies and chat artifact card. **Terminal:** current version or conflict. **Failure:** unsupported fidelity → read-only/native fallback; corrupt save → old version preserved (`48` §5, `29`).

### FLOW-43 — Browser or desktop takeover
**Actors:** user · Browser/Computer runtime · Work. **Steps:** user takes control → worker yields input and records observation epoch → user acts → user returns control → fresh snapshot/reconcile → worker may resume. **Terminal:** resumed or waiting. **Failure:** session/bridge lost → no stale coordinate/ref reuse (`23`, `24`, `48`).

### FLOW-44 — Agent setup and scoped extension
**Actors:** user · agent registry · extension registry · Trust. **Steps:** staged read-only discovery → probe supported model/auth/overlay features → user signs in through actual owner → select per-scope host MCP/skill/plugin grants → resolve collision and lazy startup → attach session overlay if supported. **Terminal:** ready or explicitly unavailable. **Failure:** unsupported injection never triggers a native config edit (`46`, `48` §7).

### FLOW-45 — Library retrieval and staleness
**Actors:** Library · Files/Search · Artifact graph · Context. **Steps:** register origin/version/metadata → extract/index with coverage → permission-filter search → cite exact version and location → changed input invalidates downstream. **Terminal:** current cited result or stated gap. **Failure:** revoked/unindexed/corrupt source is excluded or marked uncertain; never invented (`29`, `48` §6).

### FLOW-46 — Team handoff and integration view
**Actors:** Mission · Work · external agents · Experience. **Steps:** assign independent bounded nodes → show host children separately from native-reported children → collect structured handoffs → integrate and verify outcome → expose per-worker status/evidence in right pane. **Terminal:** verified Mission node or explicit conflict. **Failure:** unbounded swarm/opaque child cannot receive host controls (`15`, `35`, `48` §8).

### FLOW-47 — Schedule/event trigger handoff to remote owner

## DEC-058 Machine Observer flow

### FLOW-048 — Consent, snapshot and optional one-shot elevation
**Actors:** user/authorized agent · Experience · Trust · Capability · Runtime · Machine Observer · optional Windows UAC/helper. **Steps:** a named observation request resolves to category, purpose, recipient and retention → if consent is absent, show the in-app notice before any sample → on Enable, record scoped consent through Trust and return only the selected category in the plain-language System Workbench → on Not now, collect nothing and show one inline Enable / Keep off explanation → the normal-user Observer attempts supported providers → if a provider returns `permission_needed`, expose the exact shield-marked reading and standard-access fallback → only a fresh user action authorizes Runtime to invoke Windows UAC for the typed helper operation → helper reads from its allowlisted provider, returns one provenance-stamped result directly to Core, and exits → Core passes that single value to bounded history only if separate history consent covers it → Core emits consent/config/health/alert transitions, not each sample → an authorized agent receives only a separately granted scoped projection. **Terminal:** current values with source/freshness/status, or supported standard-access values plus a clear unavailable reason. **Failure:** consent denial means no sample; UAC denial keeps standard access and shows one contextual non-error explanation with Try once / Keep standard access; dismissal is remembered; no Observer-driven install elevation, no installer approval interpreted as data consent, auto-retry, whole-app elevation, hidden service, stale grant restore or prompt storm (`08` REQ-OBS-003, `19` §6/§6.1, `48` §5/§7, `51` §6).

### FLOW-049 — On-demand process/service diagnostics
**Actors:** user · Experience · Trust · Capability · Runtime · Machine Observer · optional Windows UAC/helper. **Steps:** user selects Advanced diagnostics for a process/service → UI explains the separate `process.diagnostics` category and exact fields → Trust authorizes that category; an agent also needs its separate Work-scoped disclosure grant → provider builds a closed read-only field plan and requests minimum OS rights → accessible fields return with process-generation identity, source/time and per-field status → protected/denied fields stay unavailable; if a specific supported field genuinely requires elevation, UI names it and offers a shield-marked one-time read → Runtime launches the one-shot typed helper only after that action → result returns to Core and helper exits → no diagnostic data is retained or shared beyond the authorized result unless separately opted in. **Terminal:** requested supported facts plus honest per-field gaps. **Failure:** no broad access mask, process-memory read, handle duplication, sensitive-field expansion, retry loop or privilege bypass; UAC denial preserves process summary and ordinary System overview (`08` REQ-OBS-010, `12` §10, `51` §2/§6/§7, `49` TC-050).
