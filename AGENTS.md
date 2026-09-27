# Repository Agent Instructions

This file is intentionally **agent-agnostic**. Treat it as the durable contract for any coding agent working in this repository.

## 1. Mission

Before changing code, understand the repository's architecture, local conventions, relevant tests, and dependency relationships. Prefer evidence from source, tests, configuration, Git history, and structural indexes over assumptions.

For repository-wide or unfamiliar work, use the `codebase-intelligence` skill when available. Build/update AST and code-graph indexes rather than producing hand-written ASCII dependency maps.

## 2. Instruction hierarchy

- Follow this file for repository-wide behavior.
- Respect more-specific `AGENTS.md` files in subdirectories when supported.
- Follow the project's existing build, test, lint, formatting, and release conventions.
- Do not silently invent new conventions when an existing one is documented in the repository.

## 3. Understand before modifying

Before substantial edits:

1. locate the repository root;
2. inspect Git status and the current branch/worktree;
3. read this file and any relevant nested instruction files;
4. find the implementation, callers/dependents, configuration, and tests;
5. use structural code navigation when a relationship is non-trivial;
6. state assumptions internally and verify them from code where possible.

Do not browse the entire repository indiscriminately. Narrow the context using symbols, dependency graphs, search, and focused file reads.

## 4. Code intelligence

For architectural work, refactors, debugging, migrations, or unfamiliar areas:

- prefer Tree-sitter/AST-based extraction over regex-only structural analysis;
- use an incremental local index;
- preserve file hashes and update only changed files;
- use SQLite for durable local index state;
- use NetworkX or another local graph library for dependency analysis;
- use LSP/SCIP/compiler-backed resolution when available for precise definitions/references;
- distinguish compiler-grade edges from heuristics;
- never present an inferred relationship as certain evidence.

Never create ASCII-art dependency maps as the source of truth. Use structured graph data, Mermaid, DOT, or generated visualizations instead.

## 5. Editing rules

Make the smallest coherent change that solves the problem. Preserve existing public behavior unless the task requires a behavior change.

Before introducing a new dependency:

- check whether the repository already has an equivalent;
- consider maintenance and portability;
- document why the dependency is needed.

Do not modify generated files manually unless the repository explicitly requires it; update the source/template and regenerate them.

- **Archive rule (2026-09-22; v1 note 2026-09-26):** no new work may land in the archived v0 tree (`ARCHIVE/v0/ARCH/archive/`) or name archived modules as owners (e.g. the coordinator loop under `ARCHIVE/v0/ARCH/archive/coordinator-loop/`) — re-home to the live owner instead: the Rust context passport (`src-tauri/src/acp_cmds.rs`) for prompt/context work, `agentcowork-acp` for per-agent behavior, `agentcowork-mcp` for tool-surface work, and Work/AUTOMATION + the bound agent for coordination.

## 6. Validation

After changes:

1. run targeted tests first;
2. run formatting/lint/type checks relevant to touched code;
3. run broader tests when practical;
4. inspect the final diff;
5. verify no unintended files changed;
6. verify secrets, credentials, generated caches, and local indexes are not being committed accidentally.

If a check cannot run, record the exact reason rather than pretending it passed.

## 7. Git discipline

Git is part of the development workflow, not an optional final step.

After a meaningful completed change:

```bash
git status --short
git diff --check
git add <intended-files>
git commit -m "clear description of the change"
```

Always review what is staged before committing. Do not use `git add .` blindly when the repository contains unrelated work; stage the intended files explicitly.

Do not rewrite, squash, reset, or discard unrelated user work without explicit instruction.

Commit messages should describe the software change, not the tool, model, assistant, IDE, or agent used to make it.

**Agent names that must NEVER appear in commit messages, code comments, implementation notes, generated files, or documentation:**

- CodeBuff
- FreeBuff
- Claude Code
- Cursor
- Vibe
- Codex
- Grok
- Aider
- Copilot
- Windsurf
- Roo
- Cline
- Amp
- Any other AI tool or model name

The same rule applies to future tools with the same role: keep the repository history about the code and engineering decision, not about which agent happened to perform the edit.

**Scope of the rule:** it targets *authoring-tool attribution* — naming the tool, model or agent that produced an edit. It does not forbid citing named external systems as prior art or absorbed technology: architecture and evidence docs may name studied products (e.g. Codex, Grok Build, Cline, OpenCode) when the reference is about that software, never about who wrote the change.

## 8. Skill usage

When a reusable workflow exists, prefer the corresponding skill instead of recreating the procedure from scratch.

The `skill-creator` skill is the standard workflow for creating or evolving reusable skills. Keep reusable skills portable, versioned, and independent of a particular model or coding client.

### Store process entries (2026-09-22)

- **Dual-license pattern for the skill store:** skill-store **code** (scripts, tool definitions, executable parts of a skill) is MIT; skill-store **content** (`SKILL.md` prose, examples, reference material) is CC0. Contributions to the store follow this split and declare it in the skill's manifest.
- **Lazy-consensus window for store governance:** proposals that change the skill store (new or changed skills, registry schema, admission rules) are adopted by lazy consensus — the written proposal must be visible in the repository for a **7-day** review window before merge; no blocking objection inside the window counts as consensus, and any objection forces explicit resolution before adoption.

## 9. Communication

At the end of a substantial task, report:

- what changed;
- what was validated;
- any remaining uncertainty or skipped checks.

Keep descriptions factual and tied to the repository. Do not add marketing language about the development tool used.

---

# AgentCowork — Project-Specific Instructions

> The sections below are specific to this repository. Sections 1–9 above are universal.

## 10. Architecture

> **v1 docs (2026-09-26).** The architecture was rebuilt from scratch; the v0 corpus is archived locally at `ARCHIVE/v0/` (git-ignored). Authority: [`AGENTCOWORK-SPEC.md`](AGENTCOWORK-SPEC.md) (WHAT) → [`ARCH/08-REQUIREMENTS.md`](ARCH/08-REQUIREMENTS.md) (testable behaviors) → [`ARCH/03-HLD.md`](ARCH/03-HLD.md) (HOW) → module docs; the door is [`ARCH/00-INDEX.md`](ARCH/00-INDEX.md). Working names: product **AgentCowork**, runtime **Core** ([`ARCH/01-NAMING.md`](ARCH/01-NAMING.md)). Delivery status: [`TODO.md`](TODO.md) — the v1 docs are frozen (2026-09-26); implementation proceeds spec-driven (W0–W4).
>
> **This section is an orientation summary only** — where it disagrees with the v1 set, the v1 set wins. The invariant list lives in [`ARCH/05-INVARIANTS.md`](ARCH/05-INVARIANTS.md); do not fork it here.

The runtime's four deployment layers (a convenience view for orientation, not
the contract):

```
L4  COCKPIT           ui/ — React 19 + Zustand 5 + Tailwind 4
        ↓ Tauri IPC: nativeCall("<cmd>", args)
L3  Tauri Shell        src-tauri/ — thin Rust shell, 42 *_cmds.rs modules
        ↓ direct Rust calls
L2  Rust Kernel        crates/agentcowork-* — guard/vault/audit/office/browser
        ↓ stdio JSON-RPC 2.0, [u32 LE len][JSON] framing
L1  Bun Sidecar        packages/coordinator — shared-plane services a turn calls into (not reasoning)
        ↓ ACP/MCP/CDP
L0  External Agents    peer agents (DEC-010), MCP servers, browsers
```

### The One Invariant (summary)

**Surfaces propose; Core disposes.** Every externally visible mutating effect
requires an authorization ticket minted in Core; local persistent mutations
(in-store memory writes, DEC-042) are policy-gated and audited. Provider
credentials never leave the vault ([`INV-01`](ARCH/05-INVARIANTS.md),
[`INV-02`](ARCH/05-INVARIANTS.md)).
The full invariant set lives in [`ARCH/05-INVARIANTS.md`](ARCH/05-INVARIANTS.md).

## 11. Development Commands

```bash
# NOTE: the Rust workspace manifest is `crates/Cargo.toml` — cargo commands
# must run from `crates/` (CI sets `working-directory: crates`).

# Build (requires: Rust 1.98+, Node 22+, Bun, pnpm 11+)
(cd crates && cargo build)        # Rust kernel
pnpm install                      # JS workspace
pnpm --filter @agentcowork/coordinator build  # Sidecar

# Test
(cd crates && cargo test)                    # All Rust tests
(cd crates && cargo test -p agentcowork-core)  # Single crate
pnpm -r test                      # All JS/TS tests (recursive; skips packages without a test script)

# Typecheck
(cd crates && cargo clippy)       # Rust lint
pnpm --filter ui tsc --noEmit     # UI typecheck

# Code Graph (AST-based analysis)
. .venv/bin/activate
python3 .agents/skills/codebase-intelligence/scripts/codegraph.py index
python3 .agents/skills/codebase-intelligence/scripts/codegraph.py report
python3 .agents/skills/codebase-intelligence/scripts/codegraph.py query "symbol_name"
python3 .agents/skills/codebase-intelligence/scripts/codegraph.py path <from> <to>
python3 .agents/skills/codebase-intelligence/scripts/codegraph.py stats
python3 .agents/skills/codebase-intelligence/scripts/codegraph.py export --format graphml

# Architecture-invariant gate (no TS credential custody, one authorization
# decider, one auth vocabulary, one canonical schema)
node scripts/check-arch-invariants.mjs
node scripts/ipc-parity.mjs --md             # UI ↔ Tauri command parity
```

**Staleness checks:** `node scripts/check-doc-refs.mjs` and
`node scripts/check-doc-sync.mjs` run in CI; re-run them after substantial
structural changes (new files, moved modules, renamed crates) so the doc set
stays consistent. (`check-doc-sync` still reads the archived v0 capability
chain and is queued for re-homing.)

See `.agents/README.md` for the agent kit layout, `.agents/docs/` for the understanding
protocol and provider matrix, and `.agents/skills/codebase-intelligence/SKILL.md` for the
skill's routing contract.

## 12. File Structure

```
crates/                          # 21 workspace members (the kernel; agentcowork-engine
                                  #   was deleted 2026-09-23 — TODO P72 — do not re-add)
  agentcowork-core/                #   Orchestrator: supervisor, worktrees, CUA, tools
  agentcowork-ipc/                 #   stdio JSON-RPC 2.0 framing (transport only)
  agentcowork-guard/               #   Guard-1/2: netfloor, pathfloor, tickets, sandboxes
  agentcowork-audit/               #   Append-only tamper-evident audit trail
  agentcowork-vault/               #   Encrypted key vault
  agentcowork-memory/              #   RRF fusion, ACT-R, compaction, graph
  agentcowork-blueprint/           #   Task DAG, checkpoints, skill store, subagents
  agentcowork-types/               #   Canonical schema + id newtypes (AuthMode, AgentBinding)
  agentcowork-browser/             #   a11y snapshot, refs, actions, CDP
  agentcowork-cdp/                 #   CDP wire backend (under agentcowork-browser)
  agentcowork-catalog/             #   models.dev sync, provider seed, routing
  agentcowork-storage/             #   Work-stealing walker, dedup, FTS5
  agentcowork-codeintel/           #   LSP, SCIP, repo-map PageRank
  agentcowork-office/              #   IronCalc XLSX, OOXML patchers
  agentcowork-desktop/             #   Desktop automation (CUA)
  agentcowork-acp/                 #   Agent Communication Protocol + prefix guard
  agentcowork-mcp/                 #   MCP server/client (19 shared façades over 51 native tools)
  agentcowork-agents/              #   Agent plane primitives
  agentcowork-search/              #   Kernel search (the one implementation)
  agentcowork-script/              #   Sandboxed script runner
  agentcowork-eval/                #   Eval harness (never a runtime dependency)

packages/                        # 10 TypeScript packages (the sidecar)
  coordinator/                   #   Shared plane services a turn calls into
                                 #   (memory/guard/work/skills/MCP/connectors) — no turn
                                 #   loop: the bound external agent owns the loop
                                 #   (DEC-010; the old loop is archived at
                                 #   ARCHIVE/v0/ARCH/archive/coordinator-loop/)
  core-ai/                       #   AI runtime, streaming, retrieval
  core-providers/                #   Provider management (registry/routing; custody is the vault)
  core-agents/                   #   Agent directory projections
  core-domain/                   #   Shared domain projections
  core-tools/                    #   Tool definitions
  core-memory/                   #   Memory system
  core-connectors/               #   Connector framework
  core-search/                   #   Search projection (kernel owns search)
  core-security/                 #   Security projections (no custody — CRED-2)

ui/                              # React 19 SPA (the cockpit)
src-tauri/                       # Tauri v2 shell (thin Rust layer)
ARCH/                            # v1 architecture docs (door: ARCH/00-INDEX.md);
                                 #   product contract: AGENTCOWORK-SPEC.md (repo root)
deploy/                          # Docker, systemd, launchd, Fly.io
scripts/                         # CI gates, codegen, tools
```

## 13. Coding Conventions

### Rust
- Edition 2024 (`edition = "2024"`)
- Public items get `///` doc comments with a first-sentence summary
- Modules get `//!` module-level docs
- Use `anyhow::Result` for error propagation, `thiserror` for custom errors
- Tests live in `#[cfg(test)] mod tests` at the bottom of the file, or in `tests/` for integration tests
- Prefer `tokio` async runtime

### TypeScript
- Strict mode enabled
- Use `interface` over `type` for object shapes
- Export named items (not default exports)
- Tests use Vitest (`describe`/`it`/`expect`)
- Prefer `const` over `let`

### IPC Contract
- Tauri commands: `#[tauri::command]` in `src-tauri/src/*_cmds.rs`
- UI calls: `nativeCall("<cmd>", args)` — **not** raw `invoke()`
- Protocol version: `1` (both sides must match)

## 14. Testing Conventions

- Rust: `cargo test` runs all unit + integration tests
- TS: `pnpm -r test` runs the workspace JS/TS suites (Vitest per package; the coordinator uses `bun test`)
- Integration tests in `crates/*/tests/` use `acceptance_*` prefix
- UI tests in `ui/src/**/*.test.tsx` use DOM testing library
- Security tests: `scripts/e2e/security-gate.mjs`
- Live tests require env var: `AGENTCOWORK_LIVE_TEST=1`
- CI runs all tests on every PR; no merging with failing tests

## 15. Security Rules

- Provider API keys live ONLY in the Rust vault (`agentcowork-vault`)
- The sidecar NEVER holds credentials
- All outbound network goes through Guard-2 (`agentcowork-guard`)
- Path traversal is blocked by `pathfloor` (Guard-2)
- SSRF is blocked by `netfloor` (Guard-2)
- Sandboxed execution via `sandbox` (Guard-2)
- Audit trail: every mutating operation is logged to `agentcowork-audit`

## 16. Spec-driven development (SDD)

Development here is spec-driven: requirements are testable contracts and code exists
to satisfy them.

- **The chain.** `REQ-*` (behavioral requirements, [`ARCH/08-REQUIREMENTS.md`](ARCH/08-REQUIREMENTS.md))
  derive from [`AGENTCOWORK-SPEC.md`](AGENTCOWORK-SPEC.md); designs cite them
  (`DEC/DM/CTR`); the plan ([`TODO.md`](TODO.md)) turns them into `TASK-*` units; tests
  (`TEST-*`) verify them; [`ARCH/09-FEATURE-MATRIX.md`](ARCH/09-FEATURE-MATRIX.md) maps
  the chain; [`ARCH/42-EVIDENCE-MAP.md`](ARCH/42-EVIDENCE-MAP.md) holds acceptance evidence.
- **The gate.** Before implementing: read the applicable specs → extract the `REQ-*` you
  will satisfy and the constraints they impose → inspect the current implementation →
  plan → obtain a decision for any architecture change → implement the smallest change →
  test every failure case → verify every acceptance criterion → report deviations.
- **Never silently change a spec.** If code and spec disagree, stop and emit `BLOCKED`
  with a proposed `DEC`/spec change; do not implement around the conflict.
- **IDs.** `REQ-*` / `TASK-*` / `TEST-*` are never renumbered or reused; doc status labels
  move `Planned → Draft Pn → Review Pn → Frozen` ([`ARCH/00-INDEX.md`](ARCH/00-INDEX.md) §6).
- **Commits.** Keep spec changes (`spec:` / `arch:`) separate from code (`feat:` / `fix:`)
  and tests (`test:`).
- **Protocol:** [`.agents/docs/spec-driven-development.md`](.agents/docs/spec-driven-development.md) ·
  template: [`.agents/templates/SPEC.template.md`](.agents/templates/SPEC.template.md).
