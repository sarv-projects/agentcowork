# 17 — Memory

> **DEC-054 amendment:** Mission state is not memory. Goal contracts, PlanNodes, decisions, assumptions and evidence live in Mission/event stores; only durable reusable knowledge is eligible for Memory under existing provenance/consent rules. Agent-native memory stays native and is not harvested by discovery (`35`, `46`).

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P2).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-MEM-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Consumers:** `ARCH/16-CONTEXT.md` (recall), UI (Memory screen), `ARCH/32-CHANNELS.md` (external-agent projection).
> **Dependencies:** `ARCH/10-KERNEL.md`, `ARCH/30-EVENTS.md`, `ARCH/18-MODEL-ROUTING.md` (extractor call), `ARCH/12-TRUST.md` (authorization + audit).
> **Evidence:** `ARCHIVE/v1-research/memory.md` — a source-read survey (Claude Code, Codex, OpenCode, Grok Build, Hermes, mem0, Graphiti, Letta, NOOA, claude-mem, anything-llm) with `path:line`/URL citations. Decisions from this module are registered as `DEC-*` in `ARCH/04-DECISIONS.md`; unresolved items remain `OQ-MEM-*` (§14).

## 1. Purpose & responsibilities

Durable, scoped, provenance-carrying knowledge that survives sessions — deliberately **not** per-turn context.

**Owns:** the durable store (SQLite + FTS5); the write pipeline (extraction → validation → persistence); the recall primitive; lifecycle (dedup · supersede · forget · expiry); sensitivity handling at the store boundary; export/import; the audit trail of memory mutations.
**Never owns:** context assembly and budgeting (the bound engine via `16-CONTEXT`); work state, checkpoints, approvals (`11-WORK`); project rules files (`AGENTS.md`-style — user/repo authored); secrets (rejected, never stored); artifacts (`29-ARTIFACTS`).

Locked rules:

| Rule | Meaning |
|---|---|
| **Memory ≠ context** (P-10) | `recall()` returns ranked **candidates**; the Context Controller decides inclusion. |
| **Budget is a maximum** | Zero **query-relevant** hits ⇒ zero tokens in the relevant block; the always-on block is separately budgeted. Injection is measured on **rendered output** and degraded by dropping **whole items**, never truncating one. |
| **Non-touching read** | Injection never mutates memory state (no counter/salience bump); only explicit use may. |
| **Off the hot path** | Extraction runs after settled boundaries; a memory failure never blocks or fails a turn. |
| **Inspectable & deletable** | Every item shows scope/source/created-at; per-item delete, per-scope wipe, export. |
| **No rewrite** | The extractor may only `ADD` a new item or `SUPERSEDE` a known one; stored text is never rewritten by a model. |

Rationale (§0–§3 of the evidence): shipping systems converge on keyword-searchable durable notes + a tiny always-on index (Claude Code, Codex, Letta MemFS, Grok Build); the field's best-evidenced weakest point is stale/conflicting memory (TEPA arXiv:2608.07429; STALE arXiv:2605.06527) — hence **supersede pointer + permanent forget-with-suppression are day-1**, while vectors, graph, decay and consolidation loops are deferred behind a metric.

## 2. Concepts

### 2.1 Scopes — where an item lives, who may see it

| Scope | Keyed by | Lifetime | Written by | Typical content |
|---|---|---|---|---|
| `session` | session id | session anchor (archive/hibernation/last-active) + 7 d TTL | extractor | working context, session summary |
| `task` | task/run id | task lifetime + archive | extractor + agent | task summary, decisions taken, open threads |
| `project` | stable project identity (`DM-024 project_identity`, not a raw path — DEC-040) | durable | extractor + user | conventions, environment quirks, architecture decisions, gotchas |
| `user` | user id | durable | user-explicit; extractor (preferences only) | preferences, working style, profile |
| `org` | org id | durable — **schema-ready, v1 disabled** | — | shared conventions (future multi-user) |

The **permitted scope set is actor-derived**: a caller may pass at most a narrowing filter; the service computes what the actor binding can reach (never trusting caller-supplied scopes — DEC-042).

### 2.2 Kinds — what an item is

`preference` · `fact` · `decision` · `reference` (pointer/where-to-find) · `summary` (episodic roll-up for session/task).
No `skill` kind — procedural know-how is a capability/skill concern (`31-SKILLS-PLUGINS`), not a memory item.

### 2.3 Deliberately NOT memory

- **Project rules** — `AGENTS.md`-style instructions read at session start from the repo; the extractor never writes them; it may only *propose* a diff through approval (OQ-MEM-02).
- **Work state** — steps, checkpoints, approvals, task graph belong to `11-WORK`; the checkpoint (`DM-006`) is authoritative (DEC-041); memory may hold a `summary` that *references* a checkpoint/session through `source_ref`, never a second timeline and never served as work state.
- **Transcripts** — session history is the session plane's; memory stores extracted items only.

## 3. Data model — `DM-018` (`17`-local table/field names; the `DM-MEM-*` grouping is informal)

One SQLite file (app data dir), WAL mode, `Core`-owned, **encrypted at rest** (whole-DB, DEC-039). FTS kept in sync by triggers; `PRAGMA user_version` carries the schema version.

```sql
-- Per connection: PRAGMA foreign_keys=ON (SQLite default is OFF);
--                  PRAGMA busy_timeout=<bounded, default 5 s>; PRAGMA secure_delete=ON (DEC-039).

CREATE TABLE memory_items (
    id             TEXT PRIMARY KEY,                 -- uuidv7
    scope          TEXT NOT NULL CHECK (scope IN ('session','task','project','user','org')),
    scope_ref      TEXT,                             -- session_id | task_id | project_identity | user_id | NULL(org)
    kind           TEXT NOT NULL CHECK (kind IN ('preference','fact','decision','reference','summary')),
    content        TEXT NOT NULL,                    -- one atomic, self-contained item; ≤4 KiB (default; oversize rejected at validate)
    byte_size      INTEGER NOT NULL,                 -- measured UTF-8 bytes; feeds the per-scope byte bound
    content_hash   TEXT NOT NULL,                    -- keyed digest (store-local key) of normalized content: dedup + suppression
    hash_version   INTEGER NOT NULL DEFAULT 1,       -- normalization function version (REQ-MEM-027)
    dedup_key      TEXT,                             -- optional stable key, e.g. 'project.build.test_cmd'
    sensitivity    TEXT NOT NULL DEFAULT 'personal'
                   CHECK (sensitivity IN ('public','personal','confidential')),
    trust_tier     TEXT NOT NULL DEFAULT 'derived_untrusted'
                   CHECK (trust_tier IN ('user_explicit','agent_asserted','derived_untrusted','import')),
    source         TEXT NOT NULL,                    -- 'user' | 'agent:<id>' | 'extractor:<model>' | 'import'
    source_ref     TEXT,                             -- turn/event/artifact/checkpoint ids (provenance)
    confidence     REAL NOT NULL DEFAULT 1.0,        -- extractor-reported, inspect-only; no ranking role in v1
    pinned         INTEGER NOT NULL DEFAULT 0,       -- user-pinned: never auto-pruned
    used_count     INTEGER NOT NULL DEFAULT 0,       -- bumped only by explicit use (non-touching read)
    last_used_at   INTEGER,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL,                 -- set on any row mutation; clamped monotone (REQ-MEM-025)
    expires_at     INTEGER,                          -- session TTL; NULL = durable
    superseded_by  TEXT REFERENCES memory_items(id) ON DELETE CASCADE
                   -- contradiction pointer; NULL = current. Deleting the superseding head cascades to the rows
                   -- it superseded: never dangling, never resurrecting an older item.
);
CREATE INDEX idx_mem_scope   ON memory_items(scope, scope_ref, kind);
CREATE INDEX idx_mem_hash    ON memory_items(content_hash);
CREATE INDEX idx_mem_key     ON memory_items(dedup_key) WHERE dedup_key IS NOT NULL;
CREATE INDEX idx_mem_current ON memory_items(scope, scope_ref) WHERE superseded_by IS NULL;

CREATE VIRTUAL TABLE memory_fts USING fts5(
    content, content='memory_items', content_rowid='rowid', tokenize='porter unicode61');

-- External-content FTS5 requires explicit sync triggers — part of the store definition.
CREATE TRIGGER memory_items_ai AFTER INSERT ON memory_items BEGIN
    INSERT INTO memory_fts(rowid, content) VALUES (new.rowid, new.content);
END;
CREATE TRIGGER memory_items_ad AFTER DELETE ON memory_items BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
END;
CREATE TRIGGER memory_items_au AFTER UPDATE OF content ON memory_items BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
    INSERT INTO memory_fts(rowid, content) VALUES (new.rowid, new.content);
END;

CREATE TABLE memory_suppressions (                   -- explicit forgets survive re-extraction and import
    content_hash TEXT PRIMARY KEY,                   -- same keyed digest as memory_items.content_hash
    scope        TEXT NOT NULL,                      -- provenance only; the key is global (a suppressed hash stays suppressed everywhere)
    scope_ref    TEXT,
    created_at   INTEGER NOT NULL
);

CREATE TABLE memory_jobs (                           -- extraction bookkeeping (leases/retries/debounce)
    job_key      TEXT PRIMARY KEY,                   -- 'session:<id>' | 'task:<id>'
    status       TEXT NOT NULL CHECK (status IN ('pending','running','done','error')),
    lease_until  INTEGER,                            -- writer holds the job until this time; an expired lease is reclaimable
    retry_at     INTEGER,                            -- next eligible attempt (backoff)
    retry_remaining INTEGER NOT NULL DEFAULT 3,
    last_error   TEXT, watermark INTEGER,            -- watermark = last harvested session-event seq / turn index (idempotent harvest)
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);

CREATE TABLE memory_session_anchors (               -- the persisted anchor the 7 d session TTL evaluates against (§7, REQ-MEM-025); added at schema version 2
    session_id  TEXT PRIMARY KEY,
    anchor      TEXT NOT NULL CHECK (anchor IN ('archived','hibernated','last_active')),
    anchored_at INTEGER NOT NULL
);
```

- **Pointer policy:** FK enforcement is on per connection; `superseded_by` is cascade-collapsed (no dangling pointer, no resurrection). User forget of a current item never resurrects its predecessor; forgetting a superseding item removes the chain it headed (audited).
- **Erasure policy (DEC-039):** forget pages are overwritten (`secure_delete`), the WAL is checkpointed/truncated after forget, FTS rows are removed through the delete trigger, and the claim is bounded by the stated threat model (no secure erase from OS caches, backups, or flash wear-leveling).
- **Key custody (DEC-039 + INV-02):** the whole-DB key is vault-held and supplied by the caller, applied with `PRAGMA key` before any schema read so a wrong key surfaces as a read error rather than a silent plaintext fallback. **The product path never opens a plaintext store.** A keyless open is a test/ephemeral-session affordance only; when the store is wired into the product path the two cases become separate constructors rather than a nullable argument, so "no key" cannot be reached by forgetting one.
- **Session anchors:** one row per session, written by the anchor event (`archived`, `hibernated` where the retention class allows, `last_active` as fallback). Session-scope expiry reads `anchored_at` — never a live wall-clock delta, so a clock jump cannot extend or cut a TTL (REQ-MEM-025, EDGE-177). The table arrives at schema version 2 through the idempotent `user_version` migration (REQ-KERNEL-006); a missing anchor row is not a silent "now".
- **FTS integrity:** `integrity-check`/row-count parity detects drift; repair is FTS5 `rebuild` followed by post-rebuild verification (REQ-MEM-001).
- **Jobs GC:** `done` rows older than 7 d and `error` rows older than 30 d are swept by the single writer (defaults; product knobs). A lease older than its term is reclaimed with an audit entry.
- Temporal columns (`valid_at`/`invalid_at`) are intentionally absent — the single `superseded_by` pointer covers explicit reversal; add them with upgrade U10.

## 4. Interfaces — `CTR-008` (`memory.*` operations; the `CTR-MEM-*` grouping is informal)

| Interface | Signature (semantic) | Notes |
|---|---|---|
| `memory.recall` | `(query, scope_filter?, token_budget) → RankedItem[]` | Candidates only; the Context Controller decides inclusion. `scope_filter` may only **narrow** the actor-derived permitted set; no caller-supplied ceiling. Outcome is reported as hit / abstain / error. |
| `memory.remember` | `(content, scope, kind, sensitivity_raise?) → id` | Synchronous, user-explicit; may raise sensitivity, never below the source floor; passes the same validate step. |
| `memory.forget` | `(id \| scope_wipe) → void` | Hard delete + suppression + audit; permanent; chain-collapse per §3. Pinned-item delete and wipe require explicit confirmation. |
| `memory.inspect` | `(query \| scope) → items` | Actor-derived scopes; Memory UI data (provenance, pin, edit, delete). |
| `memory.export` / `memory.import` | `(format: json \| md) → file` / `(file) → result` | Export carries the suppression set with the items; import re-runs hash + secret + **suppression** checks, merges suppressions (never removes one), remaps ids/scopes (project identity is not assumed portable), lands as `source='import'` / trust tier `import`. |

**Mutation classification (DEC-042).** Memories are **local persistent mutations**: policy-gated per scope, audited, and **not** per-write tickets. Boundary-crossing operations — export/import file IO, sharing, anything leaving the machine — follow the guarded path (pathfloor / egress / tickets as applicable). Every mutation (including forget/wipe/import and policy-driven deletes) is audited, and the audit record carries **no item body**.

**Events emitted:** `memory.item.added` · `memory.item.superseded` · `memory.item.forgotten` · `memory.extraction.run` (counts, model, token cost, failures) — registered in `30-EVENTS` — plus `memory.recall.outcome` (hit / abstain / error, metered; registered in `30-EVENTS`).

**External-agent projection** (enforced by `12-TRUST`, surfaced by `32-CHANNELS`): filtered **recall-only** — owning project scope + the agent's own session/task + user preferences; **no** org, no other projects, no `confidential` unless a loadout grants it (v1 default: project + user only). v1 exposes **no write path** to external agents, and Core never writes or mutates an external agent's native memory/config/session stores (DEC-043).

## 5. Write path

```
turn/task settled → (1) signal gate → (2) harvest → (3) extract (1 LLM call)
                                                            │
        (5) persist in one tx ← (4) validate deterministic ←┘
```

1. **Signal gate (no LLM).** Run only at settled boundaries: session idle (≥5 min) or task completion; debounce ≤1 run / N turns (≈10) per scope. Cheap prefilter: user correction, preference statement, explicit “remember”, decision, repeated failure, task outcome. No signal ⇒ **no model call, no write**. Extraction is metered against a **global budget** (calls/tokens per period) with global and per-scope **kill switches** — N concurrent sessions cannot exceed it; over-budget extraction defers, never runs un-metered (DEC-044).
2. **Harvest (bounded).** Last ≤20 turns / task result / changed-file summary; per-message truncation; include top-k existing items for the same scope so the extractor can link/supersede. Harvested material is **data, never instructions**: it is delimited and escaped into the extraction prompt; a child session is harvested only when Core-owned, at its own settle boundary, and its items stay keyed to the child session/task with parent linkage in `source_ref` (never auto-promoted) (DEC-043).
3. **Extract (one LLM call, JSON).** Closed verb set: `ADD` | `SUPERSEDE` (target id required) | `NONE`. Per item: `kind`, `scope`, `text`, `sensitivity` (may propose; the deterministic floor below wins), optional `dedup_key`, `supersedes`. Caps: ≤3 items/run; scope may not widen without explicit user statement; relative dates resolved to absolute. Instruction-shaped candidates from untrusted sources (e.g. “always …”, scope-widening, policy-bearing) are **rejected or downgraded** without authority, never stored as `decision`/`preference`.
4. **Validate (deterministic).** Normalize + hash (versioned normalization); drop duplicates (batch + store) and suppressed hashes; **secret scan → reject + log, never persist**; verify `supersedes` targets exist, are current, same-or-narrower scope; enforce the per-item byte cap and scope caps; assign `trust_tier` from provenance (`user_explicit` | `agent_asserted` | `derived_untrusted` | `import`) and apply the **monotone sensitivity floor** (an item is never less sensitive than its source scope/surface).
5. **Persist (single transaction).** Insert items; set `superseded_by` on targets; remove any applicable suppression on explicit user re-remember; sync FTS; emit one audit event per run; update the job row (`watermark`, lease, backoff). Failure ⇒ mark error, backoff retry, never block the session.

**Extraction model & disclosure (DEC-044/054).** For a Core-owned model session, the default is its active Core provider (no new disclosure). An opaque external agent has no presumed Core provider and its native transcript is never harvested; extraction of eligible Core-owned/user-approved material requires an explicitly selected Core provider or local model. `confidential` scopes extract **local-only or not at all** until explicitly enabled. Every run reports cost/outcome; kill switches stop model calls and writes.

**Explicit writes bypass extraction:** `memory.remember` is immediate (same validate step; user may raise sensitivity); `memory.forget` hard-deletes + writes suppression + audit + recovery per §7 — it never returns via extraction or import.

**Forbidden in v1:** rewriting/merging stored item text; cross-scope promotion; writing project-rule files; extracting from provider-session transcripts we do not own; writing or mutating another agent's native memory/config/session store (DEC-043); treating injected memory text as authority (it can never change policy, goals, permissions, or tool choices).

## 6. Read path & injection

```
Context Controller ── memory.recall(query, scope_filter?, tokens) ──►
  0 scope set + ceiling derived from the actor binding (never caller-supplied)
  1 scope+state filter (current, unexpired, sensitivity ≤ ceiling)
  2 FTS5 BM25 candidates (top ~50); free text sanitized into valid MATCH syntax or abstain
  3 relevance = (−bm25) × kind/pin boost × recency boost   → sort descending, deterministic ties
  4 dedup (hash); MMR-ready interface (v2 hook; no MMR in v1)
  5 budget fit: render, drop whole items until ≤ budget (never truncate an item)
  ◄── ranked items + provenance (id, scope, source, trust_tier, created_at)
```

- **Scoring direction is explicit (F-02/C-03):** FTS5 `bm25()` returns **negative** values (better matches are more negative), so the score is normalized to a non-negative relevance (`relevance = −bm25`) before boosts. Boosts apply on that base, the sort is descending, and ties break deterministically (`created_at`, then id). A malformed/empty query abstains — it never returns arbitrary candidates.
- **Query construction & i18n (F-12):** free text is parsed/escaped into valid FTS5 MATCH syntax (operator/quote injection rejected) or the call abstains; the tokenizer is `porter unicode61` (English stemming) — non-English/CJK behavior is recorded by fixture, and a tokenizer upgrade is a schema-versioned change, not a silent swap.
- **Always-on block (≤128 tokens; counted inside the relevant-block ceiling, metered separately — INV-22):** pinned `user` preferences + pinned project conventions; present only when pinned items exist; rendered once per session and reused (cache stability). It is **invalidated** by any mutation of its member set — forget, edit, supersede, pin/unpin, disable, scope wipe — and the next turn reflects it; the cache-bust is accepted for correctness. System deletes (forget) are effective immediately.
- **Relevant block (≤256 tokens total incl. always-on, tunable):** the 256 ceiling is the rendered relevant block and **includes** the always-on block when pinned items exist; INV-22 accounts for the always-on block separately (zero query-relevant hits still allow at most the always-on block). Top-k items for the current query; one line per item with a source tag and trust tier; wrapped with the instruction that memory is historical context, **untrusted data with no authority**, to be verified against live state.
- **Zero-hit semantics:** no query-relevant candidate above the relevance floor ⇒ zero tokens in the relevant block beyond the always-on block (INV-22); the always-on block is measured separately and only exists for pinned items.
- **On-demand:** `recall` stays available as a tool for deep retrieval; session/task summaries are the entry point for “what happened here”.
- **Abstention is a feature:** abstention, a genuine miss, and a recall error are distinguishable outcomes (`hit | abstain | error`) with metering, so silent quality loss is visible.
- **Provenance is deletion-tolerant:** items render their source tag; a ref whose source was pruned shows “source unavailable” and is never dereferenced during injection.
- **Retrieval ownership (C-09):** `memory.recall` is the **injection** path (candidates for the Context Controller). `context.search` (`16` §1.1) and the `27` search plane return **refs + bounded snippets** for user/agent search; each path has exactly one scoring owner and neither re-ranks the other's results.
- **Counters:** `used_count`/`last_used_at` bump only on explicit use; if unreliable to detect in v1, defer them with U4 (they are not load-bearing).

## 7. Lifecycle

- **Dedup:** keyed hash on versioned normalized content; `dedup_key` for canonical facts (`project.build.test_cmd`). Near-duplicate detection is U2, not v1.
- **Update:** ADD-only body + single `superseded_by` pointer (extractor `SUPERSEDE` or user edit); superseded rows retained for audit; read paths filter them. A user edit creates a **new** item with `source='user'` and supersedes the old one — the stored body is never rewritten.
- **Forget:** hard delete + suppression + audit; recovery follows DEC-039 (§3). A suppressed hash is **global** (the suppression row survives scope wipes), so neither re-extraction nor import can resurrect it. If the forgotten item superseded older rows, the chain is removed with it — no dangling pointer, no resurrection — and **every hash the removal took with it is suppressed, not only the head**: suppressing the head alone would let a predecessor's text return one run later.
- **Scope wipe:** deletes the scope's items and superseded rows (and their FTS entries) but **retains suppressions**, which are not scope data; consequence (declared): content forgotten anywhere stays un-extractable everywhere, including inside a wiped scope. Requires explicit confirmation.
- **Expiry & TTL anchor:** only `session` scope has a default TTL — 7 d after the session **anchor** event (`archived`, or `hibernated` where the retention class allows; `last_active` is the fallback — `11` §7). Expiry is evaluated against the persisted anchor timestamp, never a live wall-clock delta (REQ-MEM-025).
- **Growth bounds:** per-item cap 4 KiB (reject oversize at validate); per-scope caps — session 500 · task 500 · project 5,000 · user 1,000 items with a declared byte bound (defaults; product-visible knobs). Eviction removes the oldest **unpinned** items first and audits each removal; a pinned item is never auto-evicted.
- **Pruning precedence:** summaries prune by count/age per scope, except **pinned summaries, which follow the pin rule** (never auto-pruned).
- **Sensitivity:** `public` (export/share later) · `personal` (default; leaves the device only inside an active provider call) · `confidential` (project-bound; never org-shared; marked in exports). Assignment: default `personal`, user actions may raise, the extractor may propose, and the deterministic **monotone floor** prevents an item from being stored below its source scope/surface (DEC-038). Secrets are **not a class — they are rejected** at the write path (regex + entropy scan, logged).

## 8. Failure modes & recovery

| Failure | Behavior |
|---|---|
| Extractor model call fails | Job marked error, retry with backoff; turn unaffected. Budget exhausted / kill switch on ⇒ extraction defers (zero model calls, zero writes). |
| DB **locked** (transient) | Bounded `busy_timeout` + backoff at the call site; the call degrades or defers — memory is **not** disabled for the session and the turn is never blocked. |
| DB **corrupt** (persistent) | Memory disabled for the session with a surfaced warning and an audit entry; the store is quarantined; a **repair path** (export readable rows → recreate/rebuild → re-import validated) is offered to the user; chat is never blocked. |
| Two writers contend (desktop + CLI/detached across processes) | Single-writer ownership: one writer host holds the store under a lease (owner + heartbeat); other processes route through it when present and otherwise reclaim a stale lease with an audit entry; WAL + bounded busy handling serialize the rest — no lost writes, no turn failure. |
| Clock jump / non-monotonic observations | Recency deltas clamp (a backwards clock never promotes an older item); TTL evaluates against the persisted anchor; job leases fail safe; skew events are recorded (REQ-MEM-025, EDGE-177). |
| FTS desync | Detection by FTS5 `integrity-check` / row-count parity; repair by `rebuild`; verification runs **after** rebuild; a failure to restore is surfaced, never silently ignored. |
| Secret detected post-hoc | `forget` + suppression + audit; scan corpus added to tests. |
| Stale recall served | Staleness annotation on every injected item + “prefer live state” instruction + abstention. |
| Recall error vs abstention | Both are explicit outcomes (`hit | abstain | error`) with metering — an error never masquerades as “nothing relevant”. |
| Memory disabled (per scope/user) | No injection, no retrieval, no writes, no background extraction. |

## 9. Security & privacy

- The **extractor call is a disclosure boundary** — eligible Core-owned material leaves the machine if a cloud provider model is used; Core-owned sessions may default to their active Core provider, while opaque external-agent work requires an explicit Core extractor choice and never harvests a native private transcript. `confidential` scopes are local-only or off until enabled; a global budget + kill switches bound cost and exposure (DEC-044/054).
- **Sensitivity is assigned at write and enforced at read:** one vocabulary (`public | personal | confidential`, canonical per `06` §0), default `personal`, monotone floor from source, ceiling derived from the actor binding (never a caller parameter); `confidential` items never leave their project scope (DEC-038, INV-10).
- **External agents receive the filtered recall projection only** (§4); Core never writes or mutates their native memory/config/session stores; provider-session transcripts we do not own are never harvested (DEC-043).
- **At rest:** the memory store is whole-DB encrypted (SQLCipher, key in the vault; WAL/temp inherit the encryption). Suppression digests are keyed, not plain content hashes, and audit records carry no item body. The erasure claim is bounded by a stated threat model — no secure erase from OS caches, backups, or flash wear-leveling (DEC-039).
- **Authorization classification:** memory writes are **local persistent mutations** — policy-gated per scope, audited, and not per-write tickets; boundary-crossing operations (export/import to disk, sharing) follow the guarded path; permitted scopes are actor-derived (DEC-042, INV-01/04/24).

## 10. Performance

- Recall p95 target ≤ 50 ms at 10k items (local FTS5), measured in the eval suite; the growth bounds are exercised at 1k / 10k / 50k items (§13).
- Extraction runs off the hot path; cost visible via `memory.extraction.run` events and bounded by the global extraction budget (DEC-044).
- Injection budget measured on rendered output; a zero-query-hit turn measures **zero relevant-block tokens**, with the always-on block measured separately (§6, INV-22).

## 11. Interop

**Depends on:** kernel (`10`), events (`30`), model routing (`18`, extractor), trust (`12`, authorization/audit).
**Exposes to:** context (`16`, recall), UI (Memory screen), channels (`32`, projection), workflows (task summaries as node context, later).
**DAG check:** memory never calls context or work; it only reads its own store and emits events.

## 12. Not in v1 — upgrade triggers

| # | Deferred | Upgrade trigger |
|---|---|---|
| U0 | Memory graph / entity store | multi-hop questions fail eval |
| U1 | Vector/hybrid retrieval | recall@k misses on paraphrase queries exceed threshold |
| U2 | LLM dedup/merge/rewrite | duplicate rate > 2% after N items |
| U3 | Markdown as system of record / editable store | users demand hand-editing |
| U4 | Decay/activation math | drift sim shows stale trivia polluting top-k |
| U5 | Autonomous consolidation / dreaming | task summaries prove insufficient |
| U6 | MMR / cross-encoder / fusion | near-duplicates flood the budget |
| U7 | Org sharing / multi-user sync | team mode appears |
| U8 | Import from Claude Code / Codex memory dirs | onboarding friction proves it |
| U9 | Agent-private memory stores | bindings need isolation |
| U10 | Bi-temporal columns | point-in-time queries requested |
| U11 | FSRS/taste-profile/strategy-zoo features | specific product demand (never new subsystems) |

Sequencing if metrics force upgrades: U1, U2 → U5, U4 → U0. Nothing is built without a failing metric.

**Schema versioning is not deferred:** `PRAGMA user_version` carries the store version, migrations run forward and are tested from every shipped version, and import/export remaps ids/scopes per the declared rules (REQ-MEM-027). A tokenizer or normalization-function change ships as a schema-versioned migration, never a silent swap.

**Code-phase re-home obligation (C-10; code is frozen — the doc set wins).** The frozen code carries a large multi-module memory implementation (graph, activation math, paging, taste/profile, a TS memory package) that contradicts this v1 minimal set (`DEC-018`). The doc set is the contract; the code phase must re-home or retire the divergent modules rather than treat them as v1 behavior. A `FIX`/evidence entry is owed to `42-EVIDENCE-MAP.md`.

## 13. Evaluation — v1 acceptance gates

1. **Write quality (fixture):** 100-turn scripted session → keep-rate ≥ 90%, duplicate rate < 2%, secret-leak count = 0.
2. **Update/conflict suite:** ~30 scenarios → stale-return on explicit supersede = 0; re-extraction after forget = 0; forget → **import** = 0; deletion of a superseding item leaves no dangling pointer and no resurrection (implicit conflicts are reported, not gated).
3. **Recall golden set:** 50–100 query→ids pairs → recall@5 ≥ 0.8; false-injection < 5%; abstention correct when no relevant memory; monotone ordering under boosts (a pinned item with worse raw BM25 still ranks first); malformed-query fuzz abstains.
4. **Budget honesty:** injected-token distribution vs ceiling; zero **query-relevant** hits ⇒ zero relevant-block tokens with the always-on block reported separately; whole-item drop only.
5. **Isolation, sensitivity & trust:** cross-project leakage = 0; external-agent view cannot see other projects/org/confidential; secret corpus never persisted; adversarial fixture (hostile page/tool output carrying “remember …”) produces no policy-bearing item, no scope widening, and recall grants no authority.
6. **Lifecycle & integrity:** delete/wipe leaves no FTS orphans; suppression digest is keyed; export→import round-trip per the declared remapping rules; schema migration tested from every shipped version.
7. **Ops:** recall p95 ≤ 50 ms @10k (1k/10k/50k growth run); extractor failure never affects a turn; back-to-back lock contention never disables memory or fails a turn; corruption quarantine + repair path surfaces without blocking chat; disabled ⇒ zero activity; extraction budget/kill switch honored.
8. **Drift simulation** (upgrade trigger for U4/U5): replay 30 simulated days; measure stale ratio, duplicates, injected-token waste, store growth vs the declared bounds.
9. **Time correctness:** simulated backwards clock never inverts recency ordering or TTL outcomes; lease expiry fails safe (REQ-MEM-025).

## 14. Open questions (`OQ-MEM-*`)

1. **Engine-private memory vs Core shared memory** — **resolved (DEC-043/054):** Core has one owned memory store; an external engine may independently own private native memory/notes. Core neither imports nor mutates that store through discovery, and its session log records only Core-observed events.
2. **Rules write authority** — extractor never writes project rules; may propose a diff via approval. *(still open as UX detail)*
3. **Extraction model** — **resolved (DEC-044):** session provider by default; `confidential` scopes local-only or off until enabled; global budget + kill switches.
4. **Encryption at rest** — **resolved (DEC-039):** whole-DB SQLCipher (key in the vault) + erasure policy and threat model; PEND-06 closed.
5. **Project identity keying** — **resolved (DEC-040):** bind the `project` scope to `DM-024 project_identity`, not a raw path; re-key rules for move/clone/rename/worktree; Windows canonicalization verification pending a Windows acceptance record.
6. **Org layer plumbing** — scope reserved and empty in v1; reserve `owner_id` now or migrate later. *(open)*
7. **Summary ownership** — **resolved (DEC-041):** the checkpoint is authoritative; memory summaries reference it and are never served as work state.
8. **Cross-agent recall permissions** — **resolved (DEC-043):** v1 default project + user only; no org, no other projects, `confidential` only with a recorded loadout.
9. **Counters** — instrument explicit-use counters in v1 or defer with U4. *(open; not load-bearing — §6)*
10. **Promotion UX** — explicit-only promotion of a project item to user layer; extractor may not propose. *(open)*
11. **Eval harness ownership** — fixtures/goldens location; LLM-judge gates manual/nightly, not CI-blocking. *(open)*
12. **Disabled semantics granularity** — per user / per project / per agent binding. *(partially resolved: §8 declares per-scope/user disable; binding-level granularity remains open)*

## 15. Evidence index

Primary: `ARCHIVE/v1-research/memory.md` (full citation list). Strongest anchors: Codex pipeline `clone2/codex/codex-rs/memories/README.md:29-152`; Grok Build memory crate `clone2/grok-build/crates/codegen/xai-grok-memory/src/*`; NOOA non-touching read `clone2/nooa/packages/nooa-memory/src/nooa_memory/schema.py:315-331`; mem0 ADD-only `clone2/mem0/mem0/memory/main.py:879-1195`; claude-mem budget `clone2/claude-mem/src/services/context/ContextBudget.ts:4-40`; Claude Code memory docs `https://code.claude.com/docs/en/memory`; TEPA `https://arxiv.org/abs/2608.07429`; STALE `https://arxiv.org/abs/2605.06527`; LongMemEval `https://arxiv.org/abs/2410.10813`.

## 16. Requirements (`REQ-MEM-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-MEM-001` | Memory v1 algorithm set: SQLite + FTS5 with sync-trigger DDL and a named integrity-check/rebuild; ADD-only extraction with `superseded_by`; suppression-based forget; no vectors/graph/decay (DEC-018) |
| `REQ-MEM-002` | Write discipline and non-blocking degradation: off the hot path, never fails a turn, secrets rejected, forget permanent; disabled means zero activity (INV-09) |
| `REQ-MEM-003` | Memory ≠ context: recall returns ranked candidates with provenance; the Context Controller owns inclusion (DEC-019) |
| `REQ-MEM-004` | Non-touching read: assembly and recall mutate nothing; counters bump only on explicit use; block invalidation is not a read-side write (INV-08) |
| `REQ-MEM-005` | Scope model and lifecycle: five scopes, session TTL 7 d anchored to the session anchor event, org schema-ready but v1-disabled, pinned items never auto-pruned |
| `REQ-MEM-006` | Isolation, sensitivity and projection: canonical classes + write-time floor, ceiling from the actor binding, confidential stays project-bound, filtered external-agent view (INV-10, DEC-009/038) |
| `REQ-MEM-007` | Extraction trigger discipline: settled boundaries only, signal gate (no signal ⇒ no model call), debounce, bounded harvest |
| `REQ-MEM-008` | Extractor contract and deterministic validation: ADD/SUPERSEDE/NONE, caps, no scope widening, hash/secret/supersede checks, one transaction |
| `REQ-MEM-009` | Supersede semantics: ADD-only body plus `superseded_by` pointer; superseded rows retained and filtered; stale return = 0 |
| `REQ-MEM-010` | Forget and scope wipe are permanent: hard delete + suppression + audit; re-extraction and import = 0; no dangling pointers; no FTS orphans (INV-09, INV-24, DEC-039) |
| `REQ-MEM-011` | Inspect, export, import: provenance visible, per-item delete, per-scope wipe; imports revalidate and remap; round-trip per declared rules |
| `REQ-MEM-012` | Recall p95 ≤ 50 ms at 10k items; zero query-relevant hits ⇒ zero relevant-block tokens (always-on separately budgeted); whole-item degradation (INV-22) |
| `REQ-MEM-013` | Sensitivity assignment, vocabulary and per-surface ceilings (DEC-038, INV-10) |
| `REQ-MEM-014` | Extraction boundary: untrusted content, provenance trust tiers, no policy-bearing items (DEC-036/037) |
| `REQ-MEM-015` | Recall scoring correctness and query robustness: non-negative relevance, deterministic ties, abstention on malformed queries |
| `REQ-MEM-016` | Forget, supersede pointers and erasure integrity (DEC-039) |
| `REQ-MEM-017` | Growth bounds: item/per-scope caps, TTL anchor + sweeper, jobs GC (INV-06) |
| `REQ-MEM-018` | Concurrency, single-writer and corruption semantics: bounded lock handling, quarantine + repair (INV-06) |
| `REQ-MEM-019` | Mutation invalidation and frozen-injection budget semantics (INV-22) |
| `REQ-MEM-020` | Project identity, re-keying and cross-project isolation (DEC-040, DM-024) |
| `REQ-MEM-021` | Checkpoint is authoritative; memory summaries reference, never duplicate (DEC-027/041) |
| `REQ-MEM-022` | Multi-agent boundary: external agents read-only, no native-store writes, subagent harvest policy (DEC-043) |
| `REQ-MEM-023` | Mutation classification, authorization and actor-derived scope sets (DEC-042, INV-01/04/24) |
| `REQ-MEM-024` | Extractor budget, model policy and kill switch (DEC-044) |
| `REQ-MEM-025` | Time correctness and clock-skew resilience for ranking/TTL/leases |
| `REQ-MEM-026` | Provenance integrity under deletion and edit (DEC-032) |
| `REQ-MEM-027` | Schema versioning, migration and import/export fidelity |
