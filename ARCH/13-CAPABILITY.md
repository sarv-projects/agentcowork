# 13 — Capability Plane

> **DEC-054 amendment:** The shared catalog is additive to an external agent's native tools. Catalog visibility is not a grant or automatic MCP mount. Resolver preference may recommend structured API/MCP before browser/desktop by observed reliability, but it cannot force an external agent to abandon native tools. Actual path and provenance are recorded (`46`).

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-CAP-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** semantic Core-mediated operations — **what** can be done — resolved to providers — **who** does it (DEC-004). The host presents budgeted shared capabilities to compatible agents; an external agent retains its own native tool catalog and context policy (DEC-054).
> **Dependencies:** `10-KERNEL` · `11-WORK` · `12-TRUST` (policy/tickets) · `14-PROVIDERS` (implementations) · `16-CONTEXT` (what enters prompts) · `31-SKILLS-PLUGINS` (skill→capability requirements).
> **Evidence:** product-owner brief (`CapabilityDescriptor` / `CapabilityResult` / `CapabilityHandle`, capability graph, loading modes, “do not expose 500 raw tools”, L1/L2/L3 progressive model) · `ARCHIVE/v1-research/agent-harness-verification.md` §A2/§E1 (bounded tool fragments), §D1 (scope-tagged registrations) · `ARCH/06-DATA-MODEL.md` DM-011/012 · `ARCH/07-CONTRACTS.md` CTR-009/010 · DEC-004/005/024/025.

## 1. Purpose & responsibilities

**Owns:** the capability registry · the catalog (UI/agent-facing discovery) · the resolver (capability → provider selection) · capability **handles** (epoch-checked, reusable bindings) · affordances · guidance states · loading modes · the capability graph · risk-class declarations.
**Never owns:** execution (`14`) · permission decisions (`12`) · reasoning (`15`).

Rules:

1. A capability describes **what**, never **who** — no protocol, vendor, or tool name appears in a capability id or descriptor.
2. One capability ↔ many providers; one provider ↔ many capabilities (DEC-004).
3. Capability ids are stable public API; provider assignment is **not** part of the contract.
4. Anything invocable **through Core's shared surface** is a registered capability with a descriptor, risk class and verification hook (INV-19). External agent-native tools are outside this registry and carry separate provenance (DEC-054).

## 2. Descriptor model (DM-011)

```
CapabilityDescriptor {
  id, version, description,
  affordances[],            // what the caller can express / what it supports
  requirements[],           // auth, connected app, environment, other capabilities
  providers[],              // registry-derived; not hand-maintained
  loading_mode,             // eager | catalog | on-demand
  risk_class,               // safe | sensitive | dangerous
  auth_requirements?,       // vault refs / connection ids
  verification              // what must run before a receipt (hook ref)
}
```

Descriptor rules: dotted naming (`office.spreadsheet.edit`) · content-versioned · `risk_class` maps to policy defaults (`12` §3) and verification depth (`34`) · `requirements` may name other capabilities (graph edges, §7) · `providers` is derived from the registry, never copy-edited.

## 3. Result model

```
CapabilityResult {
  status: completed | guidance | requires_user_action | failed,
  output?, receipt?, next_action?, retryable
}
```

- **`guidance` / `requires_user_action` are first-class** — a capability may answer *“you need to connect Google Drive first”* with a `next_action`, instead of failing the agent’s plan (`AGENTCOWORK-SPEC.md` §5; verified pattern).
- `completed` always carries (or references) a receipt for **Core-mediated** externally visible effects (INV-07, DEC-054).
- `failed` carries a typed error (`10` §3) and `retryable`.

## 4. Handles (DM-012)

```
CapabilityHandle {
  capability_id, provider_id, provider_epoch, environment_id,
  permission_snapshot, runtime_handle_ref, expires_at
}
```

- **Lifecycle:** resolve → invoke (validated) → expire. A provider restart (epoch bump) invalidates outstanding handles (DM-012; semantics in this section).
- **Hot path** (why handles exist): handle lookup → Guard → ticket → dispatch → ack — the control path we bound (p50 < 2 ms / p95 < 10 ms / p99 < 25 ms). Cold work (discover, connect, negotiate, enumerate) happens once, behind the registry.
- **Caching:** per session/agent scope with TTL; `permission_snapshot` is bound into the handle so a policy change mid-session re-validates at ticket time.

## 5. Resolution & selection

Resolver inputs: capability id + constraints (agent, session, workspace, environment, policy snapshot, cost/latency class, health).

First filter candidates by declared grant/policy and environment requirements; an ineligible provider cannot win a ranking. Rank eligible candidates by **health → environment fit → cost/latency class**; ties are deterministic and audited. Guard still decides each invocation. Failover on provider failure advances only to another eligible candidate. When setup could enable a missing provider, return `guidance`/`requires_user_action` with the concrete requirement.

## 6. Loading modes & context discipline

| Mode | Meaning | Where it appears |
|---|---|---|
| `eager` | Always known for the active agent/task | Compact definitions in model context |
| `catalog` | Known to the UI/registry, not the model | UI catalog, capability browser, agent loadout editor |
| `on-demand` | Resolved only when invoked | Zero context cost until requested |

This is the **shared-plane semantic compression layer**: compatible agent sessions receive task-relevant shared capability subsets under budget (`16` §3), never a host-generated dump of hundreds of raw tools. The host cannot rewrite an external engine's private prompt or native catalog. An engine without an explicit overlay/tool-attachment mechanism shows the shared capability as unavailable for that binding; discovery alone never mounts it. Activation is scoped per binding/session/Work (four-state model, DEC-024) — the registry itself is global/workspace.

**Progressive complexity (L1/L2/L3)** — generalized from the office model, adopted as a plane-wide convention: L1 semantic (read/inspect/query) → L2 structured mutation (set/add/remove/move) → L3 raw escape hatch (domain-native API). Domains document their ladder; the model prefers the lowest level that does the job.

## 7. Capability graph

Edges:

```
implemented_by → Provider            requires → Capability
exposed_to     → AgentProfile        described_by → Skill
guarded_by     → Policy              executes_in → Environment
```

Uses: dependency resolution (`requirements` pulls in other capabilities before invocation), UI grouping/authoring, policy mapping (risk→decision), verification mapping (risk→depth), and skill binding (`31`).

## 8. Registry operations & governance

- **Registration paths:** native domains (`22`–`28`), MCP discovery mapping (`14` §2/§4), ACP/CLI/plugin adapters, future connectors (`28`).
- **Versioning:** additive changes are minor; breaking descriptor changes require a DEC; deprecation windows are explicit and surfaced. Runs pin the descriptor version they started with; a capability deprecated mid-run keeps serving that run until it completes (EDGE-012).
- **Census gate:** a test gate asserts unique ids, non-empty affordances/verification, and provider coverage; the census outputs a generated capability catalogue (doc generation, not hand-maintained).
- **Review checklist for a new capability:** id naming · risk class · affordances · requirements/auth · verification hook · default loading mode · provider(s) · exposure scope.

Capability ids stay protocol-neutral. The adapter owns mappings for **host-exposed** tool calls at discovery; an unmapped tool is not invocable through Core and surfaces as guidance. That restriction does not disable a bound agent's private native tools (`DEC-047/054`; `14` §5).

## 9. Failure modes

| Failure | Behavior |
|---|---|
| No provider available | `guidance` / `requires_user_action` with concrete `next_action`. |
| All providers unhealthy | `Unavailable` + retryable; resolver failover recorded. |
| Version mismatch | Typed error + migration note; deprecated versions keep serving until window ends. |
| Stale handle | Re-resolve (epoch mismatch ⇒ `InvalidState`); no silent retry against a changed runtime. |
| Ambiguous resolution | Deterministic tie-break + audit; never random. |
| Requirement chain incomplete (rechecked at invocation) | Blocked with the missing requirement named (capability or auth); a revoked/uninstalled requirement fails typed (`NotFound`/guidance) without retaining permissions (EDGE-095). |

## 10. Interop

**Depends on:** `10` · `12` (decisions, tickets) · `14` (adapters/health) · `16` (activation budget) · `31` (skill requirements).
**Exposes to:** `15` (invocation surface), `20` (workflow action nodes), `32` (agent/UI catalogs), UI (capability browser).
**DAG check:** the plane never executes; it resolves and hands back handles.

## 11. Open questions (`OQ-CAP-*`)

1. Descriptor publication format (language-neutral schema; generated docs) and its role in the capabilities triple (catalogue ↔ docs ↔ spec).
2. Catalog subsetting rules for external agents (projection, `32`).
3. Handle TTL defaults + revocation semantics (with `12`).
4. Standard `next_action` taxonomy for guidance (connect / install / configure / grant).
5. Capability versioning policy details (major/minor + deprecation windows).

## 12. Evidence

Product-owner brief (`CapabilityDescriptor`/`Result`/`Handle`, capability graph, loading modes, semantic compression, L1/L2/L3) · `agent-harness-verification.md` §A2 (bounded fragments feeding activation), §E1 (fragment registry), §D1 (scope-tagged tool registration as the per-agent variance mechanism) · `ARCH/06-DATA-MODEL.md` DM-011/012 · DEC-004/005/024/025 · INV-03/19.

## 13. Requirements (`REQ-CAP-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-CAP-001` | No host flat dump — compatible sessions receive budgeted shared-capability subsets; native agent catalogs remain agent-owned (INV-13, DEC-054). |
| `REQ-CAP-002` | Epoch-checked handles — resolve→invoke→expire; provider epoch bump invalidates stale handles (DM-012, §4). |
| `REQ-CAP-003` | Capabilities describe what, never who — providers attach to capabilities, never the reverse (DEC-004). |
| `REQ-CAP-004` | Descriptor contract — every capability has a versioned descriptor with risk class and verification hook (DM-011, INV-19). |
| `REQ-CAP-005` | Guidance is first-class — `guidance` / `requires_user_action` are results, not failures. |
| `REQ-CAP-006` | Shared-capability loading modes and semantic compression — eager/catalog/on-demand; activation scoped per binding/session/Work (DEC-005/024/054). |
| `REQ-CAP-007` | Permission/environment eligibility precedes deterministic health→environment→cost/latency ranking; ties audited; failover stays eligible. |
| `REQ-CAP-008` | Capability graph — requirements resolve before invocation; blocked chains name the missing edge. |
| `REQ-CAP-009` | Registry governance — unique ids, additive versioning, deprecation windows, census gate. |
| `REQ-CAP-010` | Core-invocable ⇒ governed — every shared invocation carries descriptor + risk class + verification hook; native tools retain native provenance (INV-03/19, DEC-054). |
