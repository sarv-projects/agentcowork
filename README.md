# AgentCowork

> In development. The target architecture below is accepted design, not a list of shipped or qualified capabilities. [Support matrix](SUPPORT-MATRIX.md) describes the currently intended release artifacts and qualification status; [feature matrix](ARCH/09-FEATURE-MATRIX.md) records implementation evidence.

AgentCowork is a work environment around interchangeable external AI agents. A person asks a question or states an outcome. AgentCowork keeps substantial goals as durable Missions, dispatches bounded work to agents and workflows, offers shared capabilities for files, browser, desktop, Office and connected services, and checks the result against the requested outcome. The agent keeps its own reasoning loop, model, tools, native extensions and private configuration. Horizon Code is a future external binding.

## What the finished experience is designed to do

- **Start simply.** Chat is the default. The composer has an agent picker, a model picker only when the agent supports it, an access control, `+` attachments, structured `@` references and namespaced `/cowork:*` commands. Running work can be steered or queued.
- **Keep work durable.** A Mission stores the goal, requirements, plan versions, attempts, artifacts and evidence outside any agent session. A failed agent or lost context does not erase the goal. Resume checks changed files, accounts and other external state before acting.
- **Use the right surface.** The right Workbench opens a managed browser, workspace tree/worktree, code, PDF, Word, spreadsheets, slides, media, artifacts and agent/task detail beside chat. It exposes editing only when the selected provider can preserve the file faithfully; otherwise it offers a reader or native-app fallback.
- **Produce real artifacts.** Outputs have versions, provenance, validation status and input dependencies. The Library distinguishes generated, uploaded, imported and linked material. Retrieval cites exact permitted versions and locations.
- **Coordinate workers.** A lead can delegate independent work to different external agents, integrate their outputs and verify the combined result. Agent-native subagents stay owned by their agent; the host only claims control of child Work it created.
- **Automate repeated work.** A reviewed recording can become a scoped skill or a versioned workflow. Existing workflow systems can be called as external providers while retaining their own execution and credentials.
- **Show what happened.** Core-mediated effects use its policy, ticket, verification and receipt path. A self-contained agent's native effects remain under that agent's policy and are labelled as reported or observed. Mission completion needs evidence against the user's requirements, not an agent's assertion alone.

The experience is designed for nontechnical and technical people: ordinary answers stay uncluttered, while Tasks, the Workbench, agents, permissions, timeline, cost, evidence and diagnostics become available when needed. Local and cloud are execution locations in the target architecture. Work on a laptop pauses when that is its only executor and it goes offline; continued execution requires a configured remote/cloud executor. No claim of superiority over other products is made until comparable tasks are measured.

## Current state and limits

The repository contains working code and an accepted architecture, but the final Mission plane and Experience described above are implementation work. The Windows artifact is not yet qualified on a real Windows acceptance run. Support for a particular external agent, model picker, shared-tool overlay, browser attachment, Office feature, local model or remote executor must be probed and verified; the UI must show an unavailable state or fallback when it is absent. “Any file” means one entry point with truthful format support, not lossless editing of every MIME type.

## Architecture and delivery

| Need | Source |
|---|---|
| Product contract and requirements | [Specification](AGENTCOWORK-SPEC.md), [requirements](ARCH/08-REQUIREMENTS.md) |
| Architecture map and ownership | [Index](ARCH/00-INDEX.md), [HLD](ARCH/03-HLD.md), [system blueprint](ARCH/50-SYSTEM-BLUEPRINT.md) |
| Mission and recovery | [Mission](ARCH/35-MISSION.md), [outcome and recovery](ARCH/36-OUTCOME-AND-RECOVERY.md) |
| External agents and extensions | [Ecosystem architecture](ARCH/46-ECOSYSTEM-ARCHITECTURE.md) |
| Final UI and Workbench | [Experience surfaces](ARCH/48-EXPERIENCE-SURFACES.md) |
| Research and comparison | [Source ledger](ARCH/45-REFERENCE-RESEARCH.md), [market protocol](ARCH/47-MARKET-AND-BENCHMARKS.md) |
| Product scenarios and pass oracles | [Test cases](ARCH/49-TEST-CASES.md) |
| Implementation status | [Feature matrix](ARCH/09-FEATURE-MATRIX.md), [TODO](TODO.md), [evidence map](ARCH/42-EVIDENCE-MAP.md) |
| Contributor process | [AGENTS.md](AGENTS.md), [CONTRIBUTING.md](CONTRIBUTING.md) |

The desktop uses a React/Tauri UI, Rust Core crates and a compiled Bun sidecar for shared services. External agents attach through supported adapters; MCP is a tool/resource protocol, not the Mission runtime. Build, test and release instructions are in [AGENTS.md](AGENTS.md) and the [packaging contract](PACKAGING.md). Code changes follow the repository's spec-driven process. Architecture acceptance does not imply a passing product benchmark.
