# 01 — Naming & Brand Map (v1)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P0) — provisional **working names** per product-owner direction (2026-09-26). Nothing here is final branding; the layer is centralized so a rename is mechanical.
> **Rule of use:** all v1 docs use the **v1 names** only. v0 names appear only in the rename map below or in explicit historical notes.
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).

## 1. Working names

| Concept | v1 working name | v0 name | Owner doc | Notes |
|---|---|---|---|---|
| Product / platform | **AgentCowork** | EveryAIOS / EAIOS | `AGENTCOWORK-SPEC.md`, `ARCH/02-THESIS.md` | Working name “for now”. Formal long name open (OQ-001). |
| Runtime / kernel | **Core** (“AgentCowork Core” in formal prose) | EveryCore | `ARCH/03-HLD.md` | The brain; all surfaces are projections of it. |
| Agent engine | *no first-party engine in v1* | EveryAgent | `ARCH/15-AGENT-PLANE.md` | Engines are external in v1. The first-party engine is developed outside this repository and bound later as an ordinary binding — same `AgentEngine` contract, no privileged path into Core (DEC-052). |
| Digital world model | **World Model** (the World) | EveryWorld | `ARCH/21-WORLD-MODEL.md` | Continuously updated structural map of the machine. |
| Office runtime | **Office Runtime** | EveryOffice | `ARCH/22-OFFICE.md` | Lives under the universal document surface — not a sidebar mode. |
| Browser runtime | **Browser Runtime** | EveryBrowser | `ARCH/23-BROWSER.md` | Managed Chromium default + selectable adapters. |
| Repository intelligence | **RepoGraph / RepoMap** | same | `ARCH/26-CODE.md` | Graph + token-budgeted projection. Names kept. |
| CLI (future) | `agentcowork` (placeholder) | `every …` | `ARCH/32-CHANNELS.md` | Final binary name decided with the CLI surface (OQ-005). |
| Code identifiers | `agentcowork-*` | `everyaios-*` | `ARCH/04-DECISIONS.md` DEC-053 | Unfrozen and renamed 2026-09-27. The old names survive only in the rename map below and in the decision record. |

## 2. Rename map (reading v0)

| v0 term | v1 term |
|---|---|
| EveryAIOS | AgentCowork |
| EAIOS | (retired shorthand — use AgentCowork) |
| EveryCore | Core |
| EveryAgent | *(retired — the first-party engine is external; DEC-052)* |
| EveryWorld | World Model |
| EveryOffice | Office Runtime |
| EveryBrowser | Browser Runtime |
| EveryRepo | RepoGraph / RepoMap |
| `everyaios-*` (code) | unchanged until the code-phase rename |
| `every …` CLI | `agentcowork …` (placeholder) |

## 3. Rules

1. **Docs use v1 names only.** The only places v0 names may appear: this map; archive references; explicit “v0 called this X” history notes.
2. **Renames are decisions.** Changing any working name = update this doc + a `DEC` entry in `ARCH/04-DECISIONS.md` + a mechanical sweep of docs.
3. **Commit hygiene** (carried from `AGENTS.md` §7): never attribute an edit to the authoring tool, model, or agent in commits, code, or docs. Naming external systems as prior art or absorbed technology (e.g. Codex, OpenCode, Cline) is allowed when the reference is about that software. This is separate from product naming.
4. **Code identifiers follow the product name — superseded 2026-09-27 (DEC-053).** The earlier freeze held `everyaios-*` and `EveryAIOS` through the docs phase; that freeze is lifted and the rename executed across crates, packages, scopes, imports, strings, the data home (`~/.agentcowork`, env `AGENTCOWORK_*`, with a legacy fallback and a Core-owned one-time migration). The `acpx` CLI binary keeps its own name. OQ-003 closes here.
5. **Product positioning** does not rename with the brand: AgentCowork is described as an AI-native execution environment layered on the user’s existing computer — never as an OS/kernel replacement (`ARCH/02-THESIS.md`).

## 4. Open

- **OQ-001** — product shorthand for UI copy (“AC”, “Cowork”, or none). Decide before UI copy freeze (P5).
- ~~**OQ-003** — timing + scope of the code identifier rename~~ → **closed by DEC-053** (2026-09-27).
- **The agent engine is external.** No first-party engine ships in v1 (DEC-052). The first-party engine is developed outside this repository and is bound here afterwards as an ordinary binding — same `AgentEngine` contract, same Guard, no privileged path. It is not named in this table because it is not a component of this product; it is an engine binding like any other.
