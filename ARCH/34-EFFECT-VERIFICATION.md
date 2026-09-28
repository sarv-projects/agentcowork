# 34 — Effect Verification

> **DEC-054 amendment:** This module verifies individual **Core-mediated** effects. Mission outcome evaluation in `36` separately tests the user's versioned goal against requirements and evidence. A self-contained agent's native tool action can be independently observed, but not assigned a Core effect receipt or authorization it did not use.

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-VERIFY-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the **verification plane** — `observe → validate → render → verify → reconcile` before any receipt. Verification depth scales with the capability’s risk class (INV-19; DEC-022/023). “Implemented but unverified” can never masquerade as complete.
> **Dependencies:** `13-CAPABILITY` (hooks/risk classes) · domains (`22`–`28` provide validators/renderers) · `19-RUNTIME-ENVIRONMENTS` (execution of checks) · `29-ARTIFACTS` (receipts/records) · `30-EVENTS`. **Consumers:** the governed path itself.
> **Evidence:** product-owner brief (verification plane; render→inspect→fix loop; “verification loop is critical”) · `ARCH/22-OFFICE.md` §5 (format validation hooks) · `ARCH/29-ARTIFACTS.md` §3 (receipt policy) · `ARCH/26-CODE.md` §7 (tests as verification) · DEC-022/023 · INV-19.

## 1. Purpose & rules

**Owns:** the verification pipeline · the verification-hook registry · the risk→depth matrix · verification records (the evidence a receipt cites) · reconciliation semantics · verifier failure policy.
**Never owns:** the effect itself (`13`/`14`/domains) · receipt storage (`29`) · policy (`12`).

1. **Depth scales with risk** — `safe` · `sensitive` · `dangerous` (DM-011) map to declared minimum verification.
2. **Deterministic first** — validators, re-reads, structural checks and diffs lead; model/vision inspection runs only where necessary and consent-gated.
3. **Verification is read-only** — it never mutates the effect; repairs are **new operations** (`15` recovery / `20` repair paths).
4. **Receipts state what ran** — every receipt records the verification performed (what passed, what was skipped, why).

## 2. Pipeline

```
observe → validate → render → verify → reconcile
```

The five-stage list is completed by `DEC-046` (`ARCH/04-DECISIONS.md`; DEC-023's rules otherwise unchanged).

1. **Observe:** capture the effect context — request, inputs digest, capability/provider, environment, ticket.
2. **Validate:** deterministic validators per capability/domain (Office structural checks `22` §5; file identity/size `25`; test results `26`; browser URL/form assertions `23`; delivery/compose checks `28`).
3. **Render:** produce an inspectable artifact where applicable (preview/render refs, `29` §7) — for humans always; for model vision only when scheduled by the depth matrix.
4. **Verify:** compare intended vs actual postconditions (re-read state; diffs; assertions).
5. **Reconcile:** record `pass | fail | partial`; failures route to repair (`15`), escalation, or `needs_attention` (`20` §4) — never a silent retry.

## 3. Risk → depth matrix

| Risk class | Minimum verification | Examples |
|---|---|---|
| `safe` | Deterministic validation where meaningful; otherwise trivially observable effect | metadata reads; scaffold creation |
| `sensitive` | Validation + re-read/render check | file write → re-open + hash; document edit → structural validation (`22`); spreadsheet write → IronCalc recalc (`22` §3) |
| `dangerous` | Validation + render/inspection + reconciliation (+ confirmation surfaces where required) | PDF **redact** → text-extraction check proving removal; email send → delivery receipt; delete/wipe → count + audit; mass writes → sample diff |

Per-capability overrides live in descriptors (`13` §2 `verification` field); the matrix is the floor, never the ceiling.

## 4. Verification-hook registry

- Capabilities declare hook refs; domains implement hooks (validators · renderers · comparators).
- **Hook contract:** `(effect record) → verification record` — idempotent · bounded · read-only · no side effects; failures are typed (`10` §3).
- **Missing hook:** fall back to risk-class default (re-read/observe); the receipt records that the deep hook was unavailable.

## 5. Verification records

`{ id · effect_ref · capability/provider · checks[] (name · kind · result · evidence refs) · render refs · outcome · duration }` — stored once; **receipts reference the record** rather than duplicating it (`29` §3). Verification runs emit events (`30` §3 `verification.*`).

## 6. Reconciliation

- **Postconditions** are declared per operation (“cell B2 = 42”, “file exists with hash H”, “tab URL = X”, “message accepted by provider”).
- **Partial outcomes** (some batch operations applied) are recorded explicitly; batch atomicity (`22` §4) prevents them where declared.
- **Repair** = a new work item/operation with its own ticket and receipt — never a silent re-execution.

## 7. Failure modes

| Failure | Behavior |
|---|---|
| Validator crash/absent | Verification fails → effect marked **unverified** in the receipt; policy may block the effect entirely for dangerous classes. |
| Weak validator (false pass) | Sampled audits + drift metrics (OQ-VER-02); misses feed back as new checks. |
| Vision scheduled but unavailable | Degrade with recorded gap; sensitive/dangerous classes may require human confirmation instead. |
| Reconciliation mismatch | Repair path or `needs_attention`; never auto-overwrite state. |
| Verification loop with no progress | Bounded retries then escalation (no infinite verify-repair cycles). |

## 8. Interop

**Depends on:** `10` · `13` (hooks/risk) · `19` · `22`–`28` (validators/renderers) · `29` · `30`.
**Exposes to:** the governed path (receipts cite records), `15` (repair decisions), UI (verification visibility) — through `CTR-023` (`EffectVerifier`; `07` §1).
**DAG check:** verification observes; it never authors effects and never decides policy.

## 9. Not in v1

Formal-method verification engines · a continuous independent audit service · automated repair synthesis · cross-effect global reconciliation (per-effect only).

## 10. Open questions (`OQ-VER-*`)

1. Default depth per capability family (beyond the risk floor).
2. Sampled-audit rate + metrics for validator quality.
3. Vision-in-the-loop consent UX and budget caps.
4. Verification-record retention (longer than events? tied to receipts).
5. Which domains need custom renderers first (Office/PDF likely).

## 11. Evidence

Product-owner brief (verification plane; render→inspect→fix; “create → render → inspect → verify → revise → deliver”) · `ARCH/22-OFFICE.md` §5 (validators/redact check) · `ARCH/29-ARTIFACTS.md` §3 (receipt policy) · `ARCH/26-CODE.md` §7 (tests) · `ARCH/23-BROWSER.md` §4 (structured assertions) · DEC-022/023 · INV-19.

## 12. Requirements (`REQ-VERIFY-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-VERIFY-001` | Verification depth scales with risk class; the matrix is a floor, never a ceiling (INV-19, DEC-022) |
| `REQ-VERIFY-002` | Verification is read-only; repairs are new operations with their own ticket and receipt |
| `REQ-VERIFY-003` | Pipeline order `observe → validate → render → verify → reconcile` precedes every receipt (DEC-046) |
| `REQ-VERIFY-004` | Receipts state what ran, what was skipped, and why (DEC-022) |
| `REQ-VERIFY-005` | Verification records are stored once; receipts reference them (CTR-018) |
| `REQ-VERIFY-006` | Hook contract: idempotent, bounded, read-only, typed failures |
| `REQ-VERIFY-007` | Missing hook → risk-class default with the gap recorded (EDGE-160) |
| `REQ-VERIFY-008` | Deterministic validators lead; model/vision only where necessary and consent-gated |
| `REQ-VERIFY-009` | Reconciliation records `pass`/`fail`/`partial`; failures route — never silent retries |
| `REQ-VERIFY-010` | Verify–repair cycles are bounded; no progress escalates (EDGE-163) |
| `REQ-VERIFY-011` | Postconditions are declared per operation and compared; partial batches are explicit |
| `REQ-VERIFY-012` | Verification runs emit `verification.*` events (INV-23) |
| `REQ-VERIFY-013` | Verifier crash/absence marks the effect `unverified`; dangerous classes block or ask (EDGE-160) |
