# AgentCowork

> **One workspace for the AI agents and tools you already use.**
>
> AgentCowork is in development. This README describes the accepted product design; it does not claim that every capability is implemented, released, or qualified.

AgentCowork is designed to make a user's chosen agent useful across a durable workspace. The agent keeps its own reasoning loop, model choice, tools, extensions, and private configuration. AgentCowork adds the surrounding workbench: shared capabilities, scoped approvals, persistent goals, collaboration, recovery, files, and evidence.

**The agent can change. The work and the user's intent stay.**

[Architecture](ARCH/00-INDEX.md) · [Product specification](AGENTCOWORK-SPEC.md) · [Experience design](ARCH/48-EXPERIENCE-SURFACES.md) · [Implementation plan](TODO.md) · [Support and qualification](SUPPORT-MATRIX.md)

## Designed experience

| Start with a simple chat | Grow into a complete workbench |
|---|---|
| Choose an agent, its supported model, and an access level in the composer. Add files, reference Library items, or invoke discoverable actions without leaving the conversation. | Keep chat beside a browser, workspace tree, editor, document/spreadsheet/slide preview, generated artifact, or running-agent view. Switch between work surfaces without losing the conversation. |
| Ask a question, attach several files, or describe an outcome in everyday language. Ordinary answers remain uncluttered. | Turn larger requests into durable Missions with milestones, independent work, approvals, recoverable attempts, and outcome checks. |

### Work that survives a session

- **Durable goals:** Mission state, requirements, plan versions, decisions, artifacts, and evidence live outside an agent's transcript. Resume reconciles changed files and other relevant state before continuing.
- **Agents that keep their own harness:** use supported external agents as peers. AgentCowork does not rewrite a discovered agent's native setup. Optional delegation creates separately scoped Work with clear ownership and provenance.
- **Shared capabilities:** attach browser, desktop, files, Office, connected apps, MCP servers, skills, and workflows at explicit scopes. A catalog entry is not an active grant.
- **Useful results:** create artifacts beside the conversation, review versions and sources, and save reusable outputs to a searchable Library. File support and edit fidelity are shown per format and provider.
- **Control you can understand:** see what is running, what it can access, what needs approval, what changed, and what evidence supports completion. Pause, steer, take over, or cancel.
- **Automation with history:** schedule and reuse reviewed workflows. Execution depends on an available local or configured remote executor; a laptop-only job pauses when that laptop is offline.

## Capability design and delivery status

The product design covers chat and attachments, model/agent selection, long-running Missions, parallel work, browser and computer use, office files, generated artifacts, connected apps, MCP, skills/plugins, workflows and schedules, local machine observation, and searchable project knowledge.

**These are design targets, not a shipping checklist.** The implementation plan and evidence map identify unfinished work; the support matrix records what can currently be built or qualified. The Windows artifact has not yet passed a real Windows acceptance run. Agent/model support, integrations, formats, editability, and remote execution must be detected and qualified; unsupported combinations need a clear fallback. “Any file” means a format-aware entry point, not a promise of lossless editing for every format.

## How it fits together

```text
Human goal
    ↓
Mission — requirements, milestones, durable plan and outcome evidence
    ↓
Work — bounded attempts owned by an external agent or a workflow
    ↓
Shared capabilities — files · browser · desktop · Office · apps · MCP
    ↓
Local or configured remote runtime
    ↓
Observed effects · artifacts · verification · recoverable progress
```

The desktop is a React/Tauri application with a Rust Core and a compiled Bun sidecar. The Core owns policy and authorization for Core-mediated actions. External agents retain their native credentials and actions; the UI labels those boundaries instead of implying the Core governed them. MCP is an integration protocol, not the Mission or Work scheduler.

## Architecture and project status

| Topic | Canonical document |
|---|---|
| Product requirements and decisions | [Specification](AGENTCOWORK-SPEC.md) · [requirements](ARCH/08-REQUIREMENTS.md) · [decisions](ARCH/04-DECISIONS.md) |
| System structure and module ownership | [Architecture index](ARCH/00-INDEX.md) · [HLD](ARCH/03-HLD.md) · [system blueprint](ARCH/50-SYSTEM-BLUEPRINT.md) |
| Agents, integrations and trust | [Agent plane](ARCH/15-AGENT-PLANE.md) · [ecosystem](ARCH/46-ECOSYSTEM-ARCHITECTURE.md) · [trust](ARCH/12-TRUST.md) |
| Missions, artifacts and interaction design | [Mission](ARCH/35-MISSION.md) · [artifacts](ARCH/29-ARTIFACTS.md) · [experience surfaces](ARCH/48-EXPERIENCE-SURFACES.md) |
| Research and benchmark scenarios | [Reference research](ARCH/45-REFERENCE-RESEARCH.md) · [market and benchmarks](ARCH/47-MARKET-AND-BENCHMARKS.md) · [test cases](ARCH/49-TEST-CASES.md) |
| Implementation and proof | [TODO](TODO.md) · [feature matrix](ARCH/09-FEATURE-MATRIX.md) · [evidence map](ARCH/42-EVIDENCE-MAP.md) · [support matrix](SUPPORT-MATRIX.md) |

## Build and contribute

Follow [AGENTS.md](AGENTS.md) for prerequisites, workspace commands, testing, and the spec-driven contribution process. Start with [CONTRIBUTING.md](CONTRIBUTING.md) for project conventions.

Architecture acceptance is not implementation completion. Product comparisons and claims of superiority require equivalent, reproducible tasks and measured results; no such result is claimed here.
