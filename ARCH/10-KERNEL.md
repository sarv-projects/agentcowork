# 10 — Kernel

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-KERNEL-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the smallest layer: identity, errors, configuration, time, serialization, and the base conventions every module depends on. **No domain logic** (INV-14).
> **Evidence:** `ARCH/06-DATA-MODEL.md` (conventions), `ARCH/07-CONTRACTS.md` (contract rules), product-owner brief (“the kernel stays small”), INV-14.

## 1. Scope

**Owns:** id generation/validation rules · error taxonomy · configuration layering and validation · time primitives · serialization/storage conventions · the base envelope every contract uses (actor context, cancellation, result/error) · the minimal-kernel enforcement rule.
**Never owns:** policy (`12`) · scheduling (`11`) · storage engines (`17`/`29`/`30`) · domain types · protocol handling (`14`).

## 2. Identity

- **Entities:** uuidv7, minted by the owning service (single-writer, INV-06) — never by callers or UI.
- **Semantic ids:** capability ids are dotted names (`office.spreadsheet.edit`); provider ids are stable registry ids; model ids are `provider/model`.
- **Opaque rule:** ids never encode state, version, or meaning.
- **Human-facing short ids** (UI lists, receipts) are derived and non-authoritative; lookups always resolve through the owning service.
- **Reference scheme:** cross-entity references are by id (+type) only; content locations use a URI form owned by `29-ARTIFACTS` (scheme decided there).

## 3. Errors

Taxonomy (canonical for every boundary; extensions require a DEC):

| Code | Meaning | Retryable |
|---|---|---|
| `AuthorizationDenied` | Guard denied; ticket missing/expired/insufficient | no (re-plan or ask) |
| `NotFound` | target does not exist or is filtered by policy | no |
| `Conflict` | concurrent state change (lease, version, duplicate) | yes (bounded) |
| `Unavailable` | provider/agent/environment down or degraded | yes (backoff) |
| `Unsupported` | requested operation is not supported by this binding or environment | no (choose another binding or operation) |
| `Timeout` | call exceeded deadline | eligible after reconciliation; never blindly repeat an uncertain effect |
| `InvalidState` | operation not valid for current state (incl. stale epochs) | no |
| `Internal` | bug | no (report + log) |

Rules: typed and actionable; **no secrets or user content in messages**; stable codes for UI mapping; cause chains preserved for diagnostics; boundary errors never leak internals (INV-11). `guidance`/`requires_user_action` are **results with next steps**, not failures (`13`).

## 4. Configuration

Preference scopes are declared by the typed Core settings registry (`CTR-032`, DEC-056). The declared precedence for a key is applied only across scopes that key permits; every effective value records its source and revision. Cosmetic device preferences may stay local; authorization, connector grants and policy are Core-owned and cannot be overridden by a later UI/session/run preference.

```
defaults → permitted device/user/workspace/agent/conversation/mission preference overrides
```

- Schemas are typed, versioned, and validated at load. Unknown or invalid keys are rejected with an actionable migration/error report; they never become effective configuration.
- **No secrets in config** — vault references only (INV-02).
- Feature flags: default-safe, locally overridable, no remote dependency for core behavior.
- Config changes that affect running work are versioned into that work's record (reproducibility).

## 5. Time

- **Storage:** integer epoch milliseconds, UTC (`06` conventions). Display formatting is a boundary concern only.
- **Durations/timeouts:** monotonic clock; never wall-clock deltas.
- **Schedules:** absolute times + explicit timezone policy; DST resolved at the boundary. `20-WORKFLOW` owns trigger evaluation and the fenced trigger owner; `11-WORK` admits resulting execution (DEC-057).
- **Clock skew:** never infer cross-machine order from wall-clock timestamps. Remote handoff uses persisted occurrence identity, cursor and fencing epoch (`20`); channel projections may be cross-device (`32`).

## 6. Serialization & storage conventions

- **Canonical JSON** at boundaries (IPC, export, events); stable field ordering; numbers as integers where possible (no float precision surprises in ids/sizes).
- **Durable stores:** SQLite (WAL) per store owner; one writer per store (INV-06); no cross-module direct DB access — services only.
- **Migrations:** forward-only, idempotent, tested; run before the feature that needs them; failures block cleanly (no partial schema).
- **Content refs over copies:** prefer references; inline only when bounded and reconstructable.
- **UTF-8 everywhere**; sizes in bytes; token counts only via `18-MODEL-ROUTING`.
- **Invalid payloads fail typed** — bad UTF-8, non-canonical JSON, or float-unsafe numbers are rejected at the boundary, never lossy-coerced; ids and sizes stay integer-safe (EDGE-113).

## 7. Contract plumbing (base envelope)

Every `CTR-*` (in `07`) carries:

- **Actor context:** who is calling (user · agent · workflow), with scope + a permissions **reference** (`permissions_ref`, resolved by the Trust owner — a snapshot never travels inside the envelope; DEC-050, INV-11).
- **Cancellation:** cooperative cancellation token; deadlines propagate; cancellation leaves durable state consistent (INV-16).
- **Idempotency:** effect invocations carry keys minted here (`work_id` + `ticket`), so retries cannot double-apply where providers support dedupe.
- **Result envelope:** `{ ok, value } | { error: { code, message, retryable, cause? } }` — language-neutral schema, versioned.

Canonical shapes (illustrative, canonical JSON):

```json
{ "ok": true, "value": {} }
{ "error": { "code": "Unavailable", "message": "provider unreachable", "retryable": true, "cause": {} } }
{ "actor": { "kind": "user | agent | workflow", "id": "…", "scope": "…", "permissions_ref": "…" },
  "deadline_ms": 120000, "cancel_token": "…", "idempotency_key": "<work_id>:<ticket>" }
```

Work-owned contracts (`CTR-003` `WorkService`, `CTR-004` `SessionLog`, `CTR-026` `Scheduler`) inherit this envelope and are registered in `ARCH/07-CONTRACTS.md` (Provisional).

## 8. Minimal-kernel rule (enforcement)

1. The kernel contains no domain logic — no Office, browser, file, or agent semantics.
2. No module may add a “just one” kernel special-case; capabilities land in their modules.
3. Kernel surface changes require a `DEC` entry and an INV-14 checklist pass (dependency-direction check).
4. Everything depends on the kernel; the kernel depends on no higher plane (`11`–`38`).

## 9. Failure modes

| Failure | Behavior |
|---|---|
| Config parse error | Reject that layer/key and surface the error; use a valid prior/default value only where the setting schema explicitly allows fallback. Policy and grant state never inherit a weaker fallback. |
| Migration failure | Block the dependent feature; never run against a half-migrated store; report exact migration + error. |
| Clock anomalies (jump backward) | Monotonic clock shields timers; schedules re-evaluate conservatively. |
| Id collision (uuidv7) | Treated as Internal error; single-writer minting makes this a bug, not a case. |

## 10. Open questions (`OQ-KRN-*`)

1. Result/error envelope: one schema for all boundaries vs transport-specific wrappers (adapters may map).
2. Migration-runner ownership: kernel utility vs storage owner (`19`/`30`).
3. Trace/span ids: separate namespace or derived from `work_id`/`step_id` (observability shape, `30`).
4. Canonical JSON strictness: number precision + key ordering rules for cross-language consumers.
5. Short-id format for UI (prefix + base32?) — cosmetic, low stakes.

## 11. Interop

**Depends on:** nothing.
**Exposes to:** the higher planes in `11`–`38`.
**DAG check:** no cycles are possible while this rule holds (INV-14).

## 12. Requirements (`REQ-KERNEL-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-KERNEL-001` | Minimal kernel — no domain logic; kernel depends on no higher plane (INV-14). |
| `REQ-KERNEL-002` | Single-writer identity — uuidv7 minted by the owning service; ids stay opaque. |
| `REQ-KERNEL-003` | Typed, safe errors — canonical taxonomy; no secrets; no internals across boundaries. |
| `REQ-KERNEL-004` | Typed settings and permitted scope precedence; Core policy/grants cannot be overridden by UI preferences; vault refs only (DEC-056). |
| `REQ-KERNEL-005` | Time discipline — epoch-ms UTC, monotonic durations, explicit timezone policy. |
| `REQ-KERNEL-006` | Canonical serialization and store conventions — stable JSON, SQLite WAL one-writer, forward-only migrations. |
| `REQ-KERNEL-007` | Base envelope on every contract — actor context, cancellation, idempotency, versioned result. |
