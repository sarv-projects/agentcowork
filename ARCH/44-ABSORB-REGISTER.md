# 44 — Absorb Register

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; cross-references verified.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Purpose:** the standing record of **what v1 absorbed from external systems, how, under what licensing, and what was explicitly rejected** — the operational ledger behind DEC-017.
> **Evidence base (surviving):** `~/business_Dev/REPO-COMPARE/` — `MASTER-COMPARISON.md` (329 lines; 190-item register measured against v0 — its §1 “186” line is stale per `DISPOSITION.md` §a), `DISPOSITION.md` (347 lines; item-level dispositions), `LICENSE-LEDGER.md` (100 lines; fresh-`LICENSE` verified), `BRIEFS/` (20 files), `clone2/` (55 clones) · `ARCHIVE/v1-research/` verification lanes (claim corrections).

## 0. Method & standing rules

**Three absorption levels** (DEC-017):

1. **Integrate directly** — only with cleared licensing (permissive-reuse set, §3).
2. **Reimplement the primitive** — study → redesign around our contracts → build natively (the default).
3. **Use as external provider** — MCP/ACP/API adapter; no code dependency at all.

**Standing rules:**

- **Pattern-read-only repos are never linked, copied, or vendored** — documentation-derived reimplementations only (`LICENSE-LEDGER` standing rule).
- **Fresh `LICENSE` beats any brief** — if a brief and the ledger disagree, the fresh citation wins and the ledger is amended in the same change.
- **Provenance caveat:** the original temp clones are gone; line-level provenance for v0 comparative work lives in `BRIEFS/` + `clone2/`. The v1 research lanes re-verified key claims from fresh clones (pinned HEADs recorded in each deliverable). Re-clone from upstream if line-level provenance is required for a new claim.

## 1. Absorb by subsystem (v0 register → v1 docs)

`MASTER-COMPARISON §2` categories, re-homed into the v1 set. Full item-level dispositions remain in `DISPOSITION.md`; this table is the v1 outcome map.

| v0 category | v1 docs | What was absorbed (representative) |
|---|---|---|
| 2.1 Memory & compaction | `17`, `16` | Phased write pipeline; hash-dedup-before-insert (mem0); injection≠touch (NOOA); compaction constants + budget vocabulary + overflow recovery (opencode/codex/hermes/headroom); log+projection checkpoints. → DEC-018/019/027. |
| 2.2 Guard / security | `12`, `19` | Connector-token custody → vault (SEC-1); control-plane/IPC rate limiting (SEC-2); ACP permission bridge (once/always/reject + diff previews, fail-closed — SEC-3); UI-blob secret scan (SEC-21). Design adopted; code-phase items. |
| 2.3 Browser / CDP | `23` | Managed Chromium + adapters; stale-ref/document-identity invalidation; role+nth disambiguation (already present per verification); trusted-input actions. |
| 2.4 Office | `22` | L1/L2/L3; resident contexts + flush; batch atomicity; dump/replay; render/validate; per-domain op registries + docs-sync gate (GenOffice). Corrections applied (§4). |
| 2.5 Codeintel | `26` | RepoGraph/RepoMap (Aider-lineage ranking); inference labeling; LSP bridge; rollback snapshot already present. |
| 2.6 Routing / BYOK | `18` | Provider-state ownership; half-open permit accounting + error-rate trip (RTE-3/4); catalog-as-data (models.dev-class); local discovery; provider-layer normalization contract (`DEC-034`); gateway client-identity/session-affinity headers incl. OpenCode Go (`DEC-035`). |
| 2.7 Coordinator / context / IPC | `16`, `11`, `32` | Budget scaling + live-zone rule; admission-around-Work (COO-10); typed `stop_reason` vocabulary (COO-11); engine-hosting seam → adapter/provider model. |
| 2.8 Blueprint / subagents / skills | `15`, `31` | Child session per subagent + receipts + per-spawn worktree option (verified); max_concurrent/loop-detection already present; scope-tagged registrations (DeepSeek D1); async lifecycle: completion modes · wake gate · typed child stream · report-trust scanning (`DEC-036`, wave 2). |
| 2.9 Work / scheduler / recovery / audit | `11`, `20` | Occurrence/lease/refresh state machines; liveness diagnostics with named repair actions; richer receipts + owner-native joins. |
| 2.10 Session / storage | `11`, `25`, `29` | Retention classes; single-session rule (multi-run rejected, §2-12); identity/lease model. |
| 2.11 Connectors / MCP | `28`, `14` | Connector-hub patterns (pattern-read); MCP dual-era policy (DEC-030); web search/fetch capability contract (`DEC-037`, wave 2). |
| 2.12 UI / UX | `AGENTCOWORK-UI.md` (P5) | Settings decision table (UI-8); preview/editor/artifact surfaces; workbench model. |
| 2.13 Docs / process / verification | `00`, `42` | Evidence-first discipline; source-frontmatter conventions (DOC-1); docs-sync gates (adopted: op-registry drift gate, `22` §3). |
| 2.14 Telemetry / analytics / cost | `30` | Usage/cost telemetry; **micro-compaction deferred with telemetry first** (hermes self-rejection adopted as policy). |
| 2.15 Local machine observability | `19`, `21`, `30`, `48`, `51` | Provider-per-metric hardware/process collection (System Informer, LibreHardwareMonitor, windows_exporter), bounded histories (Glances), typed OS tables (osquery), scan/cancel/treemap (WinDirStat), read-only SMART formats (smartmontools), platform-specific collection (btop/lsof); no code copied. Keep the existing AgentCowork storage walker; use on-demand least-rights process diagnostics; defer eBPF/driver tracing. See DEC-058, `ARCH/45-REFERENCE-RESEARCH.md` §Machine Observer, and `ARCH/51-MACHINE-OBSERVABILITY.md`. |

## 2. Explicit rejections (carried forward — 16)

From `MASTER-COMPARISON §3`, each mapped to the v1 invariant that enforces it:

| # | Rejected | Enforced by |
|---|---|---|
| 1 | Plaintext key storage | INV-02 (vault custody) |
| 2 | Keys visible to gateway/router processes | INV-02 |
| 3 | Fail-open authorization defaults | INV-04 (fail closed, `12` §11) |
| 4 | LLM reviewer or “server answers for the user” as authority | INV-04 (input at most; human-gesture human-only) |
| 5 | Per-engine approval authority over shared-plane effects | INV-03 (one governed path) |
| 6 | A second engine or giant model router as kernel | DEC-004/005, INV-06 |
| 7 | Micro-compaction by default | `16` §11 · `30` §5 (telemetry first) |
| 8 | Config-file credentials / unsandboxed shell as perimeter posture | INV-04/05, `12` §2 |
| 9 | Electron desktop patterns as architecture | (stack decision; Tauri stands) |
| 10 | AGPL code linking/vendoring | §3 licensing rules |
| 11 | Nango key-scope authz replacing tickets; KMS custody; hosted resale | INV-03 (tickets sole authority), INV-02 |
| 12 | Session-per-run multi-run data model | DM-004 single-session rule |
| 13 | Public naming of algorithm subsystems | `17` §4 (one API; algorithms behind it) |
| 14 | Deprecated V1 patterns as surfaces | module doc currency rules |
| 15 | Main-process data model as truth (no event log) | INV-23 (single log) |
| 16 | Silent-degradation / “installation = works” claims | INV-19, `42` evidence rules |

## 3. Licensing ledger (v1 standing)

**Handling vocabulary:** `permissive-reuse` · `pattern-read-only` (never link/copy/vendor) · `evidence-only` (facts/corpus cited; no material enters the tree) · `dual-license caveat` (scoped per component).

**Gated (pattern-read-only / evidence-only / caveats):**

| Repo | License | Handling |
|---|---|---|
| warp | MIT (`warpui*`) + **AGPL-3.0** (rest, incl. `crates/ai` index) | dual caveat — AGPL parts pattern-read-only |
| cherry-studio | AGPL-3.0 | pattern-read-only |
| nango | ELv2 | pattern-read-only (no link/vendor/hosted reuse) |
| litellm | MIT + `enterprise/` BerriAI license | dual caveat — `enterprise/` pattern-read-only |
| openwork | MIT + `ee/` OpenWork EE License | dual caveat — `ee/` pattern-read-only |
| workany | WorkAny Community License (conditions) | pattern-read-only |
| system-prompts-and-models-of-ai-tools | GPLv3 | evidence-only (taxonomy only) |
| ChatGPT (lencx) | **no license** (all rights reserved) | evidence-only (anti-pattern docs) |
| open-webui | MIT/BSD pre-cutover → custom license after `60d84a3` (branding clause) | dual caveat — post-cutover pattern-read |
| AIOS | **empty LICENSE** | pattern-read-only |
| claude-squad | AGPL-3.0 | pattern-read-only |
| agentapi (coder) | MIT — **deprecated upstream** | pattern source only (no dependency) |
| vibe-kanban | Apache-2.0 — **sunsetting upstream** | pattern source only (no dependency) |
| System Informer | MIT | pattern-read-only; no source copied |
| WinDirStat | GPL-2.0 | pattern-read-only; no source copied or linked |
| Glances | LGPL-3.0 | pattern-read-only; no source copied or linked |
| osquery | Apache-2.0 OR GPL-2.0-only | pattern-read-only; no source copied or linked |
| windows_exporter | MIT | pattern-read-only; no source copied |
| eBPF for Windows | MIT | evidence/pattern only; implementation deferred |
| smartmontools | GPL-2.0 | pattern-read-only; no source copied or linked |
| btop | Apache-2.0 | pattern-read-only; no source copied |
| lsof | Custom permissive terms in upstream COPYING | pattern-read-only; no source copied; recheck exact terms before any reuse |
| LibreHardwareMonitor | MPL-2.0 | pattern-read-only; no source copied |
| termic / acp.el | AGPL / GPLv3 (rejected candidates) | recorded; never cloned |

**Permissive-reuse set (attribution hygiene required):** codex (Apache-2.0) · opencode (MIT) · genoffice (Apache-2.0) · mem0 (Apache-2.0) · graphiti (Apache-2.0 + CLA) · nooa (Apache-2.0) · rustwright (MIT) · hermes (MIT) · obscura (Apache-2.0) · agent-browser (Apache-2.0) · deerflow (MIT) · jan (Apache-2.0) · cc-switch (MIT) · openclaw host (MIT) · openchamber (MIT) · zeroclaw (MIT OR Apache-2.0) · headroom (Apache-2.0) · ccmanager / agent-client-protocol / acpx / mosoo-agent-driver / codex-acp (MIT/Apache-2.0) · prompts.chat (MIT code + CC0 data) · everything-search skill (MIT) · open-cowork / NextCoWork / AionUi / tide / open-design / atlas / sovereign-agentic-os (MIT/Apache-2.0 grouped) · Composio (MIT) · modelcontextprotocol/registry / mcp-context-forge / dify-plugin-daemon / open-connector / Observal (Apache-2.0) · mcpm.sh (MIT) · anything-llm (MIT) · claude-mem (Apache-2.0) · cline (Apache-2.0, © 2026 Cline Bot Inc.) · grok-build (Apache-2.0 + SpaceXAI notice).

`MASTER-COMPARISON §5` priority ordering (P0: SEC-1/2/3/21) remains the code-phase entry sequence.

**Protocol SDK status (amended by DEC-061).** The Rust MCP client and façade target the official `rmcp` SDK behind Core-owned provider policy and a custom Guard-2 HTTP transport. DEC-048's rejection was superseded after a pinned SDK source review found the injection seam it had considered unavailable. SDK use does not outsource Guard, credential custody, origin policy, per-server legacy state, or audit; the official 2026-07-28 table lists Rust as Tier 1, and AgentCowork-specific tests remain required. The existing ACP client remains a separate protocol adapter owned by `ARCH/32-CHANNELS.md` §1. The sidecar's MCP client is temporary compatibility pending `TASK-PROV-006/007`, not another permanent protocol core.

| Dependency | Where it lives | Licence as recorded in the repo | Disposition |
|---|---|---|---|
| MCP protocol SDK in the Rust kernel (`rmcp-v3.4.0` initial review pin) | planned Rust dependency; not yet linked | Run dependency/license/security review before adding; no source is copied or vendored by this architecture decision | **adopt behind the custom Guard transport after parity qualification** (`14` §4, DEC-061, `TODO.md` `TASK-PROV-003`) |
| `@modelcontextprotocol/sdk` (TypeScript, v1 line) | sidecar (`packages/core-search`) | **MIT** — recorded in the repo's own notices file (`THIRD-PARTY-NOTICES.md`, MIT section). The `LICENSE-LEDGER.md` behind this § has **no row for this package** (its `modelcontextprotocol/registry` row is a different repository), so for any package this ledger does not record, **this entry defers to the licence ledger rather than guessing a licence** | in-dependency; the manifest range resolves to and the lockfile pins an **exact** version (`1.30.0`). It does not carry the modern `2026-07-28` revision, which is one reason it is not a candidate for the kernel's protocol core |

## 4. Verification corrections (do not carry forward)

| Claim | Correction | Source |
|---|---|---|
| “Codex scheduled/review queues” | No queue in the OSS tree; `review/start` RPC only — a review queue is our own product-layer build | `agent-harness-verification.md` §A3 |
| “OpenCode native-vs-summary compaction” | No native path in either generation; both summarize with the model | §C1 |
| “OpenCode V2 pruning” | Pruning is V1-only at the pinned HEAD | §C2 |
| “OfficeCLI MCP = rich tool surface” | Historical study described one command-string tool; the current pinned public `3442550` repository does not expose the engine, so do not treat the old file note as reproducible implementation evidence. Our typed-op registry is a separate requirement. | `office-runtime-verification.md` §1.A (historical); `45` OfficeCLI row (current public pin) |
| “OfficeCLI atomic swap is crash-safe” | Historical study reported a writer lacking fsync; this cannot be rechecked from the public `3442550` pin. Our fsync requirement stands independently and must be verified in our code. | §1.A (historical); `45` OfficeCLI row |
| “HTTP+SSE deprecated ≥12 months” | Deprecated since 2025-03-26 (~18 months); removal clock = SEP-2596 (eligible ≈2026-08-18, not removed) | `mcp-provider-verification.md` |
| “Vision is the industry fallback pattern” | Screenshot-first is the industry default; structured-first is **our** design choice | `world-model-verification.md` §1.B |
| Browser-event / completion-chained workflow triggers | No precedent — product inventions if shipped | `workflow-engine-verification.md` §3 |

## 5. Open questions

> **Wave-2 additions:** (4) catalog **data** license (models.dev-class source) — UNVERIFIED; decide before vendoring a snapshot. (5) Prompt-cache policy defaults — joint `16`↔`18` decision before enabling. (6) Cross-provider failover invalidation semantics (cache breakpoints · signed reasoning blocks · in-flight tool-call ids).

1. Re-clone policy for line-level provenance when a future claim needs it (README caveat stands).
2. Whether P0 code items (SEC-1/2/3/21) enter the code phase as one wave or independently.
3. Attribution file maintenance (`THIRD-PARTY-NOTICES.md` sync) when any permissive-reuse code is actually integrated.

## 6. Evidence

`REPO-COMPARE/MASTER-COMPARISON.md` §1–§5 · `REPO-COMPARE/LICENSE-LEDGER.md` (main table + deprecated/sunsetting) · `REPO-COMPARE/DISPOSITION.md` (item-level v0 dispositions) · `REPO-COMPARE/BRIEFS/` (20) · verification lanes in `ARCHIVE/v1-research/` (§4 corrections) · DEC-017.
