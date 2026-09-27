# 22 — Office

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-OFFICE-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the office domain runtime **under the universal document surface** (DEC-013) — not a sidebar mode. Progressive **L1 semantic → L2 structured mutation → L3 raw escape hatch**; documents stay **resident** for active sessions; render/validate before receipts.
> **Dependencies:** `13`/`14` (capabilities/providers) · `19-RUNTIME-ENVIRONMENTS` · `12-TRUST` (paths/exec) · `29-ARTIFACTS` (previews/versions) · `34` (verification depth). **Consumers:** `15` (agent office work), UI (document surface).
> **Evidence:** `ARCHIVE/v1-research/office-runtime-verification.md` (448 lines; OfficeCLI verified in source, GenOffice per-domain registries verified) · v0 corpus `ARCHIVE/v0/RESEARCH/desktop_app/28,29` · local `crates/agentcowork-office` (frozen reference) · DEC-013.

## 1. Purpose & rules

**Owns:** document/spreadsheet/presentation/PDF runtimes · per-format **operation registries** · resident contexts + leases · batch atomicity · render/preview · validation hooks · template merge · format providers (native · IronCalc · LibreOffice/external).
**Never owns:** UI surfaces (document surface is UI; `AGENTCOWORK-UI.md`) · artifact storage (`29`) · verification policy (`34` decides depth; Office supplies hooks).

1. **One registry per format, shared by every surface** — CLI/MCP/GUI/agent/API all resolve the *same* operations; a docs-sync test gates drift (verified GenOffice pattern).
2. **Typed ops, not a command-string tool** — unlike OfficeCLI’s single MCP command tool (verified: one string tool), our operations expose through the capability registry as typed descriptors (`13`); the model gets semantic ops, not a shell.
3. **Resident with a lease** — one resident context per document, exclusive writer lease, flush policy, crash-safe commit (atomic swap **+ fsync** — the OfficeCLI gap we do not copy).
4. **No lossy re-serialization without disclosure** — formats declare engines/limits; we prefer surgical OOXML patching over whole-file re-serialization (openpyxl’s lost shapes are the cautionary proof).
5. **Render/validate before receipts** — validation hooks run proportional to risk (`34`, INV-19).

## 2. Format providers & technology

| Format | Primary (native) | Fallback / external | Notes |
|---|---|---|---|
| DOCX | `agentcowork-office` OOXML patcher | LibreOffice headless (convert/render, MPL-2.0) | Surgical edits preferred; re-serialization declared when unavoidable |
| XLSX | IronCalc (recalc) + patcher | LibreOffice headless | **Always recalc via IronCalc** before commit (formula integrity) |
| PPTX | OOXML patcher + staged deck builder | LibreOffice headless | Charts/images: subset; SmartArt/OLE deferred |
| PDF | Native PDF runtime | LibreOffice/mutool-class tools | Redact must **remove** content, not annotate (code-phase P0) |

Licensing: LibreOffice is MPL-2.0 (usable; packaging decision deferred); OfficeCLI (.NET) is **benchmark/external-provider only** — its architecture is absorbed, not embedded (`44-ABSORB-REGISTER`).

## 3. Operation registry (per format — v1 op set)

| Format | L1 (semantic read) | L2 (structured mutation) | L3 (raw) |
|---|---|---|---|
| **DOCX** | text · blocks · fields · styles outline | patch block text · track changes · comments · citations · media GC | part-level raw XML (gated) |
| **XLSX** | sheet/cell reads · ranges · formulas · names | set cell/formula · batch ops · fill/sort/shift · rename · basic format · basic chart · **recalc** | gated raw part |
| **PPTX** | slides · shapes · notes inventory | shape text · add/remove slide · notes · transitions/animations (subset) | part-level raw XML (gated) |
| **PDF** | text/objects · forms · pages | form-fill · exact-match replace · annotate · page ops · author · **redact (true removal)** | object-level raw (gated) |

Deferred with triggers (per verification §6.2): pivot authoring · text reflow · SmartArt/OLE editing · multi-writer merge · real-time co-editing. Triggers: product demand + a fidelity proof.

Registry mechanics: each op = `{id · input/output schema · risk · executor · verification hook}`; docs-sync test asserts registry ↔ docs parity; MCP/CLI/GUI/agent surfaces are generated from it (no divergent implementations).

## 4. Resident contexts & crash safety

- **One resident context per document + exclusive lease**; second writer gets an explicit “in use” result (no merge in any surveyed system — merge is deferred).
- **Flush policy:** interval + dirty-marker driven; explicit flush on session end; idle eviction under memory bounds.
- **Crash-safe commit:** write to a staging package → **fsync** → atomic swap (OfficeCLI is process-death safe but not fsynced; we add fsync) → op-log replay on recovery.
- **Scratch isolation:** temp/work areas confined to declared roots (pathfloor; GenOffice `GENOFFICE_ALLOWED_ROOTS` pattern).
- **Batch atomicity:** a batch applies all-or-nothing with an op log for replay/audit.

## 5. Render, validate, verify

- **Render:** previews are projections (`29` §7) — HTML-screenshot via a browser shell-out (verified OfficeCLI approach) or native renderers; never model tokens (DEC-015).
- **Validate (deterministic):** per-format structural checks before commit (DOCX structure/text round-trip; XLSX recalc + formula presence; PPTX slide/shape audit; PDF page/object counts), plus render-diff where useful.
- **Verification hooks (to `34`):** risk-scaled — e.g. redact requires a post-op text-extraction check proving removal; sends of externally visible documents require a receipt with the validation result (`29` §3).

## 6. Templates & staged construction

- **Templates:** declared merge subset per format (fields/placeholders), validated after merge.
- **Staged deck pattern** (verified GenOffice): `deck_start → deck_page → deck_build → deck_replace` with **check-before-write** validation at each stage — adopted as the reference for large presentations.
- **Audit tooling:** render + inspect + repair loop is a first-class workflow for high-risk artifacts.

## 7. Failure modes

| Failure | Behavior |
|---|---|
| Crash mid-write | Staging package + fsync + swap + op-log replay (never a torn file). |
| Corrupt/unreadable document | Quarantine + typed error; original untouched. |
| Engine limitation (charts/pivots/SmartArt) | Typed `guidance` with the limitation named; no silent lossy path. |
| External engine crash/timeout (LibreOffice headless) | Typed `Unavailable`/`Timeout` naming the engine; the operation aborts without commit and the original stays untouched. |
| Concurrent open | Lease message + options (read-only render vs wait). |
| Huge workbook/document | Bounded loads + streaming reads; declared limits. |
| Validation failure pre-commit | Batch aborts atomically; receipt records the failed check. |

## 8. Interop

**Depends on:** `10` · `12` (paths/exec) · `13`/`14` · `19` (resident hosts) · `29` · `34`.
**Exposes to:** `15` (office capabilities), UI (document surface), `34` (validators/renderers), `29` (versions/previews).
**DAG check:** Office executes document ops; it never governs itself or owns artifacts.

## 9. Not in v1 (with triggers)

Pivot authoring · reflow · SmartArt/OLE editing · multi-writer merge · real-time co-editing · embedded rendering engine (we shell out) — each with product-demand/fidelity-proof triggers.

## 10. Code-phase fixes identified (frozen code)

1. **Resident/lease missing** in the current crate (has commit/snapshot primitives) — the main gap for DEC-013.
2. **PDF “redact” currently annotates** — must remove content (v0 P0 carried forward).
3. **fsync before atomic swap** — partial: the DOCX/PDF command paths use `agentcowork_office::write_atomic` (temp → `sync_all` → rename, plus a best-effort directory fsync on POSIX), but the XLSX command path has its own `atomic_write` (`src-tauri/src/xlsx_cmds.rs:301-312`) = write + rename with **no fsync**; route it — and every new commit path — through the fsynced primitive (OfficeCLI's no-fsync trade-off is the gap we do not copy).
4. Declare per-engine fidelity limits in the registry (lossy ops surface as `guidance`).

## 11. Open questions (`OQ-OFFICE-*`)

1. Native renderer vs browser shell-out per format (packaging/licensing trade).
2. IronCalc coverage vs LibreOffice fallback thresholds.
3. PDF redact implementation path (object-level removal engine).
4. Template subset definition per format.
5. Merge deferral trigger (when multi-writer demand justifies design).

## 12. Evidence

`ARCHIVE/v1-research/office-runtime-verification.md` — §1.A OfficeCLI (`IDocumentHandler.cs:58-104` L1/L2/L3; `ResidentFlushPolicy.cs:5-15`; `CommandBuilder.Batch.cs:187-193,473-490`; `AtomicPackageWriter.cs:45-52`; `McpServer.cs:562-600` single-tool; `HtmlScreenshot.cs:10-12` shell-out) · §1.B GenOffice (`pptx-ops/src/ops/registry.ts:1-21` + `tests/op-docs-sync.test.ts:43-60`; `cli/src/mcp/deck.ts:58-253,260-276`; `GENOFFICE_ALLOWED_ROOTS`) · §2 fidelity (openpyxl shapes/pivot docs; LibreOffice `start_parameters` + MPL-2.0) · §3 op set · §4 resident design · §6 hooks/non-goals · v0 corpus 28/29 · DEC-013.

## 13. Requirements (`REQ-OFFICE-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-OFFICE-001` | One per-format operation registry shared by every surface; typed descriptors via `13`; docs-sync gate — no command-string tool (DEC-013) |
| `REQ-OFFICE-002` | Progressive L1 semantic read → L2 structured mutation → L3 gated raw escape hatch; L3 never the default |
| `REQ-OFFICE-003` | One resident context per document + exclusive writer lease; second writer gets "in use" (read-only/wait); no merge in v1 |
| `REQ-OFFICE-004` | Crash-safe commit: staging package → fsync → atomic swap → op-log replay; scratch under declared roots (pathfloor) |
| `REQ-OFFICE-005` | Batch all-or-nothing with op log; pre-commit validation failure aborts and records the failed check |
| `REQ-OFFICE-006` | Declared engine limits; unsupported fidelity returns typed `guidance` naming the limitation — no silent lossy path |
| `REQ-OFFICE-007` | Previews are token-free projections (DEC-015); per-format structural validation runs before commit |
| `REQ-OFFICE-008` | Risk-scaled verification hooks (INV-19): redact proves removal; externally visible sends receipt the validation result |
| `REQ-OFFICE-009` | XLSX always recalculates (IronCalc-class engine) before commit — never stale cached formula values |
| `REQ-OFFICE-010` | Templates and staged deck builds validate check-before-write at each stage; a failed stage leaves no partial artifact |
| `REQ-OFFICE-011` | Corrupt inputs quarantine with a typed error and untouched original; huge documents use bounded/streaming loads |
