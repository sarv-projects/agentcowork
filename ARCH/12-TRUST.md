# 12 — Trust & Control

> **DEC-054 amendment:** Guard/tickets/vault/audit govern Core-mediated calls. A `SelfContained` ACP agent's native shell/files/network/MCP calls are under that agent's own permissions and OS environment, not this Trust path. The UI and receipts must disclose the actual governance class. Discovered native configuration is read-only; shared grants are scoped by real binding/session/Work identity (`46`).

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-TRUST-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the one place where permission, authorization, custody and audit live. **Guard decides; agents request; prompts never enforce** (P-13, DEC-002).
> **Dependencies:** `10-KERNEL` · `11-WORK` · `19-RUNTIME-ENVIRONMENTS` (sandbox hosts) · `30-EVENTS` (audit feed). **Consumers:** `13`/`14` (capability execution), `15`, `21` (consent), `22`–`28` (domains), `32` (projections).
> **Evidence:** product-owner brief (trust section: projections, defaults, isolation) · `ARCHIVE/v1-research/agent-harness-verification.md` §A4 (approval policy enum · sandbox policy · exec-policy engine; anchors `codex-rs/protocol/src/protocol.rs:969-1125`, `sandbox.rs:10-16`, `execpolicy/src/`) · DEC-028 · INV-01…12, 24 · `ARCH/06-DATA-MODEL.md` (DM-009/010) · `ARCH/07-CONTRACTS.md` (CTR-011/012/013).

## 1. Purpose & rules

**Owns:** policy evaluation · Guard (three layers) · **control-plane admission control** · tickets · approvals · vault custody · egress control · audit hooks · consent policy · enforcement of the external-agent projection boundary.
**Never owns:** domain logic · collector implementations (`21` records consent; Trust evaluates it) · UI copy.

1. **One decider** — exactly one component returns ALLOW / ASK / DENY (INV-04).
2. **One Core egress path** — Core-mediated outbound network passes Guard; no Core side doors (INV-05, DEC-054).
3. **Custody** — provider credentials exist only in the vault; contracts use `use`-style APIs, never reads (INV-02, CTR-013).
4. **Enforcement lives here, never in prompts** — no agent instruction is a security boundary (P-13).
5. **Every decision is audited** — including denials and forget/delete (INV-24).

### 1.1 Control-plane admission (rate limiting)

The control plane — the `nativeCall`/IPC surface and the kernel tool path — is rate-limited at its entry, so one caller cannot saturate the policy engine, the ticket store or the audit chain. Admission is a **bounded token-bucket gate**: it is *not* a fourth policy layer and *not* a second egress path. It never returns ALLOW / ASK / DENY for an effect; it only decides whether a request is looked at at all.

- **Two tiers compose.** One global bucket for the whole control plane, plus one bucket per `(caller, command)`. Both must have a token to proceed, so one noisy command cannot starve the rest of the surface and one noisy caller cannot spend another's budget. A refusal on either tier spends nothing on the other.
- **Caller identity is host-owned.** The Tauri IPC gate keys by the host-created webview label; the kernel tool gate keys by its host-owned `ToolService` instance. Neither trusts a renderer header or agent-supplied `sessionId`/`agentId` to create a new rate-limit principal. Richer per-agent attribution must arrive in an authenticated host context before it can partition admission buckets.
- **The shape is the contract; the numbers are not.** The **steady rate is the control** — the sustained rate one caller may reach; the per-key burst only has to absorb a legitimate bursty turn. The **map cap is the memory bound**: tracked buckets are LRU-capped and idle buckets expire on a TTL, so a churning key space cannot pin memory. The concrete rates, bursts, map cap and TTL are a **product knob** (configuration), *not* a constant this doc fixes — a change to them is not a spec change.
- **Fails closed.** An unavailable limiter is a denial, never an allow. The gate runs *before* any command body, state lock, disk access or ticket minting, so a refused call performs no work and there is no second path around it.
- **Refusal shape.** The canonical `Unavailable` code — provider/agent/environment down or degraded, retryable with backoff (`10` §3) — carrying a stable `rate_limited` reason, the **scope** that refused (`global` or `caller_command`), and a `retryAfterMs` backoff. The kernel error taxonomy is canonical and extending it requires a `DEC`, so a rate-limited refusal introduces **no new error code**.
- **The clock is caller-supplied** (monotonic epoch-ms), never a wall-clock delta, so durations are correct across a clock adjustment and the gate is testable without sleeping.

## 2. The three layers (DEC-028) — never collapsed into one enum

| Layer | Question | Shape |
|---|---|---|
| **Platform confinement** | What may this process do on the OS? | Sandbox policy: `read-only` · `workspace-write { writable_roots, network_access }` · `full`; per-platform backends (Windows restricted token / MXC · Linux landlock/bwrap/seccomp · macOS seatbelt); **protected subpaths** (e.g. VCS hooks) stay read-only inside writable roots. |
| **Approval policy** | When is a human asked? | Policy enum (`unless-trusted` · `on-request` · `granular` · `never`) + per-category granular config (sandbox escapes · exec rules · permission requests · skill installs · MCP elicitations …). |
| **Declarative exec rules** | Which commands are pre-authorized? | Command/prefix/network pattern rules; rule engine defaults to **ask**; every decision recorded. |

Guard composes the three into a decision, then issues/validates tickets. Sandboxing is executed by `19-RUNTIME-ENVIRONMENTS`; the policy lives here.

## 3. Policy model

- **Risk tiers** (owner brief defaults): *everyday* → allow (workspace read/write/edit, normal commands/tests/deps, local git, browser navigation, Office editing, MCP reads) · *dangerous* → ask (permanent deletion, destructive shell, credential access, OS/security changes, disk ops, mass external writes, destructive git) · *catastrophic* → always gated, even in Full Access (irreducible gate).
- **Capability risk classes** (`safe` / `sensitive` / `dangerous`, DM-011) map to default decisions and to verification depth (`34`, INV-19).
- **Scopes:** policy evaluates innermost-applicable with outer ceilings (global → workspace → agent → session → run); an external agent's own settings may narrow the ceiling but never widen it, and conflicts are logged (EDGE-152).
- **Full Access** is user-activated; it widens the allow tier but never removes the catastrophic gate.
- **Policy snapshots** are recorded on tickets/handles (`permission_snapshot`) so decisions are reproducible.

## 4. Tickets (DM-009)

- Issued after ALLOW (or a granted approval); bound to `capability_id`, `provider_id`, `environment_id`, scope (paths/targets/patterns), `uses`, `expires_at`, `provider_epoch`, `approval_ref?`.
- Validated at execution time; **stale epoch / expired / revoked ⇒ `InvalidState`**; revoke on provider restart or cancellation.
- Single-use vs bounded-multi-use is declared at issue; effects never execute without one (INV-03). Bounded-use tickets decrement `uses` atomically at validation; a losing concurrent execution fails `InvalidState` and both outcomes are audited — a ticket can never be spent twice (EDGE-039).

## 5. Approvals (DM-010, DEC-021)

- One primitive for both agents (questions) and workflows (approval nodes): `request(prompt, options: approve | reject | edit | provide-data, context, timeout)`.
- **Vocabulary mapping:** the UI card's `once · session · always · deny` choices express the ticket scope of this one primitive (`once` ≈ approve with a single-use ticket; `session` ≈ approve bounded to the session; `always` ≈ approve plus a persisted declarative rule where the backend honours it; `deny` = reject). The policy enum (`unless-trusted · on-request · granular · never`, §2) decides *when* a human is asked; the recorded decision (`DM-010`) carries *what* they decided (`AGENTCOWORK-UI.md` §5.10).
- Routed through UI/channels (`32`); durable across waits (`11` §4); decisions recorded once and referenced by tickets/receipts.
- Expiry policy per action class (default: expire ⇒ deny, surfaced).

## 6. Vault & custody (CTR-013)

- Credentials never appear in prompts, context, events, logs, receipts, or code (INV-02).
- `use`-style API only (perform a signed call / inject into an adapter at call time); enumeration by agent code is impossible.
- Scope per provider/profile; rotation supported; local-only by default; vault access itself is audited.

## 7. Egress

- All **Core-mediated** outbound network through the Guard egress (INV-05, DEC-054): allowlists by domain/method per policy; per-agent and per-session scopes; request metadata audited (never payloads by default). Native external-agent network remains outside this guarantee.
- Rust HTTP callers that need a bounded request use `agentcowork_guard::egress_http::GuardedHttpClient`: resolve/check once through `toctou::bind_url_with_policy`, pin that set in the resolver, bind the client to one origin, disable redirects, reject Host/Proxy-Authorization overrides, and cap response bytes. The current adoption is partial; the live inventory is `TODO.md` `TASK-PROV-002` / `42` FIX-09. A blocking system DNS call may overrun the configured request timeout, and this client does not yet provide streaming cancellation.
- MCP/HTTP/CLI child processes inherit governed network through their environment (`19`).
- The P6/P9 sweep claim that no direct network clients exist above the adapter layer was disproved during W0 re-verification. Do not treat a pure `netfloor::preflight_url` followed by a default client as guarded socket egress; it does not pin DNS or disable redirects. Each direct client must migrate to the shared Guard transport or have a documented, reviewed non-egress role before W0 closes.

## 8. External-agent boundary (DEC-009 — projection enforcement)

The Agent Gateway (`32`) *builds* projections; Trust *enforces* them:

| Projection | Enforcement |
|---|---|
| Identity / agent contract | Gateway-issued and audited; the session binding fixes the agent id — spoofing is rejected (`32` §3). |
| Capability set | Effective = Installed × Available × Allowed × Relevant; anything else resolves to `NotFound` for that agent. |
| Context (incl. memory) | Sensitivity-filtered slices (`16`); memory is a filtered **recall-only** projection — bound project + own session/task + user preferences, no org, no other projects, `confidential` only with a recorded loadout — with scopes and ceilings **actor-derived**, never caller-supplied, and no write path exposed (`17` §4/§9, DEC-038/042/043); cross-project/confidential leakage = 0 (INV-10). |
| Workspace | `allowed_paths` / `read_only_paths`; **interception, not un-discovery** — out-of-scope reads are denied and logged (interception point and its open part below). |
| Tools / MCP subset | Only the granted subset is mounted; the rest is invisible. |
| Artifacts | Via the artifact gateway with permissions; never raw storage. |
| Events | Filtered stream; never the internal bus. |

Never exposed: service topology, stores/schema, queues, scheduler internals, vault, policy-engine internals, model-router internals, other agents' state (INV-11).

**Read interception (owner ruling).** Reads are intercepted at the **scope boundary, never per-read approval**: every read path resolves through the session's path scopes and the protected-subpath rules (`25` §7.1). Out-of-scope is a **typed denial plus an audit row**; an in-scope read requires **no approval and no prompt**. Approval-gating each read is not a stronger control here, it is a broken one — the file tree is a browsing surface, and a prompt per node is what made the gap unclosable by patching. The scope boundary is where a human's decision (which roots this session may see) is actually made, so that is where reads are checked. `25` §7.1 carries the same ruling for the file module.

**Still open — a tracked follow-up, not a completed fix.** The *renderer-chosen path* is **not yet resolved against the scopes in code**: `fs_read_file` (`src-tauri/src/fs_cmds.rs:91`) and `fs_list_dir` (`src-tauri/src/fs_cmds.rs:33`) pass the caller's `path` straight to `std::fs`, with no interception — the write path already floors (`src-tauri/src/fs_cmds.rs:149` → `control::floor_user_file`). The behavior above is therefore **specified but not implemented**; it is a tracked follow-up alongside the open `fs_*` authority item in the code-phase fix register (`42` §4, FIX-06), and no claim of enforcement is made until the read commands resolve their path through the session's scopes.

## 9. Audit

- Append-only, tamper-evident chain; every Core-mediated mutating operation logged (INV-24): actor · action · target · decision · ticket · result · timestamps. Native agent effects are outside this audit coverage.
- Denials and forget/delete/wipe are first-class audit entries.
- Memory mutations are audited under the local-mutation class (DEC-042); the record carries **no item body**, and suppression digests are keyed (DEC-039).
- Audit reads are themselves access-controlled; exports carry the chain proof.

## 10. Consent (collector-facing; DEC-058/059)

Trust owns the product-level `ObservationConsent` record (DM-043) and evaluates it; the collector owner (`21` or `51`) records each active collection instance and its actual provider/OS state. On first explicit System Workbench use, or before fulfilling an unconsented user/agent request, Experience presents consent before any sample (DEC-059); Core returns `authorization_required` with the complete missing local-consent and Work-share grant set and never lets an agent grant its own access. If both are missing, the UI can offer independent choices in one prompt, but the request remains held until both are recorded; granting local observation never implies Work disclosure. Consent is versioned by category, scope, purpose, sampling cadence/active condition and retention. The grant record persists until revocation, but active sampling follows its scope: basic overview sampling is on-demand or while the System page is open; sampling after it closes requires a separately granted history/background scope and cadence. This is a narrow DEC-058 exception to the baseline no-persistent-grants rule. Local consent authorizes collection only: every agent/work disclosure still needs its independent scoped capability grant. OS elevation and helper tickets are never persistent and never inferred from app-install/UAC consent. The UI shows active categories, recipient, collection state, cadence, last sample, retention and revoke state; revocation closes collection leases, invalidates helper grants and prevents future reads.

Per collector instance, the owner records collector id+version · consent ref · effective scope · capability required (standard/elevated/OS permission) · what the OS actually granted and how · event source + epoch/cursor · data classes · start/stop + retention · revocation path · audit ref. A collector cannot convert an absent/revoked consent record into authorization. Deny-by-default; no background sampling while consent is absent; visible indicator while collection is active.

## 11. Failure modes

| Failure | Behavior |
|---|---|
| Policy engine error | **Fail closed** — DENY with reason; audited. |
| Guard decision deadline exceeded | Bounded decision deadline; timeout ⇒ **DENY** with reason + audit (fail closed); an implicit allow is never returned (EDGE-038). |
| Vault unavailable | Credentialed calls fail typed (`Unavailable`); no plaintext fallback ever. |
| Egress engine down | Outbound fails closed; offline capabilities unaffected. |
| Ticket replay / stale epoch | `InvalidState`; audited; provider epoch bump re-issues. |
| Approval timeout | Per-class expiry policy (default deny). |
| Policy conflict (scopes disagree) | Innermost decision applies unless an outer **ceiling** forbids; conflicts logged. |
| Control-plane rate limit reached | Typed `Unavailable` refusal (`rate_limited` + scope + `retryAfterMs`) **before** any work: no command body, no state lock, no disk, no ticket minted; audited (§1.1). Never queued, never silently dropped, never served by a second path. |
| Limiter state unavailable (poisoned lock) | Denial with the same typed shape — fail closed, never an implicit allow (§1.1). |

## 12. Interop

**Depends on:** `10` kernel · `19` runtime (sandbox hosts) · `30` events (audit) · vault storage.
**Exposes to:** `13`/`14` (decisions + tickets) · `15` (requests) · `21` (consent evaluation) · `32` (projection enforcement) · UI (approval prompts).
**DAG check:** Trust never executes effects; it authorizes them. No module may evaluate its own policy (INV-04).

## 13. Open questions (`OQ-TRUST-*`)

1. Granular approval category list for v1.
2. Approval routing defaults per channel and notification delivery; DEC-056 fixes the authority rule for all Core-mediated approvals, including mobile/API clients.
3. Pathfloor/netfloor mapping to the current crate reality (code phase — `agentcowork-guard` exists; wiring fidelity to verify).
4. Policy version storage + migration semantics.
5. ~~Consent-record ownership split confirmation (`21` records, Trust evaluates — assumed here).~~ Resolved by DEC-058/DM-043: Trust owns the user grant and collector owners record the active collection instance.

## 14. Evidence

Owner brief (projections, permission defaults, isolation: host / agent workspace / vault) · `agent-harness-verification.md` §A4 (three verified layers + anchors) · DEC-028 · INV-01…12/24 · `ARCH/06-DATA-MODEL.md` DM-009/010 · `ARCH/07-CONTRACTS.md` CTR-011/012/013 · `ARCH/21-WORLD-MODEL.md` §5 (consent fields).

**Code-phase anchors (§1.1, §8):** `crates/agentcowork-guard/src/ratelimit.rs` (the limiter) · `src-tauri/src/lib.rs:845` + `:82-149` (the IPC gate, wrapping the `invoke_handler`) · `crates/agentcowork-core/src/tools.rs:1176-1192` (the tool-path gate) · `src-tauri/src/fs_cmds.rs:33,91,149` (read commands un-intercepted, write command floored — the open part of §8).

## 15. Requirements (`REQ-TRUST-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-TRUST-001` | Egress fail-closed — one governed outbound path; allowlists; no direct clients above the adapter layer (INV-05). |
| `REQ-TRUST-002` | One approval primitive — request(prompt, options, context, timeout); durable across waits; recorded once (DEC-021). |
| `REQ-TRUST-003` | One Core authorization decider — every Core-mediated mutating effect is decided in Trust; no second Core permission path (INV-04, DEC-054). |
| `REQ-TRUST-004` | Vault custody, use-only — credentials never appear in prompts/context/events/logs/receipts; use-style API; scoped and rotated (INV-02). |
| `REQ-TRUST-005` | Tickets bind and validate — effect tickets carry scope/uses/expiry/provider epoch and are validated at execution (INV-03, DM-009). |
| `REQ-TRUST-006` | Three policy layers stay distinct — confinement, approval policy, and declarative exec rules never collapse (DEC-028). |
| `REQ-TRUST-007` | Audit completeness — every decision and denial is append-only, tamper-evident, and access-controlled on read (INV-24). |
| `REQ-TRUST-008` | Projection-only external agents — never expose topology, stores, queues, vault or policy internals (DEC-009, INV-10/11). |
| `REQ-TRUST-009` | Trust infrastructure fails closed — policy/vault/egress failure blocks the effect; no plaintext or open fallback. |
| `REQ-TRUST-010` | Catastrophic gate is irreducible — always gated, even under Full Access (risk tiers, §3). |
