# 29 — Artifacts & Receipts

> **DEC-054/064/065 amendment:** Artifact versions gain explicit `mission_id`, `plan_node_id`, input artifact/resource versions and downstream dependency edges; a changed input marks dependent outputs stale pending review (`36`). Core effect receipts and agent-reported/native-observed evidence remain different evidence classes. No inferred native effect receives a Core ticket or verification badge. Conversation links resolve exact authorized artifact/file versions; activity projection reuses these refs.

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-ART-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** **Artifacts** are work products — versioned, provenance-carrying, scoped to work. **Receipts** are durable evidence of effects. **Library** is the reusable inventory; promotion is explicit (DEC-014).
> **Dependencies:** `10-KERNEL` · `25-FILES` (identity/locations) · `30-EVENTS` (stream, telemetry) · `34-EFFECT-VERIFICATION` (verification precedes receipts). **Consumers:** `15`, `20`, UI (`32`), external agents (artifact gateway).
> **Evidence:** product-owner brief (`Artifact` / `ProvenanceChain` / `LibraryItem` schemas; “Save to Library”; receipts first-class) · `ARCH/06-DATA-MODEL.md` DM-019/020/023 · `ARCH/07-CONTRACTS.md` CTR-018 · DEC-014 / DEC-022 / DEC-023 / DEC-032 · INV-07 / INV-18.

## 1. Purpose & responsibilities

**Owns:** the artifact model + versioning + provenance + preview refs · the **ReceiptService** (record · get · replay) · the Library (inventory + promotion) · the artifact gateway for external agents (exchange via refs) · retention/GC.
**Never owns:** verification execution (`34` produces the verification result a receipt references) · the security audit log (`12`; receipts are product evidence, audit is the security record) · file-content indexing (`25`).

## 2. Artifact model (DM-019)

| Field | Meaning |
|---|---|
| `id` · `name` · `type` · `mime_type` | identity + classification |
| `source` | `agent` · `workflow` · `user` · `worker` |
| `origin` | `generated` · `uploaded` · `imported` · `linked`; orthogonal to producing `source` |
| owner refs | `session_id?` · `run_id?` · `workflow_id?` · `agent_id?` · `workspace_id?` |
| `version` | immutable per version; `parent_artifact?` for lineage (e.g. derived reports) |
| `location` | storage URI (managed store or workspace-file reference) |
| `provenance` | chain: `created_by_agent → run → worker → workflow` + inputs digest |
| `permissions` | who can read/write/share (enforced by `12`) |
| `created_at` | UTC ms |

**Types (non-exhaustive):** documents · spreadsheets · presentations · PDFs · images · diagrams · code patches/diffs · datasets · reports · logs/bundles · web captures.

**Versioning rules:** versions are immutable once written; a new edit creates a new version; workspace-file artifacts are tracked by file identity (`25`) plus a managed copy when they must survive edits; provenance is mandatory (INV-18).

## 3. Receipts (DM-020)

```
Receipt {
  id, effect_ref, ticket_ref,
  capability_id, provider_id,
  inputs_digest, outputs,
  verification,            // what ran before the receipt (per risk class, 34)
  status, work_id, timestamps
}
```

Rules:
- **Mandatory** for every Core-mediated externally visible effect (INV-07, DEC-054); emitted inside the governed path (`12` → `13` → `34` → receipt). Native agent reports and observations are different evidence records, not Core effect receipts.
- **Immutable**; receipts are the product-facing evidence chain.
- **Replay = evidence replay**, not re-execution: `replay(receipt)` reconstructs inputs + shows what verification ran and what changed; re-doing the action is a **new work item** (never a silent re-fire).
- Emission also writes an event (`30`) and an audit entry (`12`) — three views of one fact, never duplicated state.

## 4. Library (DM-023)

**DEC-055 target:** The user-facing Library is one searchable place for generated, uploaded, imported and linked work products as well as saved templates, skills and workflows. Artifact origin is separate from Library membership: an uploaded file can be visible in the workspace inventory without being promoted to a reusable template. Every item exposes source kind, active version, MIME/type, size, creator/importer, origin Work/Mission, permissions, extraction/index status, validation status and dependency/staleness status. Filters apply to both metadata and permitted full-text search. Exact-version retrieval includes page/cell/slide/line/time anchors and parser confidence; extraction errors never become invented content. Content changes re-index the new version and invalidate downstream derivations. Revocation, deletion and workspace switching remove the item from unauthorized search results and agent context. `48` owns presentation; `25` owns file identity/freshness, format/extraction providers yield content, `27` owns indexing/query; this module owns inventory/version/provenance and dependency edges.

| Field | Meaning |
|---|---|
| `kind` | `agent` · `skill` · `workflow` · `connector` · `plugin` · `template` · `prompt` · `saved_artifact` |
| `name` · `description` · `version` · `usage_count` | inventory metadata |
| `saved_from_artifact_id?` | explicit promotion origin |

Distinction: artifacts are scoped to work; the Library is global and durable. Promotion is explicit (“Save to Library” / “Save as template”) — never automatic (DEC-014). Versioning + deprecation of library items are explicit operations.

## 5. Artifact gateway (external agents)

Exchange via refs only: `artifact_id` · `mime_type` · `uri`. Supported verbs (permission-gated): read · write · attach · transform · publish. External agents never see raw storage paths; the gateway maps to the managed store (working URI scheme token `eaios://artifact/<id>`; final scheme renames with the brand, OQ-ART-02).

## 6. Retention & GC (DEC-032)

- **Managed store:** per-workspace, content-addressed immutable versions; workspace files referenced by identity (`25`).
- **Receipt-pinned versions are never GC’d** — chain integrity outranks storage savings.
- Unreferenced versions: pruned by age/count policy per workspace; deletions are audited; media (large binaries) follow the same rule set.
- Explicit delete is a user/authorized operation and is audited (INV-24); Library items may outlive their originating work.

## 7. Previews & rendering handoff

**Workbench contract (DEC-055):** `48` owns tab identity, layout, selection and user interaction. This module returns immutable version refs and preview/provider capability metadata. A selected range is `(artifact identity, version, typed location)` rather than a pasted caption. Office/PDF/image preview support is independent of edit/round-trip support; an unsupported editor offers read-only preview or native-app fallback with an explicit warning. Edit/save creates a new version only after the domain provider validates its result; no UI control may imply lossless editing of an unprobed format. Managed artifact previews use isolated renderers and cannot inherit app or vault privileges.

**Conversation result link path (DEC-064):** Prefer structured artifact/result events referencing an immutable artifact id+version. Where an external agent only writes a file, `25`/Work observation must correlate that exact file identity/version to the active Work and authorized workspace before Experience creates an artifact card. A filename/path in prose is only a display hint; it becomes a clickable ref only if it uniquely resolves to that already-authorized Work result. Ambiguous, stale, outside-scope or unregistered text remains inert or opens an explicit chooser. Clicking opens/focuses the exact-version Workbench tab; it never reads arbitrary paths or executes content. Preserve the output's actual provenance (`core_created`, `agent_reported`, `native_observed`, `user_added`) and do not synthesize a Core receipt for native work.

Previews are **projections** (thumbnail/render refs) produced by domains (`22`–`24`) — artifacts store refs, not pixels. The UI opens them through the universal document surface (`AGENTCOWORK-UI.md`); opening/rendering consumes zero model tokens (DEC-015).

### 7.1 Bounded tool-result previews — helper landed, integration seam pending

A large tool result is **referenced, never inlined raw**. The bounded preview is the result-side of the same principle as `13` §6's loading modes: the model must never receive a dump it cannot use. The invariant is that **truncation never claims success** —

- a truncated preview reports `truncated: true` together with the **true full size**, so a consumer can never mistake it for the whole value;
- its **inline form yields nothing while truncated**, so the one use that would be a silent loss (passing a partial value off as *the* result) is refused by construction;
- it **never invents a reference**. A ref is only as good as the artifact behind it, and writing one is a local persistent mutation owned here — so the ref is a field the caller fills *after* its own write succeeds, and an unwritten artifact can never be advertised.

**The required order is: write the artifact, then attach the reference.** The component that must own the write is the **work/capability result path** — the same place a receipt is emitted (§3) and where artifact creation already belongs. It shapes the bounded preview, writes the full bytes through the artifact gateway, and only then sets the reference it received. The order *is* the invariant: a ref set before its artifact exists advertises a version that cannot be opened, which is a worse failure than an oversized result.

**Status: pending.** The helper is landed (`agentcowork-mcp/src/preview.rs`, exported from `agentcowork-mcp/src/lib.rs:21,40-42`) and unit-tested, but it has **no caller in the tree** — the seam above is unwired. It stays that way deliberately: a writer inside the MCP protocol crate would be a second artifact store, and the protocol layer has no workspace, no store and no work-item identity. The seam is recorded here rather than papered over; nothing in the doc set treats it as wired.

## 8. Failure modes

| Failure | Behavior |
|---|---|
| Write fails mid-version | Version is atomic — no partial versions visible; retry or discard; audit. |
| Location moved/deleted | Identity check (`25`) marks artifact `unresolved`; receipts referencing it keep the digest; surfaced for re-link. |
| Receipt missing for a visible effect | Blocked before commit (INV-07); the effect path cannot complete without it. |
| Truncated result delivered as if whole | Refused by construction: the inline form is empty while truncated and a ref is never invented, so the caller must write the artifact first and then attach the reference (§7.1). A result that cannot be delivered whole is an honest gap, never a silent partial. |
| Disk full / write failure during artifact or receipt write | Typed failure; no partial version or receipt becomes visible; a mandatory-receipt effect pauses with reason (EDGE-104, INV-07). |
| Cross-workspace gateway request | Denied typed with no path leakage; v1 is workspace-scoped — cross-workspace sharing is explicit export only (EDGE-105, INV-11). |
| GC vs receipt race | Receipt pin check runs in the GC transaction; pinned versions are skipped. |
| Gateway permission denied | Typed `AuthorizationDenied`; no path leakage. |

## 9. Interop

**Depends on:** `10` · `25` (identity/locations) · `30` (events) · `34` (verification result) · `12` (permissions).
**Exposes to:** `15`/`20` (attach results), `16` (artifact refs as context items), UI (`32`), external agents (gateway).
**DAG check:** artifacts never execute; receipts never authorize (they record).

## 10. Open questions (`OQ-ART-*`)

1. Storage layout details (directory scheme, blob format) — implementation-phase decision under DEC-032.
2. URI scheme finalization (brand rename; OQ-003 tie).
3. Library item versioning vs template semantics (how “template” differs from `saved_artifact`).
4. Cross-workspace artifact sharing rules (deferred; v1 = workspace-scoped + explicit export).
5. Receipt retention horizon (forever vs time-boxed with chain digest retention).

## 11. Evidence

Product-owner brief (`Artifact`, `ProvenanceChain`, `LibraryItem`; promotion lifecycle; receipts) · `ARCH/06-DATA-MODEL.md` DM-019/020/023 · `ARCH/07-CONTRACTS.md` CTR-018 · DEC-014/022/023/032 · INV-07/18/24 · `ARCH/17-MEMORY.md` (export/import pattern) · `agentcowork-mcp/src/preview.rs` (§7.1 bounded preview + ref seam; landed, no caller yet).

## 12. Requirements (`REQ-ART-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-ART-001` | Artifact versions are immutable; every edit creates a new version with lineage (DM-019, INV-18) |
| `REQ-ART-002` | Provenance is mandatory on every artifact (INV-18) |
| `REQ-ART-003` | Receipts are mandatory for Core-mediated externally visible effects, emitted inside the governed path (INV-07, DEC-022/054) |
| `REQ-ART-004` | Receipts are immutable; `replay` is evidence replay, never re-execution (CTR-018) |
| `REQ-ART-005` | A receipt emission writes event + audit — three views of one fact, never duplicated state (INV-23/24) |
| `REQ-ART-006` | Library promotion is explicit; artifacts are work-scoped, the Library global and durable (DEC-014, DM-023) |
| `REQ-ART-007` | Gateway exchange is refs-only under per-verb permissions; v1 workspace-scoped, cross-workspace denied (EDGE-105, INV-11) |
| `REQ-ART-008` | Receipt-pinned versions are never GC'd; the pin check runs in the GC transaction (DEC-032) |
| `REQ-ART-009` | Retention pruning and explicit delete are audited (INV-24, DEC-032) |
| `REQ-ART-010` | Previews store render refs, not pixels; opening/rendering consumes zero model tokens (DEC-015) |
| `REQ-ART-011` | Moved/deleted locations mark the artifact `unresolved`; receipts keep the digest and re-link is explicit (EDGE-101) |
| `REQ-ART-012` | Writes are atomic under failure; a mandatory-receipt effect cannot complete without its receipt (EDGE-104, INV-07) |
