# Repository Agent Kit (`.agents/`)

Portable, model- and agent-agnostic workflows for understanding and changing this
repository. Nothing here is specific to a particular IDE, model vendor, or MCP client.

## Layout

```text
.agents/
├── README.md
├── docs/
│   ├── README.md                          # index state vs. understanding artifacts
│   ├── installation.md                    # dependencies and real invocations
│   ├── architecture-and-protocol.md       # the 7-phase understanding protocol
│   ├── spec-driven-development.md         # spec → plan → code → verify protocol
│   └── agent-agnostic-compatibility.md    # capability surface, not product names
├── templates/
│   ├── AGENTS.template.md                 # blank project contract for new repos
│   └── SPEC.template.md                   # blank spec / requirements template
└── skills/
    ├── codebase-intelligence/
    │   ├── SKILL.md                       # routing contract for the skill
    │   ├── requirements.txt
    │   ├── references/
    │   │   ├── reference-architecture.md  # SQLite schema + edge taxonomy
    │   │   ├── understanding-pipeline.md  # layered pipeline overview
    │   │   ├── graph-schema.md            # node families, edges, confidence bands
    │   │   ├── retrieval.md               # query intents, fusion, evidence packs
    │   │   ├── providers.md               # preferred/optional/fallback matrix
    │   │   ├── security.md                # exclusions and redaction rules
    │   │   └── understanding-artifacts.md # docs/codebase/ contract
    │   └── scripts/codegraph.py           # local AST indexer + graph builder
    └── skill-creator/
        └── SKILL.md                       # portable skill-authoring workflow
```

## Design goal

Move an agent from *"I know where to look"* to *"I understand what this system does,
how its parts interact, what evidence supports that understanding, and what a change is
likely to affect."*

The default path is offline-first: no account, no external service, no embedding API.
Optional providers (LSP, SCIP, ast-grep, local vectors, CodeQL) can enrich it, but the
baseline stays useful on a fresh checkout.

## Quick use

From the repository root:

```bash
python3 -m pip install -r .agents/skills/codebase-intelligence/requirements.txt

python3 .agents/skills/codebase-intelligence/scripts/codegraph.py index
python3 .agents/skills/codebase-intelligence/scripts/codegraph.py report --top 20
python3 .agents/skills/codebase-intelligence/scripts/codegraph.py query "nativeCall" --refs
python3 .agents/skills/codebase-intelligence/scripts/codegraph.py path <from> <to>
```

The index lands in `.code-intelligence/`, which is disposable and gitignored.

## Provenance and local divergences

Derived from the agent-agnostic coding kit (v2). The following intentional divergences
exist so the kit matches this repository rather than the other way around:

- The codebase skill is named `codebase-intelligence` (upstream: `codebase-understanding`)
  and ships `scripts/codegraph.py`, a larger delta-aware indexer with PageRank, Merkle
  fingerprinting, and CLI `index`/`report`/`query`/`path`/`stats`/`export` subcommands.
- Upstream's conceptual pipeline diagram is kept as
  `references/understanding-pipeline.md`, because `references/reference-architecture.md`
  already documents this repository's concrete SQLite schema and edge taxonomy.
- `templates/AGENTS.template.md` is renamed from upstream's `templates/AGENTS.md` so it is
  not auto-loaded as an active instruction file for that directory.
- The repository root `AGENTS.md` is the single live contract. It merges this kit's
  universal sections with AgentCowork-specific sections 10–16; upstream's generic
  `AGENTS.md` is not installed separately.

Upstream `docs/` and `references/` describe `cbi.py` and its `doctor`/`understand`
subcommands. Those are adapted here to `codegraph.py`'s actual CLI. Adopt upstream content
by adapting commands, not by copying verbatim.
