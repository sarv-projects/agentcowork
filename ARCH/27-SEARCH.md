# 27 — Search

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-SEARCH-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** **one** search service over all context sources. Deterministic retrieval — **never an LLM call** (DEC-015). `27` is the one Core-side search implementation and indexing surface (the baseline's "kernel search" names this service, not module `10`); other modules register index adapters, and `16`'s `context.search` (`CTR-006`) is the assembly-facing façade over it.
> **Dependencies:** source owners (`17` memory · `21` world · `25` files · `26` repo · `29` artifacts · `30` events) · `12-TRUST` (scope/sensitivity). **Consumers:** `16` (retrieval), `15` (agent queries), UI (global search), `32` (external-agent projection).
> **Evidence:** repo principle (one Core search implementation — `agentcowork-search`; `AGENTS.md` §12's "Kernel search" names the Rust crate workspace, not module `10`) · product-owner brief (search/file-index rows are explicitly token-free) · `ARCH/16-CONTEXT.md` §1, `ARCH/17-MEMORY.md` §6, `ARCH/21-WORLD-MODEL.md` §4.

## 1. Purpose & rules

**Owns:** the unified search surface (query → ranked results across sources) · per-source index adapters · ranking & result-shaping policy · scope + sensitivity filtering at query time · the result citation format (refs + bounded snippets).
**Never owns:** source data (each owner owns its store) · context selection (`16` consumes results) · answer synthesis (the agent’s job).

1. **One implementation** — a single Core search service (this module); sidecar/UI and context assembly call it through contracts (`context.search` is the assembly façade, `CTR-006`). No second search path anywhere.
2. **Deterministic first** — lexical/structured queries; semantic retrieval is deferred behind a measured trigger (`16` §11).
3. **Scoped by construction** — every query carries scopes + a sensitivity ceiling; cross-project leakage is a defect (INV-10).
4. **Refs, not copies** — results are references with bounded snippets; resolving content is a separate, permission-checked read.

## 2. Sources & adapters

| Source | Adapter | Query powers |
|---|---|---|
| Files (metadata) | `25` index | lexical (names/paths) + structured filters (type/time/size) |
| Files/Library items (content) | Scoped extraction adapters from `22`–`24`/`29`, indexed here | Full text + exact version/page/cell/slide/line/time anchors where extraction supports them |
| Memory | `17` FTS5 | lexical + scope/kind filters (`17` §6) |
| Artifacts | `29` | name/type/provenance filters |
| World objects | `21` registries | kind/attribute filters (apps · windows · tabs · processes) |
| Repo symbols | `26` RepoGraph | **structural queries delegate to `26`** (definitions/callers/imports); `27` does not re-index code |
| Events | `30` | operator-style filtered reads (bounded) |

Adapters declare: supported query forms · freshness semantics · cost class. `27` composes; it never bypasses an owner’s store.

### 2.1 Exact-version content indexing for Library and files (DEC-055 target)

Files (`25`) supplies identity, observed version and access scope; Artifacts (`29`) supplies immutable artifact versions, Library origin and dependency status. Format adapters extract bounded text/structure with source anchors and parser confidence; Search owns the single full-text index and query path. The index key includes source identity **and version**, extractor version, scope/permission epoch, content hash and anchor. A MIME sniff and size limit select the adapter; encrypted, corrupt, unsupported or unreadable files remain metadata-only with a visible reason. OCR/transcription is a separate, consented, metered extraction step and is never silently invoked by a model-free search query.

On content change, index a new version and atomically switch the current-version pointer after extraction succeeds; older versions remain queryable only when retained and authorized. On delete, scope revocation or permission change, invalidate/purge affected postings before a new query can return them; cached search/context results re-check authorization and exact version at dereference. Deduplication may share extraction work but never leak a snippet or hit count across security scopes. Parser failure or partial OCR yields `partial` coverage and confidence, never fabricated content. Results carry an exact-version ref plus page/cell/slide/line/time anchor where available, extraction status and freshness. `16` may retrieve the chosen content through its own permission-checked `context.get`; Search does not synthesize an answer.

## 3. Query model

- **Forms:** lexical (BM25) · structured filters (scope/kind/time/type/attribute) · exact (id/ref lookup) · structural (delegated to `26`).
- **Composition:** a query may mix forms; per-source results are normalized and merged by **source priority + recency** in v1 (fusion/RRF deferred until multi-source quality proves the need — mirror of memory U6).
- **Budget:** result count + snippet size are capped; zero results is a valid outcome (no padding).

## 4. Ranking & result shaping

v1 ranking: a **non-negative relevance base** — for FTS5, `relevance = −bm25` because raw `bm25()` is negative (better matches are more negative) — then × recency × pin/priority boosts, sorted **descending** with deterministic tie-breaks; a boost never inverts relevance (`17` §6). Every result carries: `ref` · `source` · `score` · `freshness` · `snippet` (bounded) · `sensitivity`. **Abstention is correct behavior** — “no relevant memory” must yield nothing (`17` §6). **Path ownership (C-09):** memory's injection path is `memory.recall` (`17` §6) with exactly one scoring owner; `27` fronts memory as a **user/agent search source** returning refs + bounded snippets and never re-ranks the recall candidates.

## 5. Scoping & security

- Scope filters (workspace/project/agent/session) and sensitivity ceilings are applied **at query time** using the caller’s policy snapshot (`12`).
- External agents receive a filtered search projection (own project + granted scopes only; `32`).
- Filtering is **not** un-discovery: out-of-scope sources are never queried for that caller.

## 6. Performance & costs

Local indexes only; no network in the search path; no model calls. Targets (declared, measured in `42`): metadata search p95 ≤ 50 ms at 100k files; memory recall target owned by `17`; result shaping bounded by budget. Index freshness follows each source’s update cadence (`21` §4).

## 7. Failure modes

| Failure | Behavior |
|---|---|
| Source index missing/stale | Freshness flag on results; degraded notice; never silent wrong answers. |
| Over-broad query | Bounded by caps + scopes; suggestion to narrow (guidance). |
| Sensitive hit for lower clearance | Filtered before scoring (structural test, `41`). |
| Adapter error | Partial results + typed error path; other sources unaffected. |

## 8. Interop

**Depends on:** `10` · `12` · source owners (`17`, `21`, `25`, `26`, `29`, `30`).
**Exposes to:** `16` (retrieval), `15`/`20` (queries), UI (global search), `32` (projection).
**DAG check:** `27` reads indexes owned by others; it never writes source state.

## 9. Not in v1

Semantic/vector search (trigger: measured lexical misses) · cross-repository federation · personalized ranking · web search (that is a capability, `28`/`14`, not the local search plane — specified in `28` §3 as `web.search`/`web.fetch`, `DEC-037`). Exact-version content indexing is the target in §2.1, not a second file index.

## 10. Open questions (`OQ-SRCH-*`)

1. Per-source p95 targets and their measurement harness.
2. Fusion/RRF trigger thresholds (when source-priority stops being enough).
3. Extraction adapter coverage and OCR/transcription cost/consent thresholds for exact-version content indexing (§2.1).
4. Global-search UI scope (single bar vs per-surface filters) — ties `AGENTCOWORK-UI.md`.

## 11. Evidence

Repo principle: one Core search implementation (`agentcowork-search`; `AGENTS.md` §12) · product-owner brief (search/files-index rows explicitly zero-token) · `ARCH/16-CONTEXT.md` §1/§4 · `ARCH/17-MEMORY.md` §6 (non-negative relevance, abstention) · `ARCH/21-WORLD-MODEL.md` §4 (index-not-walk) · `ARCH/26-CODE.md` §5 (structural queries owned by `26`).

## 12. Requirements (`REQ-SEARCH-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-SEARCH-001` | One Core search implementation (this module), consumed through contracts; `context.search` (`CTR-006`) is the assembly-facing façade — no second path, index or module-local search |
| `REQ-SEARCH-002` | Deterministic, model-free, network-free queries (DEC-015, INV-13) |
| `REQ-SEARCH-003` | Scopes + sensitivity ceiling applied before querying; out-of-scope sources are never touched (INV-10) |
| `REQ-SEARCH-004` | External agents get a filtered projection — own project + granted scopes, deny-by-default (DEC-009, INV-11) |
| `REQ-SEARCH-005` | Source adapters declare query forms/freshness/cost; search composes them, never bypasses an owner's store; code queries delegate to `26` |
| `REQ-SEARCH-006` | Lexical · structured · exact forms compose; v1 merges by source priority + recency with deterministic ties (fusion deferred) |
| `REQ-SEARCH-007` | Canonical result shape (`ref`/`source`/`score`/`freshness`/`snippet`/`sensitivity`); abstention returns nothing, never padding |
| `REQ-SEARCH-008` | Ranking = source score × recency × pin boosts, deterministic tie-breaks; no hidden personalization in v1 |
| `REQ-SEARCH-009` | Result count and snippets are capped; over-broad queries receive narrowing guidance |
| `REQ-SEARCH-010` | NFR: metadata search p95 ≤ 50 ms at 100k files, local indexes only |
| `REQ-SEARCH-011` | Stale indexes and adapter failures are surfaced (freshness flag + partial results + typed error), never silent |
| `REQ-SEARCH-012` | Responses carry refs + bounded snippets only — search never synthesizes answers or resolves content without a permission check |
| `REQ-SEARCH-013` | Exact-version file/Library content index with scoped extraction, source anchors/confidence, atomic reindex, revocation purge and permission-checked dereference (DEC-055) |
