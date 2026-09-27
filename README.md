# AgentCowork

> **In development — not released.** What follows describes the finished product. Nothing here is available to install today, and no capability described below has been qualified on a real customer machine yet.

## What it is

AgentCowork is a **desktop application that does work on your computer** — and does it with an agent you choose, a model you choose, and rules you control.

You bring the agent. You bring the model keys. The work happens on your machine: your files, your documents, your browser, your apps. Nothing is uploaded to us, because there is no us-hosted service in the middle.

It is an execution layer that sits on the computer you already own. It is not an operating system, not a cloud platform, and not a chat box with a file picker.

**And it is built for work measured in days, not minutes.** Start something on Monday, close the laptop, and come back on Friday to work that actually got done — with the state, the reasoning and the evidence still intact. That is the part most AI tooling is quietly bad at, and it is the part this product is organised around.

**One governed path for everything it does.** Every action it takes on your behalf — reading a file, opening a browser, sending an email, editing a spreadsheet — travels the same route: request, permission check, ticket, action, verification, receipt, permanent log entry. There is no second, faster, unlogged route. That is the whole idea: an agent with real hands should be *auditable*, not just capable.

## Why it exists

Today's AI tools split into camps that don't talk to each other:

- **Chat assistants** know your conversation but not your machine, and forget you tomorrow.
- **Autonomous coding agents** are powerful but scoped to a repository, and they want to run shell commands freely.
- **Desktop automation** can click things but has no memory and no judgement about what it's allowed to do.
- **Cloud agents** are capable but your files have to leave the machine to reach them.

AgentCowork is the layer that makes those into one governed system: an agent that remembers, that can act anywhere you're allowed to act, and that shows its work.

## What it does

The capability areas. Each one is a real part of the product, not a roadmap item.

### Getting work done

- A conversation that runs real jobs, not just answers — it can start work, walk away, and come back with the result
- Foreground, background and long-running work, so a two-hour job doesn't block a two-minute question
- Every run produces a **receipt**: what it did, what it changed, what it couldn't do, and why
- You can see, stop, redirect or cancel any piece of work at any time
- Big outputs get spilled to disk rather than flooding your screen or your context window

### Agents: bring your own, then delegate

- **Any agent you already use can plug in.** Installed agent CLIs and protocol-speaking agents become first-class engines inside AgentCowork
- No privileged in-house agent. Every engine — including future ones — runs through identical rules and identical permission checks
- **Subagents**: a job can spawn child agents that work in parallel, each in its own session and, optionally, its own isolated copy of a codebase
- Parent jobs get receipts, not raw agent chatter, so the result stays readable
- Bounded, task-shaped tools — a clean set of operations rather than hundreds of raw primitives

### Your desktop and the browser

- It can **see and operate real applications**: native interfaces, structured accessibility trees, and vision as a fallback when an app exposes nothing else
- Consent-gated input — typing, clicking and keystrokes ask first, with confirmation thresholds that stop runaway bulk actions
- Every action is verified afterwards; a failure is reported, not hidden
- **Browser control** in a managed browser, plus adapters for the browsers you already use — navigate, read, extract, fill forms, download
- Lightweight page reading by default, escalating to a full browser only when a task genuinely needs one
- **Logins and sessions stay sealed.** An agent can use your logged-in session without ever being able to read your credentials
- It will not build or bypass bot-detection. When a site blocks it, you get told — it doesn't go around anything

### Documents, spreadsheets and PDFs

- Reads and **edits Word, Excel, PowerPoint and PDF surgically** — it changes the paragraph or the cell, not the whole file
- Untouched parts of a file stay byte-identical, so diffs are readable and reviews are meaningful
- **Spreadsheet truth**: computed values are recalculated by a real engine, so a formula change produces a correct result, not a guess
- Crash-safe editing with one-click rollback — a half-finished document never replaces a good one
- Universal document ingest for anything it can't edit natively, with a conformance check that a round-trip didn't lose anything
- Artifacts are versioned with mandatory provenance; a version referenced by a receipt is never silently deleted

### Memory and context

- **Memory that persists across sessions** — your projects, your preferences, your decisions, with provenance on every item
- Memories are **added and superseded, never silently overwritten**, so you can always see why the system believes something
- Forget works by suppression, and forgetting a fact forgets the chain of things derived from it
- **Memory and context are different things.** Memory is what the system knows; context is what's worth spending tokens on *this* turn
- Nothing enters the model's context that wasn't justified — no recalled memories means no memory tokens spent
- The agent decides what the model sees; the system supplies the material. An external agent keeps its own native context control
- Large context is passed by reference with bounded previews, not pasted in whole

### Models and providers

- **Your keys, your choice of provider** — multiple providers, multiple keys per provider, automatic failover when one is rate-limited
- Works with subscription sign-in as well as API keys, and with models running on your own machine
- One registry, one router. The model catalog is data that updates on its own, never code
- **No silent downgrades** — if a request must move to a different model, you see it happen
- Credentials live in an encrypted vault, are usable by the system, and are never visible to the agent, the model, the logs, or the receipts
- Cost and usage are measured from what providers actually report, and you can see the ledger
- One capability can resolve across many implementations — a built-in runtime, an MCP server, a plugin, a remote service — behind a single contract

### Security, privacy and an audit trail

- **One decider** for permission: allow, ask, or deny. Everyday actions run; dangerous ones ask; the catastrophic ones are refused
- Two independent floors guard everything: a policy floor and an enforcement floor, so a bug in one does not open the door
- Path traversal and server-side request forgery are blocked before the action, not detected afterwards
- **A tamper-evident audit log.** Every effect is chained and signed; the log is append-only, and history cannot be quietly edited
- An agent and a workflow ask permission the same way, through the same primitive
- **Local-first privacy**: no telemetry, no analytics SDK, fully functional offline
- Retention windows, and a one-action "remove all my data" that deletes the lot
- Support bundles are allow-listed by hand — nothing is sent anywhere without you choosing to send it

### Files, search and code

- A fast index of your files, treemaps, duplicate detection, large-file finding and cleanup you approve before anything is removed
- **Search is deterministic, not a guess** — a real ranked index with citations, and it is allowed to say "I found nothing" rather than inventing an answer
- A tiered search cascade: a private metasearch surface, with fallbacks, plus instant filename and content search
- **Deep research reports** and per-site indexing when you want a real document, not a snippet
- Codebase understanding that stays inside a budget: a ranked map of the repo, language-server-aware edits, and isolated per-job copies of a workspace
- An edit ladder that fails loudly rather than guessing — exact match, structured patch, then a clearly-reported fuzzy fallback
- Test and build execution are governed, with bounded output and per-step rollback

### Workflow, automation and connected services

- Repeatable pipelines defined as typed workflows, with runs pinned to the definition that produced them
- Triggers: manual, scheduled, called by an agent, chained from another workflow, or from system events
- A crash-resume matrix — a machine that dies mid-pipeline resumes where it stopped instead of starting over
- **Skills teach; plugins extend** — and both go through a sandboxed review gate before they touch anything
- Email, calendar and messaging arrive as capabilities, not as a special integration: reads are bounded, and **sending is approval-gated and receipted**
- No bulk ingest of your inbox behind your back

### A live model of your world

- AgentCowork maintains a continuously updated structural model of your digital world — what exists, how it relates, and how fresh each fact is
- **Queries, not screenshots.** It asks the system what is true instead of looking at a picture of it
- Independent collectors each carry a health record and a consent record, and nothing is captured that you have not agreed to
- Watchers that detect change with cursors and epochs, so a gap in observation forces a scoped re-scan rather than a silent hole
- Local, sandboxed and remote execution environments, each belonging to a work item — and one that degrades loudly rather than quietly pretending to be contained

## Built for work that spans days

Most AI help is designed around a single sitting. You ask, it answers, the context fills up, and when you come back the thread is stale, lossy, or gone — and the real work never happened. AgentCowork is organised the other way round. **Work is a durable thing that lives in a system, not in a conversation window.**

**Work is recorded, not remembered.** Every piece of work is a durable item with an owner, a lifecycle, and a written history of what was attempted. Each step carries an idempotency key, so a retry after a crash can never apply the same effect twice. A crash is not a lost day — it is a step that gets picked up again.

**The machine is allowed to be off.** Long work does not depend on a window staying open or a laptop staying awake. Detached work runs on its own schedule, and scheduled work happens whether or not you are watching. When something needs a decision it cannot safely make on your behalf, it holds that item for you rather than guessing or firing a prompt nobody will ever see.

**Memory that improves instead of drifting.** Facts are added, never silently overwritten. When something changes, the old version is marked as superseded rather than deleted — or, worse, left to quietly contradict the new one. Forgetting is real forgetting, not an accumulation of exceptions. Memory is scoped to your projects and carries provenance, so you can always see why the system believes something, and correct it.

**Progress you can read in a minute, not an hour.** You are not handed a transcript to scroll. You are handed receipts: what was done, what changed, what it cost, what could not be done, and what was actually verified. A week of work reads as a short list of outcomes with the evidence attached.

**Nothing happens quietly that shouldn't.** Everyday actions run. Actions that leave your machine, send something to a person, or cannot be undone ask first — including at three in the morning while you are asleep. There is no class of unrequested, unattended, unlogged action.

**Budgets that stop work instead of reporting it afterwards.** Long-running work carries named budgets, checked before the work begins rather than discovered when the bill arrives, and it stops and tells you instead of overrunning in the dark.

### What this product does not claim

The field is young and the marketing around it is not, so here is the honest edge of what we make:

- **We do not claim an agent runs untouched for a week.** Nothing in this category does. What we claim is that the work is *durable* — recorded step by step, resumed from recorded state, and never a matter of guesswork about what happened.
- **We do not claim perfect memory.** We claim that a changed fact supersedes the old one instead of coexisting with it, and that you can inspect and correct any of it.
- **We do not claim plans repair themselves.** Nothing reliably does. Plans are written down, versioned, and pinned to the run that produced them.
- **We do not claim a receipt proves the result was right.** A receipt proves what happened and what was checked. Whether it was the *right* thing is still your judgement — which is exactly why you get the evidence rather than a summary.

## How the finished app would be

**A normal Windows program.** Not a runtime to install, not a toolchain, not a Python environment.

- **Windows is the first and only release target.** macOS and Linux desktops are out of scope for the first release. Linux is supported as an *execution* backend for agent work, not as an app to install
- **Two installer formats, both standard:** an MSI package for managed and enterprise deployment, and an NSIS `.exe` for everyone else. Both for 64-bit Intel and 64-bit ARM
- **Per-user install. Never elevated.** No administrator prompt, no machine-wide changes, no leaving a service behind
- **Everything needed ships inside the app.** The agent runtime is bundled and compiled. You do not install Rust, Node, Bun or a package manager
- **Automatic updates, on your terms.** A signed manifest is the only thing that can install an update. Stable and beta channels, a check shortly after launch and periodically after, a download in the background with visible progress, and an install that takes effect when you choose to restart — never in the middle of your work
- **Supported upgrades are one version back.** Anything older installs the current build directly, and a downgrade on top of newer data is refused rather than attempted
- **Your data lives in one visible folder** in your user profile, containing the encrypted vault, the audit log, your workspaces, your models and your update channel. Uninstalling removes the program and leaves your data alone. Removing your data is a separate, explicit action

## Your data and privacy

Your files, sessions, memory, calendars, bindings and audit trail stay on your machine, inside that one folder. There is no account, no telemetry, and no analytics.

Three things ever leave, and all three are things you asked for:

1. The content of a turn goes to the model provider or agent **you** configured.
2. A URL goes out when you ask for a page to be read.
3. The updater asks a manifest server which version you are on, so it can tell you about a newer one.

That's the entire list. If you want no network at all, the application is fully functional offline.

Your provider keys sit in an encrypted vault, on your machine, under a passphrase only you know. There is no back door: a forgotten passphrase means that data is unreadable by design, and the recovery path is to remove it and start fresh. The agent itself can *use* a key; it can never *read* one.

## How it compares

The honest way to compare is against the categories you would otherwise have to choose between.

### The foundations

| | **AgentCowork** | Chat-first assistants | Repo-scoped coding agents | Cloud agent platforms | General desktop automation |
|---|---|---|---|---|---|
| Runs entirely on your machine | Yes | Usually | Yes | No | Yes |
| Your files never leave it | Yes, except what you ask it to send | Varies | Varies | No, by design | Varies |
| Bring your own model and keys | Yes | Sometimes | Yes | No | Rarely |
| Operates your real desktop apps | Yes | Rarely | No | No | Yes |
| Reads and edits Office and PDF files | Yes | Reads only | No | No | No |
| Permission decisions you can see and set | Yes, one decider, every action | No | Usually a sandbox, not a decision | Varies | No |
| Tamper-evident log of every effect | Yes | No | No | Some | No |
| Runs offline | Yes | Rarely | Yes | No | Yes |
| Automatic signed updates | Yes | Not applicable | Not applicable | Yes | Rarely |

### Where the time horizons differ

This is the part that matters most, and the part where the categories are thinnest.

| | **AgentCowork** | Chat-first assistants | Repo-scoped coding agents | Cloud agent platforms | General desktop automation |
|---|---|---|---|---|---|
| Work survives a closed laptop | Yes — recorded step by step, resumed from recorded state | No; the thread *is* the work | Conversation only; local execution stops when it exits | While the run is alive, and time-boxed | No |
| Keeps working while you are away | Yes — detached lanes and schedules, holding back whatever needs a decision from you | No | No, unless the work was moved to a cloud agent | Yes, within quota and session limits | No |
| Remembers you across sessions | Yes — scoped to projects, inspectable, and changed facts supersede old ones | Conversation only | Short project notes | Varies | No |
| A retry can never apply the same effect twice | Yes — every step is idempotent | No | No | No | No |
| Progress without reading transcripts | Receipts per action, with the evidence attached | No | Diffs and branches | Pull requests and diffs | Screen recordings |
| Recurring and scheduled work | Yes, with triggers and repeatable pipelines | Rarely | Rarely | Yes — the category's strongest feature | No |
| Cost ceilings that stop work | Named budgets, checked before the work starts, then stop and report | No | Quotas that warn, or block after the fact | Spend caps, sometimes with enforcement lag | No |
| Multiple agents across days | Subagents with their own sessions and isolated working copies, reporting back as receipts | No | Some, experimental, and lost on resume | Parallel runs, reconciled by hand | No |

**What those tables do and do not claim.** Every AgentCowork cell describes what this product is specified to do, not what is shipping today. The other columns describe categories, not audited claims about named products — we studied several of the leading tools closely, but software in this space changes weekly and we would rather describe ourselves accurately than make a competitor claim we cannot re-verify. The long-horizon table is deliberately built on durability rather than on the more impressive-sounding promise: no product in this category verifiably keeps *executing* untouched work on your machine for days, and we do not pretend otherwise. What is specified here is narrower and real — the work is recorded, it resumes, and you are never guessing what happened.

**What we deliberately did not copy.** We are explicit about the sources of our design, and about what we rejected from them. We took proven *patterns* — a phased memory pipeline, a ranked repository map, a dual-era protocol client, a scheduler with occurrence leases — and reimplemented them rather than vendoring code. We also recorded, and deliberately did not take: plaintext credential storage, keys visible to a gateway, fail-open defaults, a language model as the authority on what matters, per-engine approval rights, a second engine or a second router, micro-compaction by default, credentials in config files, and a long tail of small defaults that quietly trade safety for convenience.

## Who it is for

- **Founders and operators** who want work actually completed on their machine, with a record of what happened, and without uploading their company files to someone else's cloud
- **Engineers** who want an agent that can read the whole repo, run the tests, and work in an isolated copy — while every privileged action still passes a check they control
- **Power users** who already have an agent CLI they like, and want it to have memory, a browser, their documents, and a permission model, instead of a sandbox

## For developers

AgentCowork is a desktop shell over a governed kernel. The short version:

- **Surfaces propose; the kernel disposes.** The UI, the CLI and any future surface are projections. Every mutating effect is authorised in one place, and no credential ever leaves the vault
- **Work is the universal unit.** A chat turn, a scheduled job, a workflow run, a subagent task and an automation all share one lifecycle
- **One path for every effect:** request → capability → provider → handle → policy floor → ticket → action → verification → receipt → event
- **A capability is not a provider.** Semantic operations compose with interchangeable implementations behind one contract
- **Memory is not context.** Durable scoped knowledge versus what is worth spending tokens on this turn
- **Deterministic work never calls a model.** Rendering, browsing, indexing and navigation are operating-system work

The architecture is documented in full: the product contract in `AGENTCOWORK-SPEC.md`, the behavioural requirements in `ARCH/08-REQUIREMENTS.md`, the system design in `ARCH/03-HLD.md`, and per-module detail across `ARCH/10` through `ARCH/44`. `ARCH/00-INDEX.md` is the door. Delivery status is tracked in `TODO.md`, and the operating contract for coding agents working in this repository is `AGENTS.md`.

```
crates/        Rust kernel: trust, vault, audit, memory, office, browser, protocols, storage
packages/      TypeScript sidecar: the shared services a turn calls into
ui/            React cockpit: the desktop interface
src-tauri/     Tauri shell: a thin native layer
ARCH/          architecture documentation
scripts/       continuous-integration gates and verification tooling
```

Build and check it:

```bash
(cd crates && cargo build)     # Rust kernel
(cd crates && cargo test)      # Rust test suite
(cd crates && cargo clippy)    # Rust lints

pnpm install                   # JavaScript workspace
pnpm -r test                   # TypeScript test suites
(cd ui && pnpm run type-check) # interface typecheck

node scripts/check-arch-invariants.mjs   # architecture rules
node scripts/check-doc-refs.mjs          # documentation references
```

## Naming

AgentCowork is a working name. Package and crate identifiers use an `agentcowork-` prefix; the historical `everyaios` spelling survives only where it is load-bearing — the rename record in `ARCH/01-NAMING.md`, the decisions that describe the rename in `ARCH/04-DECISIONS.md`, and the legacy-path fallback in `docs/install-layout.md`.

## License

Dual-licensed; the licence files are in this repository. Third-party attributions, including the licensing terms of everything studied and absorbed, are recorded in `THIRD-PARTY-NOTICES.md` and in the absorb register at `ARCH/44-ABSORB-REGISTER.md`.
