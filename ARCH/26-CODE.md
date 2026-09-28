# 26 — Code (Repository Intelligence & Execution)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-CODE-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the coding domain runtime — **repo understanding** (RepoGraph → RepoMap) + **code execution** (shell/tests/worktrees). This module produces structure and executes; the bound engine supplies the intelligence that uses it (`15`).
> **Dependencies:** `25-FILES` (identity/watchers) · `16-CONTEXT` (budgets/projections) · `19-RUNTIME-ENVIRONMENTS` (processes/worktrees) · `12-TRUST` (exec policy) · `29-ARTIFACTS` (outputs). **Consumers:** `15` (coder profile), `34` (verification).
> **Evidence:** product-owner brief (RepoGraph/RepoMap; edit→build→test loop) · `agent-harness-verification.md` §A2/§E1 (bounded fragments + baseline), §A3 (worktree session-bound vs per-spawn — DEC-029) · repo guidance (`.agents/skills/codebase-intelligence/SKILL.md`: tree-sitter/SQLite/graph, incremental hashing, “never present inferred edges as certain”) · local `agentcowork-codeintel` crate (read-only reference).

## 1. Purpose & rules

**Owns:** the RepoGraph index · the RepoMap projection · the LSP bridge · ripgrep-backed lexical search · git context · worktree provisioning + merge flow · test/build/lint execution wrappers · language detection.
**Never owns:** agent strategy (`15`) · context selection/budgeting (`16` owns the budget; `26` produces projections) · file identity (`25`) · exec policy (`12`).

1. **Structural parsing is first-class** — tree-sitter + LSP + ripgrep + git; the module picks the cheapest accurate answer per query.
2. **RepoMap is a projection under a token budget** — never the whole repository.
3. **Worktrees are per-spawn options** (DEC-029); merges are explicit review steps, never silent.
4. **Core-mediated code execution walks the governed path** — capabilities + environments + exec policy. A self-contained external coding agent may run native shell/tools under its own policy; its effects have separate provenance and are reconciled for Mission acceptance (DEC-054).
5. **Heuristic ≠ certain** — inferred graph edges are labeled as inferred (repo guidance).

## 2. RepoGraph (index model)

**Nodes:** File · Symbol (function/class/type/module/package) · Import · Reference · Call · Test · Config · Document · Command.
**Edges:** `imports` · `calls` · `extends/implements` · `references` · `tested_by` · `configured_by` · `generated_by` · `depends_on`.

- **Build:** incremental — file hashes; only changed files re-parse (watcher deltas from `25` W1); per-workspace SQLite store; tree-sitter grammars for syntax; LSP enrichment for precise edges where available.
- **Precision labels:** compiler/LSP-grade edges vs heuristic (tree-sitter/regex-level) edges are distinct fields; the UI/agent can tell which is which.
- **Freshness:** changed files reindex; stale edges flagged; queries prefer fresh subgraphs.
- Bounds: ignore rules (node_modules/vendor/build), size caps, and the per-language grammar/LSP sets declared for v1 (REQ-CODE-011).

## 3. RepoMap (projection)

- **Ranking:** dependency-graph centrality (Aider-style) + recency + task-relevance signals; deterministic given (graph, budget, task hints).
- **Budget:** from `16` §3 — the projection fits a declared token allowance; zero budget ⇒ zero map.
- **Granularity:** RepoMap serves the coarse context (project identity · repo map · relevant dirs/symbols); file/slice retrievals happen on demand as separate context items (`16` §2, §4).
- **Format:** compact, cache-friendly text — stable ordering for the stable prefix discipline (`16` §5); signatures + key refs, not bodies.
- **Injection:** fragments follow the bounded-fragment + baseline/delta pattern (verified §A2/§E1).

## 4. LSP bridge

Capabilities: `definitions` · `references` · `hover` · `diagnostics` · `symbols` · `rename` (policy-gated). Lifecycle per workspace: start/stop/health under `19`; graceful degradation to graph+search when an LSP is absent or crashed. Diagnostics are a context item type (`16` §2).

## 5. Search & retrieval

- **Lexical:** ripgrep integration — fast, bounded, path-scoped (`12`).
- **Structural:** graph queries (symbols, callers, imports, tests).
- **Semantic:** deferred (`16` §11 trigger: recall misses on paraphrase queries).
- Retrieval returns refs (`file:range`) + bounded excerpts — never whole-file dumps by default.

## 6. Git context & worktrees

- **Context:** status · diff · log (bounded) · targeted blame — feeds RepoMap ranking and context items; rendered for the UI as diffs (`32`).
- **Worktrees (DEC-029):** provisioned per-spawn when isolation is requested; branch strategy declared per task; worktree-isolated writers hold write leases on their checkout (`25` §6); merge = explicit step (review diff + tests) with receipts; abandoned worktrees are cleaned with a receipt.
- **Safety:** force-push/destructive operations are policy-gated (`12` §3); no silent rebase/reset.

## 7. Code execution

Wrappers over the capability plane: `code.run` · `code.test` · `code.build` · `code.lint` — each executes inside a declared environment (`19`) under exec policy (`12`), captures bounded output (full log → artifact, compact view → context per `16` §4), and routes long jobs to the background lane (`11`). The edit→build→test→diagnose loop is the coder profile's completion path (`15` §6).

## 8. Failure modes

| Failure | Behavior |
|---|---|
| Index corruption | Rebuild from source (incremental hashes make this bounded). |
| LSP crash/absent | Degrade to graph + ripgrep; diagnostics marked unavailable. |
| Worktree conflict | Queue / rebase / ask — never silent overwrite. |
| Flaky tests | Report with evidence; bounded retries only where declared; never a retry loop. |
| Huge repo | Incremental indexing + ignore rules + declared bounds; partial maps with freshness. |
| Generated files drift | `generated_by` edges surface provenance; edits to generated files flagged. |

## 9. Interop

**Depends on:** `10` · `12` (exec policy/paths) · `16` (budget/projection) · `19` (processes/worktrees) · `25` (identity/watchers) · `29` (artifacts).
**Exposes to:** `15` (retrieval + execution), `34` (verification: diffs/tests), UI (explorer/source view).
**DAG check:** `26` produces structure and executes declared commands; it never decides policy or assembles the model's context by itself.

## 10. Not in v1

Cross-repository graphs · remote devboxes · semantic/embedding retrieval · automatic merge automation beyond explicit review steps · SCIP ingestion beyond what the LSP bridge provides.

## 11. Open questions (`OQ-CODE-*`)

1. v1 language matrix (which tree-sitter grammars ship) and LSP set.
2. Indexing bounds + ignore defaults per stack.
3. Merge flow automation level (assist vs require explicit user merge).
4. Whether SCIP export is worth it for precise edges early.
5. RepoMap ranking weights tuning process (eval harness).

## 12. Evidence

Product-owner brief (RepoGraph/RepoMap, coding loop) · `agent-harness-verification.md` §A2/§E1 (fragments/baseline), §A3 (worktree models; DEC-029) · `.agents/skills/codebase-intelligence/SKILL.md` (tree-sitter, incremental hashing, SQLite, graph analysis; inference labeling) · local `agentcowork-codeintel` (reference only) · `ARCH/16-CONTEXT.md` §2–§5 · `ARCH/25-FILES.md` §2/§6.

## 13. Requirements (`REQ-CODE-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-CODE-001` | RepoGraph builds incrementally (hash-changed files only) into a per-workspace store; clean rebuild equals incremental (CTR-025) |
| `REQ-CODE-002` | Compiler/LSP-grade and heuristic edges are labeled distinctly; inferred relationships are never presented as certain |
| `REQ-CODE-003` | RepoMap is a bounded, deterministic signature projection (no bodies) — zero budget ⇒ zero map (DEC-027) |
| `REQ-CODE-004` | LSP bridge enriches definitions/references/diagnostics; absence/crash degrades to graph+search, never a silent empty answer |
| `REQ-CODE-005` | Lexical/structural retrieval returns `file:range` refs + bounded excerpts, path-scoped — not whole-file dumps |
| `REQ-CODE-006` | Worktrees are per-spawn options; merges and cleanup are explicit steps with receipts (DEC-029) |
| `REQ-CODE-007` | Destructive git operations (force-push, reset, rebase rewrite) are policy-gated with explicit approval |
| `REQ-CODE-008` | `code.run`/`test`/`build`/`lint` walk the governed path (capability + environment + exec policy + ticket) — no agent subprocess (INV-01/03) |
| `REQ-CODE-009` | Execution output is bounded: full log → artifact ref, compact view → context; long jobs go to the background lane (INV-22) |
| `REQ-CODE-010` | Index freshness follows `25` watcher deltas; stale edges are flagged, queries prefer fresh subgraphs |
| `REQ-CODE-011` | Ignore rules, size caps and the v1 tree-sitter/LSP language set are declared; unsupported languages degrade to lexical |
| `REQ-CODE-012` | Generated files carry `generated_by` provenance; direct edits to generated files are flagged |
