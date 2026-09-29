# 22 — Office

> **DEC-055 Workbench amendment:** `48` owns the visible reader/editor and format support disclosure. Provider ability is probed per document and operation; preview, structural edit, recalc, native-app fallback and round-trip fidelity are separate capabilities. “Open any file” means a universal entry point and honest fallback, not guaranteed lossless editing. A provider that cannot faithfully round-trip a feature must refuse or create a reviewed derivative, never silently flatten the original.

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-OFFICE-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** the office domain runtime **under the universal document surface** (DEC-013) — not a sidebar mode. Progressive **L1 semantic → L2 structured mutation → L3 raw escape hatch**; documents stay **resident** for active sessions; render/validate before receipts.
> **Dependencies:** `13`/`14` (capabilities/providers) · `19-RUNTIME-ENVIRONMENTS` · `12-TRUST` (paths/exec) · `29-ARTIFACTS` (previews/versions) · `34` (verification depth). **Consumers:** `15` (agent office work), UI (document surface).
> **Evidence:** `ARCHIVE/v1-research/office-runtime-verification.md` (448 lines; historical OfficeCLI/GenOffice source notes) · v0 corpus `ARCHIVE/v0/RESEARCH/desktop_app/28,29` · local `crates/agentcowork-office` · DEC-013. The current pinned public OfficeCLI repository `3442550` exposes README/skills/installer, not its execution engine (`45`); its older file-level notes are not an implementation source until their revision and availability are reverified.

## 1. Purpose & rules

**Owns:** document/spreadsheet/presentation/PDF runtimes · per-format **operation registries** · resident contexts + leases · batch atomicity · render/preview · validation hooks · template merge · format providers (native · IronCalc · LibreOffice/external).
**Never owns:** UI surfaces (document surface is UI; `AGENTCOWORK-UI.md`) · artifact storage (`29`) · verification policy (`34` decides depth; Office supplies hooks).

1. **One registry per format, shared by every surface** — CLI/MCP/GUI/agent/API all resolve the *same* operations; a docs-sync test gates drift (verified GenOffice pattern).
2. **Typed ops, not a command-string tool** — our operations expose through the capability registry as typed descriptors (`13`); the model gets semantic ops, not a shell. The older OfficeCLI single-tool observation is historical and must be reverified before comparison.
3. **Resident with a lease** — one resident context per document, exclusive writer lease, flush policy, crash-safe commit (atomic swap **+ fsync**). The commit guarantee is our requirement and must be measured on each platform.
4. **No lossy re-serialization without disclosure** — formats declare engines/limits; we prefer surgical OOXML patching over whole-file re-serialization (openpyxl’s lost shapes are the cautionary proof).
5. **Render/validate before receipts** — validation hooks run proportional to risk (`34`, INV-19).

### 1.1 Shared document-handler registry

The existing per-format operation registry is also the **universal document-handler registry** used by `48`'s Workbench; this is an extension of the Office/format-provider boundary, not a new service. A `DocumentHandlerDescriptor` declares `handler_id/version` · content signatures · MIME types/extensions · operations (`metadata`, `preview`, `extract`, `select`, `annotate`, `edit`, `render`, `validate`, `convert`, `open_native`) · per-operation support/fidelity limits · size/time/resource bounds · isolation class · provenance/health. `CapabilityBroker` (`13`) still owns invocation and Trust; the handler registry resolves which descriptor can serve a particular file and operation.

Resolve against a bounded content-signature probe first, then declared MIME, then extension; an extension/content mismatch is shown and never silently treated as the extension's format. Resolution is deterministic and may return multiple user-selectable providers when capabilities differ. Every operation is probed independently: a handler that can preview does not imply it can edit or save losslessly. Never execute a document's macros, embedded scripts, archive entries or linked content during identification or preview. Each parser/provider has input bounds; untrusted or third-party handlers run in the existing sandbox/provider boundary (`19`, `31`).

## 2. Format providers & technology

| Format | Primary (native) | Fallback / external | Notes |
|---|---|---|---|
| DOCX | `agentcowork-office` OOXML patcher | LibreOffice headless (convert/render, MPL-2.0) | Surgical edits preferred; re-serialization declared when unavoidable |
| XLSX | IronCalc (supported-formula recalc) + patcher | LibreOffice or compatible installed spreadsheet provider | Recalc with a provider that supports the workbook's formulas/features before commit; an unsupported or inconsistent recalc blocks verified save or requires explicit native-app handoff. Never overwrite cached values with an unprobed engine. |
| PPTX | OOXML patcher + staged deck builder | LibreOffice headless | Charts/images: subset; SmartArt/OLE deferred |
| PDF | Native PDF runtime | LibreOffice/mutool-class tools | Redact must **remove** content, not annotate (code-phase P0) |

Licensing: LibreOffice is MPL-2.0 (usable; packaging decision deferred); OfficeCLI (.NET) is **benchmark/external-provider only** — its architecture is absorbed, not embedded (`44-ABSORB-REGISTER`).

### 2.1 Arbitrary-file opening and native-app handoff

Every file selected from Files, Library, chat attachments or an agent result gets a stable Workbench tab, even when no semantic reader is registered. Selecting a file never runs it or auto-launches its OS default. A supported handler provides its declared preview/extraction/edit operations. With no handler, the tab shows safe metadata (identity, size, extension, detected MIME/signature, freshness and why no reader is available) and a user-invoked **Open with…** action for a registered/OS-native app; it does not fabricate extracted text. For untrusted documents with active content, use a handler's protected/view-only mode when available; otherwise disclose the native application's active-content behavior and require a separate explicit open action. Executables, scripts, shortcuts and installers never use the ordinary document-open path. The chat may carry the exact file/version reference, but an agent receives bytes or extracted content only through its granted file capability and a compatible provider.

Native-app handoff is an operation, not a renderer claim: resolve an explicitly selected executable through a provider descriptor, pass the file as a distinct argument under pathfloor, show the target application, and retain the Workbench tab as the file-status/selection surface. Watch the stable file identity and digest. After the native app changes the file, report the new version and conflict state, then refresh/re-open or import a new managed Artifact version according to the owning file/artifact contract (`25`/`29`); never overwrite concurrent Workbench changes silently. If there is no safe app association, offer a picker or leave the metadata tab open. An optional office suite may broaden preview/edit coverage but is not assumed installed or embedded.

Third-party format handlers may register through the existing provider/plugin surfaces (`14`, `31`) after review, explicit enablement and scoped grants. There is one resolution path and one capability authorization path; no extension may register a hidden shell command, bypass Core, or imply that every format has a lossless editor. This provides an open-ended format ecosystem while keeping the default Workbench useful for unknown files.

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
- **Crash-safe commit:** write to a staging package → **fsync** → atomic swap → op-log replay on recovery. Any comparison to OfficeCLI's private writer remains unverified at the current public pin.
- **Scratch isolation:** temp/work areas confined to declared roots (pathfloor; GenOffice `GENOFFICE_ALLOWED_ROOTS` pattern).
- **Batch atomicity:** a batch applies all-or-nothing with an op log for replay/audit.

## 5. Render, validate, verify

- **Render:** previews are projections (`29` §7) — a browser renderer or native renderer selected by a probed format provider; never model tokens (DEC-015). The historical OfficeCLI HTML-screenshot note is not current pinned implementation evidence.
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

## 10. Code-phase implementation status

The former frozen-code findings below have been rechecked against the live tree. Implementation status and outstanding acceptance evidence are tracked by `TODO.md` W0 (`TASK-OFFICE-001`–`003`) and the `FIX-14`–`FIX-16` rows in `ARCH/42-EVIDENCE-MAP.md`; this section records the current design boundary, not a second task ledger.

1. **Resident/lease:** `agentcowork-office::resident` now owns per-format tables and work-bound writer leases. The current Tauri commit adapters acquire a context for a commit and close it after flush; a persistent Workbench editing session and op-log/restart/batch-recovery acceptance are not established.
2. **PDF redact:** the runtime now removes content from page content streams and has a checked path that refuses unremovable intersections or requested residual strings. Format coverage and all required failure cases still need acceptance evidence; a visual overlay is never called a redaction.
3. **Durable commit:** DOCX/PDF and XLSX mutation paths use the shared staging → fsync → atomic-swap commit API. Readback compares the exact committed bytes using a bounded buffer (not merely file length). Platform-specific directory durability, operation-log replay, and crash-injection acceptance remain required; see `TASK-OFFICE-003`.
4. **Fidelity limits:** per-engine limits must be declared in the registry; lossy or unsupported operations return typed `guidance` rather than silently claiming fidelity (`TASK-OFFICE-005`).

## 11. Open questions (`OQ-OFFICE-*`)

1. Native renderer vs browser shell-out per format (packaging/licensing trade).
2. IronCalc coverage vs LibreOffice fallback thresholds.
3. PDF content types and unsupported object classes that must make redaction refuse rather than claim removal.
4. Template subset definition per format.
5. Merge deferral trigger (when multi-writer demand justifies design).

## 12. Evidence

`ARCHIVE/v1-research/office-runtime-verification.md` — §1.A contains **historical, not currently reproducible from OfficeCLI public pin `3442550`** notes naming `IDocumentHandler.cs`, `ResidentFlushPolicy.cs`, `CommandBuilder.Batch.cs`, `AtomicPackageWriter.cs`, `McpServer.cs` and `HtmlScreenshot.cs`. Do not use those as coding anchors until a revision exposing them is pinned. §1.B GenOffice notes `pptx-ops/src/ops/registry.ts`, `tests/op-docs-sync.test.ts`, `cli/src/mcp/deck.ts` and `GENOFFICE_ALLOWED_ROOTS`; confirm exact upstream revision before reuse. §2 fidelity (openpyxl shapes/pivot docs; LibreOffice) · §3 op set · §4 resident design · §6 hooks/non-goals · v0 corpus 28/29 · DEC-013. Current exact public comparison anchor: `45` OfficeCLI row.

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
| `REQ-OFFICE-009` | XLSX commit uses a provider probed for that workbook's formulas/features and verifies cached values; unsupported recalc blocks verified save with explicit fallback |
| `REQ-OFFICE-010` | Templates and staged deck builds validate check-before-write at each stage; a failed stage leaves no partial artifact |
| `REQ-OFFICE-011` | Corrupt inputs quarantine with a typed error and untouched original; huge documents use bounded/streaming loads |
