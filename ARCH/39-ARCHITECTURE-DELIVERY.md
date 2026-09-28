# 39 — Architecture delivery sequence and acceptance

> Status: accepted target architecture 2026-09-28 under DEC-054; implementation pending. This is an architecture dependency plan; `TODO.md` remains the delivery-status tracker and is intentionally unchanged in this review.

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

## Sequencing

1. Reconcile truth boundary: update invariants, governance badges, Channel-B identity, native-vs-mediated receipts and extension scope model. Preserve existing user work. Exit: no path claims tickets/audit for native effects, and a shared call identifies its actual Work/session/agent/grant.
2. Make discovery/attachment robust: staged read-only probes, capability negotiation, native extension inventory, ephemeral host overlay, install provenance and collision UI. Exit: attaching a discovered agent leaves its native config byte-identical; unavailable injection is explicit.
3. Establish Mission records inside one Rust module, with schema migrations and events: contract versions, requirements, PlanVersion/PlanNode, assumption/decision/evidence links. Extend Work references and artifact dependency edges. Exit: a Mission survives session replacement and a failed Work creates another attempt without losing its node.
4. Build Mission controller on the existing scheduler: ready-node dispatch, context packets, bounded budgets, versioned PlanPatch, branch-local waits, integration and cancellation propagation. Exit: heterogeneous agents can run independent nodes, report receipts, and a conflict cannot overwrite a newer plan.
5. Add outcome evaluator, no-progress detector and resume reconciliation. Exit: crash/context reset/agent swap preserves goal and evidence; external drift invalidates only affected nodes; repeated identical failure stops; incomplete required evidence blocks completion.
6. Connect shared browser/desktop/SaaS/office and optional external workflow providers. Exit: capability path is chosen by suitability, actual ownership is visible, OAuth actions use action scopes, and waitpoint callbacks reconcile idempotently.
7. Add workflow capture → skill proposal → evaluation → versioned publication, plus mission-control UI and cross-device/cloud executor adapters. Exit: a recorded procedure is inspectable and rollbackable; local-offline work pauses honestly; cloud continuation requires a configured executor and shows where it runs.

Each stage can ship while later stages remain planned; capability and quality determine priority, not an arbitrary v1 label. Do not create additional network services for conceptual modules. Reuse existing crates and extract `agentcowork-mission` only when its own dependency boundary is established. Horizon Code is a future external binding, never a special path.

## Review gates and open evidence

Architecture acceptance requires a source-path/pin or official document for external claims, a canonical owner for every durable field, one policy decision path for mediated effects, one trigger owner per automation, and a measurable failure case for each new behavior. Implementation acceptance requires executable scenario evidence, independent outcome review and a diff/side-effect review. Open research: current official competitor feature parity changes frequently; published native-agent protocols vary; OfficeCLI's public engine source is absent at the reviewed pin; browser/computer-use quality depends on real applications and accounts; local-model role eligibility depends on hardware and observed benchmark. These are explicit unknowns for experiments, not invented guarantees.
