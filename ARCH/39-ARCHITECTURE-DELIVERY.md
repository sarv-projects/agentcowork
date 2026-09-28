# 39 — Architecture delivery sequence and acceptance

> Status: accepted target architecture 2026-09-28 under DEC-054/055; implementation pending. This is an architecture dependency plan; `TODO.md` W6 is the delivery-status tracker. `48` owns final interaction design, `49` outcome scenarios, and `50` cross-plane maps.

## Current-state delta (source observed, not completion claims)

| Existing source/design | Keep / change / add | Reason and acceptance |
|---|---|---|
| `crates/agentcowork-types/src/lib.rs` Work state and `agentcowork-blueprint/src/checkpoint.rs` checkpoint records | Keep and extend with Mission refs | Durable execution primitives exist; Mission semantic contract/plan/evidence records do not appear as canonical entities yet. |
| `crates/agentcowork-acp` and DEC-049 self-contained ACP boundary | Keep; add negotiation and honest provenance | Preserve external engine autonomy and native extensions; test each optional method and governance badge. |
| `crates/agentcowork-acp/src/chief.rs` mandatory `COWORK_AFFINITY_STEERING` and `scripts/check-prompt-steering.mjs` | Replace forced ranking with optional task-scoped shared affordance | Current shipped block says `ALWAYS` for office/browser and conflicts with DEC-054; revise its assertion in the same code change. |
| `src-tauri/src/channel_b.rs` hard-coded `sessionId=channel-b`, `agentId=external` | Replace with authenticated actual binding/session/Work/grant context | Per-agent policy and receipts cannot be correct with placeholder identity. |
| `src-tauri/src/mcp_cmds.rs` `McpServerRow` and `mcp_servers.json` with `auto_start=true` default | Split catalog definition, activation grant and runtime instance; migrate persisted rows | Current rows lack host/workspace/Work scope and allow lazy start on a global-looking row. Existing user-defined MCP entries must survive migration without auto-granting every agent. |
| `crates/agentcowork-blueprint/src/crystallize.rs` workflow detector/compiled script helper | Reuse classification logic only after review; move promotion into `37` lifecycle | Source has deterministic/cognitive step split but the helper alone is not a reviewed, permissioned, versioned skill promotion. Reachability from product UI is not established by this inspection. |
| Work/Workflow/Artifact/Event designs in `11`/`20`/`29`/`30` | Keep one owner each | Mission dispatch/evidence integrates through existing contracts and event store, not duplicate engines or microservices. |
| First-party reasoning engine and unconditional plugin/MCP global load | Do not add | External agents own reasoning and native extensions; host catalog entries require scoped activation. |

“Existing source” means a code path was inspected, not that the proposed end-to-end behavior is implemented or benchmarked. The feature matrix remains the implementation-status authority.

## Module viability audit for the final target

The ownership design is viable where an interface and an honest fallback exist. The table names the dependency or proof that can still fail; a design row is not an implementation pass. Module numbers refer to the owner docs, and scenario IDs to `49`.

| Owner modules | Viable design boundary | Required proof / remaining limit |
|---|---|---|
| `10`–`11` Kernel/Work | One canonical state and scheduler can host Mission attempts | Crash replay, bounded cancellation and budget/accounting under `TC-027/034` |
| `12`–`14` Trust/Capability/Providers | Core-mediated tool path can be uniform; native effects stay separate | Real per-call identity/grant, lazy MCP scopes, no false receipt, connector custody under `TC-008/019/030` |
| `15`–`18` Agents/Context/Memory/Models | External engines keep native loops while Mission rebuilds context and routes compatible workers | Adapter-by-adapter capability negotiation; no assumed resume/model picker/native child data; local-model eligibility measured under `TC-024/026/028` |
| `19`–`20` Runtime/Workflow | Existing Work/Workflow ownership accommodates local, remote and deterministic waits | Remote executor, lease/credential boundary and schedule/event dedupe; local shutdown pauses unless remote work is assigned (`TC-023/034/036`) |
| `21` World | Stable IDs and freshness can ground selections and resume | Platform collectors and changed-state invalidation on real Windows apps (`TC-017/018/028`) |
| `22` Office | Typed format providers plus native fallback avoid claiming universal edit fidelity | DOCX/XLSX/PPTX/PDF round-trip, formula/chart/font/annotation oracle (`TC-010…013`) |
| `23`–`24` Browser/Computer | Managed Chromium and structured-first desktop ladder are coherent separate runtimes | Supported Chrome bridge, login/takeover/re-snapshot, scaling/elevation and observed final state (`TC-016…018`) |
| `25`–`27` Files/Code/Search | Identity, worktrees, index and retrieval have separate owners | Concurrent edit conflict, search revocation/freshness, code diff and tests (`TC-015/020/025`) |
| `28`–`29` Comms/Artifacts | Scoped SaaS action and versioned artifact graph produce real deliverables | OAuth expiry/dedup, exact-version citations, stale propagation and duplicate-effect reconciliation (`TC-019/030/033/037`) |
| `30`–`32` Events/Extensions/Channels | One event store and per-session extension grants avoid a second platform | Durable replay, native inventory/host activation distinction, collision/revoke and channel identity (`TC-007/008/026`) |
| `34`–`38` Effect/Mission/Skill/Experience | Effect proof, goal proof and reviewed procedure learning are distinct | Independent verifier, no-progress stop, safe capture and nontechnical comprehension (`TC-021/027/029/032/038`) |
| `46`/`48`/`50` Ecosystem/Workbench/blueprint | End-to-end route from user selection to worker/typed effect/artifact/evidence is explicit | The real UI and adapters must satisfy `TC-001…038`; no architecture diagram is a product benchmark |

`ARCH/09-FEATURE-MATRIX.md` owns exact implementation status, including `implemented` versus `verified`; this table records only the target seam and its proof obligation.

## Sequencing

1. Reconcile truth boundary: update invariants, governance badges, Channel-B identity, native-vs-mediated receipts and extension scope model. Preserve existing user work. Exit: no path claims tickets/audit for native effects, and a shared call identifies its actual Work/session/agent/grant.
2. Make discovery/attachment robust: staged read-only probes, capability negotiation, native extension inventory, ephemeral host overlay, install provenance and collision UI. Exit: attaching a discovered agent leaves its native config byte-identical; unavailable injection is explicit.
3. Establish Mission records inside one Rust module, with schema migrations and events: contract versions, requirements, PlanVersion/PlanNode, assumption/decision/evidence links. Extend Work references and artifact dependency edges. Exit: a Mission survives session replacement and a failed Work creates another attempt without losing its node.
4. Build Mission controller on the existing scheduler: ready-node dispatch, context packets, bounded budgets, versioned PlanPatch, branch-local waits, integration and cancellation propagation. Exit: heterogeneous agents can run independent nodes, report receipts, and a conflict cannot overwrite a newer plan.
5. Add outcome evaluator, no-progress detector and resume reconciliation. Exit: crash/context reset/agent swap preserves goal and evidence; external drift invalidates only affected nodes; repeated identical failure stops; incomplete required evidence blocks completion.
6. Connect shared browser/desktop/SaaS/office and optional external workflow providers. Exit: capability path is chosen by suitability, actual ownership is visible, OAuth actions use action scopes, and waitpoint callbacks reconcile idempotently.
7. Add workflow capture → skill proposal → evaluation → versioned publication, plus mission-control UI and cross-device/cloud executor adapters. Exit: a recorded procedure is inspectable and rollbackable; local-offline work pauses honestly; cloud continuation requires a configured executor and shows where it runs.
8. Deliver the DEC-055 Experience as a coherent journey: composer and renderer, first-run/agent setup, Workbench browser/file/Office tabs, Library retrieval, team view and searchable grouped Settings. Exit: each control is bound to a real capability/contract; `TC-001…038` oracles run on pinned builds, including nontechnical and accessibility cohorts. Unsupported format or adapter behavior has an honest fallback rather than a decorative promise.

Each stage can ship while later stages remain planned; capability and quality determine priority, not an arbitrary v1 label. Do not create additional network services for conceptual modules. Reuse existing crates and extract `agentcowork-mission` only when its own dependency boundary is established. Horizon Code is a future external binding, never a special path.

## Review gates and open evidence

Architecture acceptance requires a source-path/pin or official document for external claims, a canonical owner for every durable field, one policy decision path for mediated effects, one trigger owner per automation, and a measurable failure case for each new behavior. Implementation acceptance requires executable scenario evidence, independent outcome review and a diff/side-effect review. Open research: current official competitor feature parity changes frequently; published native-agent protocols vary; OfficeCLI's public engine source is absent at the reviewed pin; browser/computer-use quality depends on real applications and accounts; local-model role eligibility depends on hardware and observed benchmark. These are explicit unknowns for experiments, not invented guarantees.
