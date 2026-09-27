# 18 — Model Plane (registry · routing · adapters)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-MODEL-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** every model — cloud or local — behind **one registry and one router**. No module hard-codes a vendor (P-03, DEC-004).
> **Dependencies:** `10-KERNEL` · `12-TRUST` (vault) · `16-CONTEXT` (window/tokenization feeds budgets) · `30-EVENTS` (usage). **Consumers:** `15-AGENT-PLANE` · `20-WORKFLOW` (agent nodes) · `17-MEMORY` (extractor) · `24-COMPUTER-USE` (vision).
> **Evidence:** product-owner brief (ModelAdapter surface, local discovery UX, “never hard-code Claude”) · `ARCHIVE/v1-research/agent-harness-verification.md` §A1 (resolved window + feasibility check, `codex-rs/core/src/session/mod.rs:4560-4587`), §C1 (budget vocabulary) · `ARCH/06-DATA-MODEL.md` DM-025 · `ARCH/07-CONTRACTS.md` CTR-014 · `ARCHIVE/v1-research/memory.md` §7 (extraction disclosure boundary).

## 1. Purpose & responsibilities

**Owns:** the model registry (DM-025) · model adapters (implementation of CTR-014) · the router · capability normalization (context window · tool calling · reasoning · vision · streaming · structured output) · local discovery (Ollama · LM Studio · vLLM · llama.cpp · OpenAI-compatible endpoints) · reasoning-effort mapping · usage/cost accounting hooks.
**Never owns:** agent logic (`15`) · credentials (`12` owns; adapters `use` them via the vault).

## 2. ModelDescriptor (DM-025)

| Field | Meaning |
|---|---|
| `id` | `provider/model` (stable registry identity) |
| `provider` | serving provider (cloud endpoint or local server) |
| `context_window` / `max_output` | resolved limits feeding `16` budget arithmetic |
| `tool_calling` | none · basic · parallel |
| `reasoning_modes[]` | which normalized effort levels the model supports |
| `vision` | image input support (feeds `24`) |
| `streaming` · `structured_output` | transport/normalization flags |
| `cost {in, out}` | catalog values; estimates only |
| `latency_class` | local · fast · standard · slow (ranking input) |
| `locality` | `local` · `cloud` (privacy-relevant flag) |
| `tokenizer` | tokenizer ref used for budget estimation |

**Catalog sources:** cloud catalog data (models.dev-class; *data, not runtime* — provider SDKs are never installed at runtime, DEC-034) + local discovery + manual entries. Offline behavior: vendored snapshot + local scan; no model available ⇒ typed `Unavailable` with guidance.

**Descriptor additions (absorbed, A2/A3/A5/A6/A9/A10):** `status` (lifecycle `alpha|beta|deprecated|active` + visibility: whitelist/blacklist, experimental flag) · `family` + `release_date` · `variants` (named per-model option maps; config-mergeable; `disabled` removals) · per-model `options`/`headers` · `transport_ref` + `catalog_ref` (catalog key ≠ generated id ≠ runtime transport id) · `prompt_cache` (inferred from cache-cost fields) · `cost` extended to `{ in, out, cache_read, cache_write, tiers[]?, over_200k?, reported_actual? }` — cache-class-aware; provider-reported actual overrides catalog estimates; included plans are exactly 0 · `privacy` (training-use + retention class from the provider's disclosure — `confidential` scopes must not route to training-enabled models; e.g. the OpenCode Go privacy table).

**Catalog refresh discipline (absorbed, A4):** short TTL (≈5 min) · atomic `tmp`+rename writes · cross-process lock · compiled/vendored snapshot fallback (offline) · scheduled refresh (≈60 min) emitting a refresh event · refresh failures logged and swallowed (never block the UI). Resolves the cadence half of `OQ-MODEL-01`; the **data license** of the catalog source remains open (`44` §5).

## 3. Router (CTR-014)

```
resolve(preferences, constraints) → ModelSelection
```

Inputs: agent-profile default · session override · task requirements (vision? tools? reasoning? context size?) · policy (models/providers allowed per scope, `12`) · availability/health · preference weights (cost · latency · locality). Ranking is deterministic and audited; a fallback chain is declared per selection. Degrade rules are explicit: if a requirement cannot be met (e.g. vision needed, none available), the result is `GuidanceRequired`/`RequiresUserAction` — never a silent capability downgrade.

The resolved window feeds the `16` pre-turn feasibility check (`window − reserves`); router and context share the same constants.

## 4. Adapters & auth

- **Adapter kinds:** cloud-native shapes + OpenAI-compatible; local servers (probe + list); embedded runtimes later.
- **Auth:** vault `use`-style references only (CTR-013, INV-02); endpoint config carries secret *refs*; keys never appear in logs, prompts, or telemetry.
- **Streaming:** one typed chunk vocabulary shared with `15` §4 (message deltas · tool-call deltas · usage · errors); cancellation and backpressure are mandatory adapter behaviors.
- **Streaming contract (A1):** one typed stream-event union above the adapter — `step-start · text-start/delta/end · reasoning-start/delta/end · tool-input-start/delta/end · tool-call · tool-result · tool-error · step-finish · finish · provider-error` — explicit block ids (synthesized when absent); step-finish vs turn-finish distinct; consumers never branch on provider id.
- **Watchdogs (A13):** header timeout · chunk/idle timeout · read timeout are explicit abort reasons; a network-error finish fails the step rather than silently ending it.
- **Retries (A7 — single-owner rule):** request-start transport retries (exponential + jitter; honors `retry-after` in seconds/ms/HTTP-date) · **pre-content** stream interruptions via buffer-until-proven (discarded-attempt usage is summed, never lost; a user abort anywhere vetoes retry) · **post-content** failures handled at the turn level. Exactly one layer retries per failure class; context overflow is terminal **at this layer** — the adapter surfaces the typed `ContextOverflow` and never re-requests; the agent's turn-level recovery owns the single compact-and-retry of the same step (`16` §4) and never becomes a second transport retry. Provider-native compaction, when a provider exposes it, is adopted per DEC-045 — a capability event with Guard egress/audit and a per-provider off switch; its usage/cost counts in §7 as a second inference call.
- **Typed provider errors (A8):** `InvalidRequest · Authentication · RateLimit{retryAfterMs} · QuotaExceeded · ContentPolicy · ProviderInternal · Transport · ContextOverflow`; `retryable` derived from the type (typed-first, structural-walk-second classification).
- **Schema lowering (A12):** tool JSON schemas are lowered per wire protocol inside the adapter — callers keep one schema shape.
- **Prompt cache (A9/G5):** cache hints are protocol-scoped — automatic breakpoints (last tool · last system part · latest user message) only for protocols with inline cache markers, TTL hints where supported; cache reads/writes are accounted per §7. Policy defaults remain open (OQ-MODEL-01, joint with `16`).
- **Idempotency:** non-stream requests carry idempotency keys.
- **Provider client identity & session affinity (gateway class):** providers may require an identifying User-Agent (**ours** — never impersonation or a generic SDK name) and a stable per-conversation session header (`x-opencode-session` class) mapped from the logical session id (`DM-004`, `11` §2). Stability: one value per conversation — unchanged across turns, compaction and restarts; a new conversation ⇒ a new value; each subagent session is its own conversation. Provider traffic policies (typical coding-agent traffic; monitored for abuse) are conditions of enabling the provider (`12`/`44`) — no evasion. Worked example: **OpenCode Go** — one gateway, three wire protocols per model (`/v1/responses` · `/v1/chat/completions` · `/v1/messages`), ids `opencode-go/<model-id>` (`DEC-035`).

## 5. Reasoning mapping

Normalized dial: `auto · minimal · low · medium · high · extra_high` → provider-specific parameters (e.g. `reasoning_effort`; a local model's thinking budget). The model's descriptor declares which levels exist; the UI renders only supported levels (composer capability negotiation, `AGENTCOWORK-UI.md`). Raw chain-of-thought is **never** placed in the transcript — the user sees plan/status/tool activity (UI policy).

## 6. Local discovery & first-class locality

- Probe common local endpoints (Ollama, LM Studio, vLLM, llama.cpp, generic OpenAI-compatible), list models, health-check, and register them like any other provider.
- The common path requires no manual configuration (“pick a model and run”); manual entry exists as a fallback.
- Discovery results persist in the provider registry across restarts (no cold re-discovery on every start); reachability is reported as health (`registered → connected → degraded → down`, DM-013) — a registry row is never treated as reachable (A11).
- `locality: local` models carry a privacy guarantee: no egress leaves the machine (enforced by `12`).

## 7. Accounting

Usage events → `30` with the **usage contract (A2):** inclusive totals **plus** non-overlapping breakdown (`non_cached_input` · `cache_read` · `cache_write` · `reasoning`) with the invariant written down and values clamped — consumers never subtract (this removes the underflow bug class). Unnormalized provider fields ride along as an escape hatch for billing audits. **Cost (A3/A14):** cache-class-aware (read/write pricing · context-size tiers · >200k class) · provider-reported actual overrides catalog estimates · included/free plans are exactly 0. Latency · model id · work/session refs included. Per-work ceilings are enforced by `11` §5. **No prompt or completion content in telemetry.**

## 8. Failure modes

| Failure | Behavior |
|---|---|
| Provider/model down | Router failover per chain; backoff; typed `Unavailable`. |
| Window mismatch vs estimate | `16` feasibility path (compact/refuse before send). |
| Tool calling unsupported | Capability subsetting / agent adaptation (never silent tool emulation). |
| Rate limits | Backoff + queue; surfaced; per-work budget protects cost. |
| Local server version drift | Health re-probe; descriptor refresh; degraded marking. |
| Cost surprise | Ceilings pause work and surface (no silent overrun). |
| Reasoning param unsupported | Descriptor declares support; unsupported levels are not offered. |
| Stream stalls (idle/read watchdog) | Typed abort reason; step retried per the single-owner rule; never a silent hang. |
| Auth token expires mid-turn | Typed `Authentication`; one refresh attempt where supported; else re-auth guidance — never a silent failover. |
| Provider-reported cost mismatch | Actual overrides estimate; mismatch logged as an event for audit. |

## 9. Interop

**Depends on:** `10` · `12` (vault/policy) · `16` (constants/tokenization) · `30` (events).
**Exposes to:** `15` · `17` (extractor selection — disclosure policy and kill switches per DEC-044) · `20` · `24` (vision models) · UI (model picker via `32`).
**DAG check:** the model plane never calls agents; it serves them.

## 10. Open questions (`OQ-MODEL-*`)

1. ~~Catalog sync policy~~ → resolved by the absorbed refresh discipline (§2); remaining opens: catalog **data license** (`44` §5) · prompt-cache policy defaults (joint `16`↔`18`) · cross-provider failover invalidation semantics (cache breakpoints · signed reasoning blocks · in-flight tool-call ids).
2. Reasoning-param behavior on models that partially support levels (ignore vs map vs error).
3. Tokenizer strategy details (shared with OQ-CTX-03): per-provider tokenizers vs conservative estimates.
4. Which local models qualify for the vision rung (`24` decision).
5. Embedding-model support: register now for future memory retrieval (U1) or later?
6. Multi-modal input scope for v1 (images for vision; audio/video later).
7. Default-model and recent-use policy, and the small utility model used for extraction, summaries and titles (selection policy; the extractor's disclosure default is fixed by DEC-044).

## 11. Evidence

**Wave-2 addition:** `ARCHIVE/v1-research/provider-layer-absorption.md` (OpenCode `fe3f3a4` · Cline `254f40c` · pinned) — absorb list A1–A14, gaps G1–G16, rejects R1–R5; behavior changes locked as `DEC-034`.

Product-owner brief (ModelAdapter surface, local model UX, model independence) · `agent-harness-verification.md` §A1 (resolved window; pre-turn feasibility; anchors `codex-rs/core/src/session/mod.rs:4560-4587`), §C1 (keep/buffer/reserve vocabulary) · `ARCH/06-DATA-MODEL.md` DM-025 · `ARCH/07-CONTRACTS.md` CTR-014 · `ARCH/16-CONTEXT.md` §3 · `ARCHIVE/v1-research/memory.md` §7 (extraction model = disclosure boundary).

## 12. Requirements (`REQ-MODEL-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-MODEL-001` | One registry, one router; no module hard-codes a vendor (P-03, DEC-004) |
| `REQ-MODEL-002` | ModelDescriptor contract (`DM-025`) — declared capabilities only, never offered beyond them |
| `REQ-MODEL-003` | Catalog is data: TTL + atomic write + lock + vendored snapshot + scheduled refresh; no runtime SDK installs (DEC-034) |
| `REQ-MODEL-004` | Deterministic, audited routing with declared fallback chains; unmet requirements yield guidance, never a silent downgrade (CTR-014) |
| `REQ-MODEL-005` | Vault-only credentials in adapters; keys never in logs, prompts or telemetry (INV-02) |
| `REQ-MODEL-006` | One typed stream union above adapters; consumers never branch on provider id (DEC-034) |
| `REQ-MODEL-007` | Usage/cost invariant: inclusive totals + non-overlapping breakdown, never subtract; actual overrides estimate; no content telemetry |
| `REQ-MODEL-008` | Single-owner retry discipline; user abort vetoes retry; context overflow is terminal (DEC-034) |
| `REQ-MODEL-009` | Watchdogs abort with explicit reasons; typed provider-error taxonomy with derived retryability |
| `REQ-MODEL-010` | Local discovery first-class; `locality: local` means zero egress (INV-05) |
| `REQ-MODEL-011` | Reasoning-effort dial maps normalized levels to provider parameters; unsupported levels are never offered |
| `REQ-MODEL-012` | Resolved window + reserves feed the context pre-turn feasibility check from shared constants (DEC-027) |
