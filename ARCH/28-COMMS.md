# 28 — Communication (Connectors)

> **DEC-054 amendment:** Mail/Drive/Sheets and other SaaS capabilities resolve per action and OAuth scope; a globally connected account is not a grant to every Work or agent. Core-owned connector credentials remain in the vault and its calls pass Guard. An external CLI or agent may use its own credential store only under a visibly separate native/external custody and governance class (`46`, `45` cowork-os/Google Workspace CLI evidence).

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P3).
> **P7 pass (2026-09-26):** line-checked; requirements seeded (`REQ-COMMS-*`, Requirements section).
> **P9 verification pass (2026-09-26):** read line-by-line; fixes applied where needed (owner-directed; re-freeze follows).
> **Role:** an **agentic action layer** over communication systems — email, calendar, messaging — delivered as **capabilities over connectors**. Not another messaging client (owner brief).
> **Dependencies:** `13-CAPABILITY` (descriptors) · `14-PROVIDERS` (transport/auth) · `12-TRUST` (permissions, vault, egress) · `30-EVENTS` (arrival events) · `29-ARTIFACTS` (attachments) · `20-WORKFLOW` (triggers). **Consumers:** `15` (agent), UI (`32`).
> **Evidence:** product-owner brief (communication section: read/classify/draft/reply; calendar; Slack/Teams/WhatsApp/Discord; “don’t make it another messaging client”; permission defaults) · `ARCH/12-TRUST.md` §3 · `ARCH/13-CAPABILITY.md` · v0 connector evidence (`REPO-COMPARE/BRIEFS/02-connectors-nango.md`, `18-mcp-plugins-skills-connectors.md` — pattern reference only, not authority).

## 1. Purpose & rules

**Owns:** the connector framework (auth flows · credential refs · scopes · sync model · rate limits) · communication capability descriptors (`mail.*` · `calendar.*` · `messaging.*`) · event ingestion (`email.arrived`, `calendar.event.upcoming`, `message.received`) · draft/reply support · consent/records per connector instance.
**Never owns:** credential custody (`12` vault) · mail-client UI (`32`) · the user’s inbox as a mirror (we do not build one).

1. **Capabilities, not clients** — the product exposes verbs (`mail.search`, `calendar.availability`), never a second inbox UI.
2. **Sends are dangerous** — reading is sensitive, **sending is approval-gated** (ASK tier, `12` §3) with receipts (externally visible effect, INV-07).
3. **Connectors are providers** — native APIs or MCP/plugin connectors behind the `14` adapter contract; nothing above Capability knows the transport.
4. **No bulk ingestion** — query on demand + subscription events where offered; nothing is mirrored by default.

## 2. Connector framework

| Concern | Shape |
|---|---|
| Descriptor | id · provider · auth type (OAuth2 · API key · IMAP/SMTP creds) · scopes requested · capabilities offered · sync model (push/poll) · rate limits · data classes |
| Auth | OAuth/credential flows run locally; tokens land in the vault (`12` §6); scopes recorded; per-instance consent record (`21` §5 pattern) |
| Sync | on-demand queries + event subscriptions where the provider offers them; bounded local caches (headers) only where a capability requires it |
| Providers | native HTTP (Graph, Gmail, CalDAV, IMAP/SMTP) · MCP connector servers · plugin connectors (`31`) |
| Failure | expiry → `requires_user_action` “reconnect”; limits → backoff; partial responses flagged |

## 3. Capability surface (v1)

| Domain | Capabilities (L1 read → L2 act) | Risk |
|---|---|---|
| Mail | `mail.search` · `mail.read` · `mail.thread` · `mail.labels` → `mail.draft` · `mail.reply-draft` · `mail.send` (approval) · attachment via artifact refs | read: sensitive · send: dangerous |
| Calendar | `calendar.list` · `calendar.availability` · `calendar.briefing` → `calendar.create` · `calendar.update` (external invites approval-gated) | read: sensitive · write: sensitive→dangerous |
| Messaging | `messaging.search` · `messaging.read` → `messaging.send` (approval) | read: sensitive · send: dangerous |
| Web | `web.search` · `web.fetch` (bounded results/content + provenance) | read: sensitive · content is untrusted |

Every capability carries a descriptor, risk class, and verification hook (`13` §2); sends produce receipts with the exact content reference.

**Web search & fetch (`DEC-037`)** — capabilities, not the local search plane (`27` stays network-free):

- **`web.search`** — affordances: query · count (≤20) · freshness (`fallback|preferred`) · type · domain allow/block · locale. Providers: native/server-side search · MCP search server · browser-driven (degraded).
- **`web.fetch`** — affordances: url · format (text/markdown/html) · timeout (≤120 s) · max chars/tokens · `fresh` cache bypass. Providers: guarded local fetch · native fetch API · browser (JS-rendered pages only).
- **Custody & egress:** provider keys only via the vault (headers/refs — **never** keys in URLs, INV-02); all traffic through Guard egress with domain allow/block policy that **overrides model requests** (INV-05); SSRF floor (no localhost/no-dot/private/link-local/metadata; resolve-then-check).
- **Caps:** fetch 5 MB · search response 256 KiB · default 8 results (hard max 20) · synthesis context ≤10k chars · per-session search budget (default 200, counted across subagents).
- **Caching:** fetch per-session TTL (default 15 min) keyed `(normalized URL, format)` with an explicit `fresh` bypass; cached results always surface `retrieved_at`; never silently serve a different page.
- **Citations & provenance:** every result carries `{ref, url, title?, retrieved_at, …}` (+ `sha256` for fetched content); citations are never dropped or merged into prose without the source ref; full content stays as an artifact ref (`29`).
- **Safety:** fetched content is **untrusted input** — never instructions; URL-provenance option (fetch only URLs already present in the conversation); cross-host redirects are surfaced, not silently followed; no JS/anti-bot/CAPTCHA evasion (DEC-016); robots/ToS honored.

## 4. Events & triggers

Connector-originated events (`email.arrived` · `message.received` · `calendar.event.upcoming`) are published to the event store (`30`) with provenance + dedupe keys, where workflows may subscribe (`20` §5). Event payloads are **refs + metadata**, never full bodies; fetching content is a capability call with its own permission check.

## 5. Context & privacy

- Message content enters context only when needed, as bounded excerpts (`16` §2) — never bulk ingestion into memory.
- Default sensitivity: `confidential` (project-scoped) for message bodies; `personal` for user profile-ish metadata.
- Attachments move through the artifact gateway (`29` §5) with the connector’s permissions recorded.
- Multi-account: scopes isolate accounts; cross-account queries are explicit.

## 6. Sending & approval flow

`mail.send` / `messaging.send` / `calendar.create` follow: draft → **approval primitive** (`DEC-021`) with `edit` semantics (`20` §7: editable draft payload, immutable original) → send → receipt. Uncertain send outcomes (timeout after submit) go to `needs_attention` — never silent retry of an outbound message.

## 7. Failure modes

| Failure | Behavior |
|---|---|
| Token expired/revoked | `requires_user_action` re-auth guidance; no fallback to cached credentials. |
| Provider rate limit | Backoff + queue; surface if persistent. |
| Partial sync | Flagged freshness; queries report coverage. |
| Send uncertain | `needs_attention` + receipt with the attempt; no duplicate sends. |
| Abuse/spam guard | Rate caps per connector; bulk sends require explicit policy enablement. |

## 8. Interop

**Depends on:** `10` · `12` (auth/egress/approvals) · `13`/`14` · `19` (adapter execution hosts) · `29` · `30`.
**Exposes to:** `15` (comm actions), `20` (triggers), UI (`32`: compose/review surfaces live in the UI doc), `17` (explicit “remember this thread” promotions).
**DAG check:** connectors never write memory or work state directly; they act through capabilities and emit events.

## 9. Not in v1

Full mailbox mirror/search index · real-time chat presence/typing · SMS/voice · automated scheduling negotiation · consumer-only channels without stable APIs (best-effort adapters later).

## 10. Open questions (`OQ-COMMS-*`)

1. First connector set (proposal: Microsoft Graph + Gmail + CalDAV + Slack; IMAP/SMTP as generic fallback).
2. Read default: sensitive-tier ASK vs allow-with-consent-record (owner brief lists “email read: ASK”; confirm at SPEC).
3. Attachment policy: size/type caps + scan rules.
4. Multi-account UX + per-account scopes.
5. Whether messaging read ships in v1 or after mail/calendar prove the framework.

## 11. Evidence

Product-owner brief (comms capabilities; permission table; “not another messaging client”) · `ARCH/12-TRUST.md` §3 (dangerous tier), §6 (vault), §7 (egress) · `ARCH/13-CAPABILITY.md` §2–§3 · `ARCH/20-WORKFLOW.md` §5 (trigger feeding), §7 (edit semantics) · `ARCH/29-ARTIFACTS.md` §5 (gateway) · v0 `REPO-COMPARE/BRIEFS/{02,18}` (connector patterns — reference only).

## 12. Requirements (`REQ-COMMS-*`)

Testable behaviors owned by this module live in `ARCH/08-REQUIREMENTS.md`; the traceability chain is in `ARCH/09-FEATURE-MATRIX.md`. This table is a pointer, not a second copy.

| REQ | Behavior (one line) |
|---|---|
| `REQ-COMMS-001` | Communication ships as capability verbs (`mail.*`/`calendar.*`/`messaging.*`/`web.*`) — never a second inbox client or mirrored mailbox |
| `REQ-COMMS-002` | Connectors are providers behind the `14` adapter contract; nothing above Capability knows the transport (DEC-005) |
| `REQ-COMMS-003` | Connector descriptors + per-instance consent; auth flows local, tokens only in the vault (INV-02) |
| `REQ-COMMS-004` | On-demand queries + subscriptions; bounded caches; no bulk ingestion or default mirroring |
| `REQ-COMMS-005` | Sends follow draft → approval → send → receipt; uncertain outcomes land in `needs_attention` (DEC-021, INV-07) |
| `REQ-COMMS-006` | Risk classes: reads `sensitive`, sends `dangerous`; bodies default `confidential` (DM-011) |
| `REQ-COMMS-007` | Content is untrusted input; bounded excerpts only; attachments via the artifact gateway (DEC-037) |
| `REQ-COMMS-008` | Arrival events carry refs + metadata (never bodies) with provenance + dedupe keys (CTR-019) |
| `REQ-COMMS-009` | Account/scope isolation; cross-account access is explicit, never implicit |
| `REQ-COMMS-010` | Failures are typed and bounded: re-auth guidance, backoff, flagged coverage, rate caps — no cached-credential fallback |
| `REQ-COMMS-011` | `web.search` honors caps (≤20, default 8), returns cited results, and consumes the per-session budget across subagents (DEC-037) |
| `REQ-COMMS-012` | `web.fetch` is size-capped, cached per `(normalized URL, format)` with TTL + `fresh` bypass, and always surfaces `retrieved_at` (DEC-037) |
| `REQ-COMMS-013` | Web egress obeys vault-only credentials, Guard allow/block, the SSRF floor, redirect surfacing and no-evasion rules (INV-02/05, DEC-016) |
