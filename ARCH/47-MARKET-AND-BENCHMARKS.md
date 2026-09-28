# 47 — Market capability evidence and comparison protocol

> Status: research snapshot, 2026-09-28. Product features vary by plan, region, platform and rollout. Rows below state only what the linked first-party material supports; they are not measured superiority claims. `38` owns the user experience and quality requirements; `45` owns open-source source pins.

## Observed product directions

| Product | Officially documented capability at review date | Implication for AgentCowork | Unknown until a controlled run |
|---|---|---|---|
| [Claude Cowork / unified Claude](https://support.claude.com/en/articles/16761823-claude-cowork-and-chat-are-one-claude) | Unified chat/task entry, connectors, local files/browser/computer use while desktop is open, cloud schedules and mobile progress/steering. [Computer use guide](https://support.claude.com/en/articles/14128542-let-claude-use-your-computer-in-cowork) distinguishes it from connector/browser operation. | Progressive chat → Mission UI; connector/browser/desktop hierarchy; explicit local/cloud executor ownership and takeover. | Relative task success, latency, permission burden and quality on matched accounts/tasks. |
| [ChatGPT Work](https://learn.chatgpt.com/docs/get-started-with-work) | Reviewable task outputs, files/plugins/tools, progress/steering/approvals, desktop local tools, and cloud continuation when selected. | Finished artifacts, inspectable progress, approvals and cloud/local task location must be first-class. | Matched artifact quality, reliability, recovery and cost. |
| [Kimi Work](https://www.kimi.ai/help/kimi-work/overview) | Local desktop knowledge-work agent for Mac/Windows; breaks work into parallel steps, uses tools/browser/files, produces documents/sheets/decks. [Product page](https://www.kimi.ai/products/kimi-work) describes specialized agent coordination. | Heterogeneous teams and office deliverables need actual end-to-end benchmarks, not only adapter counts. | Whether its parallel paths outperform or underperform the proposed integration/evidence graph. |
| [OpenWork](https://github.com/different-ai/openwork/tree/8e52796) | Pinned source shows managed OpenCode config, skill discovery, MCP merging and executable journeys (`45`). | Native/managed configuration separation and collision UX; journey-level evaluation. | User-perceived quality and failure rates on matched workflows. |
| [OpenCowork](https://github.com/OpenCoworkAI/open-cowork/tree/a1d0e4a) | Pinned source shows bounded same-harness child sessions, layered skills and MCP manager (`45`). | Cross-engine child contracts, scoped loadouts and durable receipts. | Multi-agent integration quality in comparable tasks. |
| [AionUi + AionCore](https://github.com/iOfficeAI/AionCore/tree/ea24f50) | Pinned source shows staged agent probing, read-only native MCP detection, negotiated injection and team coordination (`45`). | Startup/attachment performance and non-invasive discovery are baseline expectations. | End-to-end user friction, agent failure recovery and output quality. |
| [OfficeCLI](https://github.com/officecli/officecli/tree/3442550) | Public pinned repository exposes interface/skills, not the execution engine (`45`). | Evaluate as a black-box external office provider; do not assume internals. | Fidelity, render, formula/chart behavior, latency, safety and supported formats. |
| [OpenChamber](https://github.com/openchamber/openchamber/tree/8c70e9812e46dbea6f57a04446ab431b1a8021ad) | Pinned source has session goals with a server-side continuation/audit loop, composer model/permission/parallel controls, file/browser panes and multi-run result/fusion; write isolation is optional and defaults off (`45`). | Strong direct reference for continuing-goal controls and candidate-result review; retain a simple entry but enforce durable Mission truth, actual binding isolation and criterion-level evidence. | Matched nontechnical usability, artifact editing fidelity, model/harness comparison quality and host-shutdown recovery. |
| [Orkas](https://github.com/Orkas-AI/Orkas/tree/75468ab) | Pinned source has a conversation task board, commander dispatch and bounded reflection; `done` is execution fact, not quality (`45`). | Keep host team state and outcome verification separate; do not market agent count as task success. | Team integration quality, user steering burden and no-progress cost. |

The open-source workflow and connector comparison uses [n8n](https://github.com/n8n-io/n8n/tree/53dc2515), [Activepieces](https://github.com/activepieces/activepieces/tree/ba93a937), [cowork-os](https://github.com/cowork-os/cowork-os/tree/45de987), [Google Workspace CLI](https://github.com/googleworkspace/cli/tree/a3768d0), and the additional source ledger in `45`. They are subsystem references, not direct product substitutes in every scenario.

## Capability and appeal scorecard to prove

The desired advantage is a hypothesis, **not a current result**. Official product descriptions establish capability presence, not relative quality. The final target is broad: ordinary chat quality, multi-agent tasking, browser/desktop/SaaS/Office work, artifact editing, durable missions, recovery and portable external agents. The shipped code currently lacks the DEC-054/055 Mission and final Experience integration, so it cannot yet be said to beat Claude Cowork, ChatGPT Work or the leading open-source products overall. Cloud continuation remains a separate executor gap until implemented; model quality remains binding-dependent.

| Dimension | User-visible advantage to prove | Required evidence |
|---|---|---|
| First-run and chat | A nontechnical user gets a good answer/artifact without choosing a runtime mode or reading agent jargon | `TC-001…009`, blind usefulness review, first-run study |
| Workbench and files | Browser, tree, code, PDF/Office and artifact editing remain in one contextual surface with honest fidelity | `TC-010…018`, round-trip/render comparisons, conflict tests |
| External-agent ecosystem | Discovered agents keep native setups; shared MCP/skills/plugins are scoped and transparent | `TC-007/008/026`, config hash unchanged, permission audit |
| Team and long horizon | Heterogeneous agents produce one verified result and recover after context/provider/environment loss | `TC-024…036`, integration rubric, fault injection, cost |
| Automation and skill learning | Recorded work becomes a safe versioned procedure; triggers are durable | `TC-021…023`, changed-input replay and secret scan |
| Business outcomes | Connected information becomes correct, useful deliverables with no duplicate real-world effects | `TC-019/030/037`, source/formula/action oracle |
| Market comparison | Same task, account, hardware and time budget against each named product | Raw result bundles, platform/plan/version, failure and unsupported labels |

Research priority is driven by measured user value and failures. [Nielsen Norman Group's progressive disclosure guidance](https://www.nngroup.com/articles/progressive-disclosure/) supports revealing advanced choices as needed; it does not prove this particular design is usable. `48` therefore requires actual nontechnical and technical studies. [OpenChamber's own product page](https://github.com/openchamber/openchamber) shows goal/multi-run, browser preview and device continuation; AgentCowork needs to match the end-to-end journey before claiming a harness advantage. [Orkas's source](https://github.com/Orkas-AI/Orkas/blob/75468ab/src/main/features/group_chat/task_board.ts) explicitly distinguishes task termination from quality, which supports the separate Mission outcome gate but does not establish relative performance.

## Benchmarks required before the word “better”

Use a dated, reproducible task pack with identical inputs, accounts/permissions and human rubric where the products can legally and practically be compared. Record platform, hardware, product version/plan, model/agent binding, token/cost accounting, allowed time and the exact human intervention. At least five repetitions per scenario should expose variance; report median, p95 and failure count, plus the full distribution when sample size supports it. Blind human review scores output usefulness, correctness, edit effort and visual quality. An independent verifier scores intent compliance, side effects, evidence validity and recoverability. Do not hide tasks a product cannot run; label `unsupported`, `blocked_by_access` or `not_tested` distinctly.

| Scenario | Outcome contract | Failure injection / review |
|---|---|---|
| Connected inbox → Drive/Sheets report | Find relevant messages, correlate sources, produce a checked sheet/report, leave sends as drafts unless approved | Missing OAuth scope, stale message, formula error, duplicate record |
| Browser YouTube research | Find specified videos, record title/channel/date and source refs, synthesize only observed content | Login handoff, layout change, video unavailable, unsupported transcript |
| Local desktop task | Modify a real application and verify saved state | Window focus loss, user takeover, app restart |
| Office artifact | Create/edit DOCX/XLSX/PPTX and verify render, formulas and output location | Bad template, corrupt input, font/format drift |
| Repository change | Implement requested behavior in isolated branch, verify tests and diff, explain side effects | Context reset, compiler failure, branch conflict |
| Heterogeneous team | Lead plus different-engine child and integration node produce one coherent deliverable | Child crash, incompatible output, scope collision |
| Long-horizon Mission | Resume after a fresh session and changed input; finish against current contract | Agent swap, laptop offline, changed repo/API, quota exhaustion |
| Reusable procedure | Convert an observed run into a reviewed skill/workflow and replay on new input | Secret in trace, unstable UI action, version regression |

**Release evidence:** publish scenario definitions, scoring rubric, raw result/evidence bundles with private data removed, failure taxonomy, measured latency/cost and known platform limits. Set performance targets after measuring a reference hardware baseline; no unmeasured number is a fact. Improvement priority follows user outcomes, friction and reliability, not a count of tools or agents.
