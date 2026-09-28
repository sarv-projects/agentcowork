# 09 — Feature Matrix (REQ traceability)

> **Status:** Frozen v1 (frozen 2026-09-26; drafted P7). **This file owns links, never content:** requirements live in `ARCH/08-REQUIREMENTS.md`, designs in the module docs, tasks in `TODO.md`, evidence in `ARCH/42-EVIDENCE-MAP.md`.
> **Purpose:** answer from one place — which requirements does a file implement · which requirements have no implementation · which acceptance tests are missing · which specs does a change affect.
> **DEC-054 inventory (2026-09-28):** 315 unique requirement rows match the 315 active requirement headings in `08`; the 17 new rows remain unplanned with task/test pending. `TODO.md` is intentionally unchanged.

---

## 1. Matrix schema

One row per `REQ-*`. Status only from evidence.

| REQ | Module (doc) | Code paths | Design (DEC/DM/CTR) | Flow/Edge | Task (`TODO.md`) | Test | Status |
|---|---|---|---|---|---|---|---|
| `REQ-<DOMAIN>-<NNN>` | `<10..34 doc>` | `<path>` | `<DEC / DM / CTR IDs>` | `<FLOW / EDGE IDs>` | `<TASK-<DOMAIN>-<NNN>>` | `<TEST-<DOMAIN>-<NNN>>` or `pending` | `<status>` |

**Status ladder:** `unplanned → planned → in-progress → implemented → verified` (+ `blocked`).
`verified` requires an acceptance record (`ARCH/42-EVIDENCE-MAP.md`); for risky capability classes, unit tests alone never reach `verified`.

## 2. Rules

- One row per requirement; a requirement without a row is unplanned work — surface it, don't hide it.
- `Design` cells cite IDs (`DEC-*`, `DM-*`, `CTR-*`); `Flow/Edge` cite `FLOW-*`/`EDGE-*` where applicable.
- `Task` uses `TASK-<DOMAIN>-<NNN>` IDs defined in `TODO.md` (task text lives there, not here).
- `Test` names the automated test and/or the acceptance record path; `pending` is visible debt, not a failure.
- Never restate a requirement's statement here — link to `ARCH/08-REQUIREMENTS.md`.

## 3. Seed rows (P7 seeds; module passes append rows as domains are seeded — `KERNEL`/`WORK` added 2026-09-26)

| REQ | Module (doc) | Code paths | Design (DEC/DM/CTR) | Flow/Edge | Task (`TODO.md`) | Test | Status |
|---|---|---|---|---|---|---|---|
| `REQ-PROD-001` | `12-TRUST`, `13-CAPABILITY` | src-tauri/src/fs_cmds.rs, src-tauri/src/terminal_cmds.rs, src-tauri/src/control.rs, crates/agentcowork-core/src/tools.rs | `DEC-002`, `INV-01/03` | — | pending | pending | implemented |
| `REQ-PROD-002` | `12-TRUST`, `18-MODEL-ROUTING` | crates/agentcowork-core/src/connectors/mod.rs, crates/agentcowork-core/src/connector_hub.rs, crates/agentcowork-vault/src/oauth.rs, scripts/check-secret-corpus.mjs, scripts/e2e/security-gate.mjs | `INV-02` | — | pending | pending | implemented |
| `REQ-KERNEL-001` | `10-KERNEL` | none yet — no kernel crate (code-state baseline) | `INV-14` | — | pending | pending | unplanned |
| `REQ-KERNEL-002` | `10-KERNEL` | pending | `INV-06` | — | pending | pending | unplanned |
| `REQ-KERNEL-003` | `10-KERNEL` | pending | `INV-11` | — | pending | pending | unplanned |
| `REQ-KERNEL-004` | `10-KERNEL` | pending | `INV-02` | — | pending | pending | unplanned |
| `REQ-KERNEL-005` | `10-KERNEL` | pending | — | — | pending | pending | unplanned |
| `REQ-KERNEL-006` | `10-KERNEL` | pending | `INV-06` | — | pending | pending | unplanned |
| `REQ-KERNEL-007` | `10-KERNEL` | pending | `CTR-003/004/026` | — | pending | pending | unplanned |
| `REQ-WORK-001` | `11-WORK` | pending | `DEC-003`, `INV-06`, `CTR-003/026` | — | pending | pending | unplanned |
| `REQ-WORK-002` | `11-WORK` | pending | `DEC-027`, `INV-23`, `CTR-004`, `DM-007` | — | pending | pending | unplanned |
| `REQ-WORK-003` | `11-WORK` | pending | `INV-16`, `DM-006` | — | pending | pending | unplanned |
| `REQ-WORK-004` | `11-WORK` | pending | `DEC-031`, `CTR-026` | — | pending | pending | unplanned |
| `REQ-WORK-005` | `11-WORK` | pending | `INV-22`, `DEC-031`, `DM-001` | — | pending | pending | unplanned |
| `REQ-WORK-006` | `11-WORK` | pending | `DM-001/002` | — | pending | pending | unplanned |
| `REQ-WORK-007` | `11-WORK` | pending | `INV-16`, `DM-006` | — | pending | pending | unplanned |
| `REQ-WORK-008` | `11-WORK` | pending | `DM-005/007` | — | pending | pending | unplanned |
| `REQ-TRUST-001` | `12-TRUST` | crates/agentcowork-guard/src/netfloor.rs, crates/agentcowork-core/src/tools.rs, crates/agentcowork-core/src/messaging.rs, crates/agentcowork-core/src/models/probe.rs, crates/agentcowork-vault/src/oauth.rs | `INV-05` | — | pending | pending | implemented |
| `REQ-TRUST-002` | `12-TRUST` | crates/agentcowork-acp/src/permission_bridge.rs, src-tauri/src/acp_cmds.rs | `DEC-021`, `DM-010` | — | pending | pending | implemented |
| `REQ-TRUST-003` | `12-TRUST` | src-tauri/src/fs_cmds.rs, src-tauri/src/terminal_cmds.rs, crates/agentcowork-blueprint/src/confined_fs.rs, crates/agentcowork-guard/src/ticket.rs | `INV-04`, `DEC-028` | — | pending | pending | implemented |
| `REQ-TRUST-004` | `12-TRUST` | crates/agentcowork-core/src/connectors/mod.rs, crates/agentcowork-vault/src/session.rs, crates/agentcowork-vault/src/oauth.rs | `INV-02`, `CTR-013` | — | pending | pending | implemented |
| `REQ-TRUST-005` | `12-TRUST` | src-tauri/src/fs_cmds.rs, src-tauri/src/terminal_cmds.rs, crates/agentcowork-guard/src/ticket.rs | `INV-03`, `DM-009` | — | pending | pending | implemented |
| `REQ-TRUST-006` | `12-TRUST` | pending | `DEC-028` | — | pending | pending | unplanned |
| `REQ-TRUST-007` | `12-TRUST` | pending | `INV-24` | — | pending | pending | unplanned |
| `REQ-TRUST-008` | `12-TRUST` | pending | `DEC-009`, `INV-10/11` | — | pending | pending | unplanned |
| `REQ-TRUST-009` | `12-TRUST` | crates/agentcowork-guard/src/ratelimit.rs, src-tauri/src/lib.rs, crates/agentcowork-core/src/tools.rs | `INV-05` | — | pending | pending | implemented |
| `REQ-TRUST-010` | `12-TRUST` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CAP-001` | `13-CAPABILITY` | pending | `INV-13`, `DEC-005`, `DM-011` | — | pending | pending | unplanned |
| `REQ-CAP-002` | `13-CAPABILITY` | pending | `DM-012`, `13` §4 | — | pending | pending | unplanned |
| `REQ-CAP-003` | `13-CAPABILITY` | pending | `DEC-004` | — | pending | pending | unplanned |
| `REQ-CAP-004` | `13-CAPABILITY` | pending | `DM-011`, `INV-19` | — | pending | pending | unplanned |
| `REQ-CAP-005` | `13-CAPABILITY` | pending | `INV-07` | — | pending | pending | unplanned |
| `REQ-CAP-006` | `13-CAPABILITY` | pending | `DEC-005/024`, `INV-13` | — | pending | pending | unplanned |
| `REQ-CAP-007` | `13-CAPABILITY` | pending | `DM-012` | — | pending | pending | unplanned |
| `REQ-CAP-008` | `13-CAPABILITY` | pending | `DM-011` | — | pending | pending | unplanned |
| `REQ-CAP-009` | `13-CAPABILITY` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CAP-010` | `13-CAPABILITY` | pending | `INV-03/19` | — | pending | pending | unplanned |
| `REQ-PROV-001` | `14-PROVIDERS` | pending | `INV-15`, `DEC-004` | — | pending | pending | unplanned |
| `REQ-PROV-002` | `14-PROVIDERS` | crates/agentcowork-core/src/tools.rs, crates/agentcowork-guard/src/ticket.rs, crates/agentcowork-audit/src/receipt.rs | `CTR-010`, `INV-03` | — | pending | pending | implemented |
| `REQ-PROV-003` | `14-PROVIDERS` | pending | `DEC-025` | — | pending | pending | unplanned |
| `REQ-PROV-004` | `14-PROVIDERS` | crates/agentcowork-mcp/src/remote.rs, crates/agentcowork-mcp/src/store.rs, src-tauri/src/mcp_cmds.rs | `DEC-030` | — | pending | pending | implemented |
| `REQ-PROV-005` | `14-PROVIDERS` | crates/agentcowork-mcp/src/server.rs, crates/agentcowork-mcp/src/remote.rs | `DEC-030` | — | pending | pending | implemented |
| `REQ-PROV-006` | `14-PROVIDERS` | pending | `DM-012`, `DM-013`, `13` §4 | — | pending | pending | unplanned |
| `REQ-PROV-007` | `14-PROVIDERS` | pending | `DM-013`, `CTR-013`, `DEC-047` | — | pending | pending | unplanned |
| `REQ-PROV-008` | `14-PROVIDERS` | crates/agentcowork-guard/src/netfloor.rs, crates/agentcowork-core/src/models/, crates/agentcowork-vault/src/oauth.rs | `INV-02/05`, `CTR-013` | — | pending | pending | implemented |
| `REQ-PROV-009` | `14-PROVIDERS` | pending | `DEC-035` | — | pending | pending | unplanned |
| `REQ-PROV-010` | `14-PROVIDERS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CTX-001` | `16-CONTEXT` | pending | `INV-08` | — | pending | pending | unplanned |
| `REQ-CTX-002` | `16-CONTEXT` | pending | `INV-22` | — | pending | pending | unplanned |
| `REQ-CTX-003` | `16-CONTEXT` | pending | `DEC-007` | — | pending | pending | unplanned |
| `REQ-CTX-004` | `16-CONTEXT` | pending | `INV-08` | — | pending | pending | unplanned |
| `REQ-CTX-005` | `16-CONTEXT` | pending | `DEC-027/045` | — | pending | pending | unplanned |
| `REQ-CTX-006` | `16-CONTEXT` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CTX-007` | `16-CONTEXT` | pending | `INV-23` | — | pending | pending | unplanned |
| `REQ-CTX-008` | `16-CONTEXT` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CTX-009` | `16-CONTEXT` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CTX-010` | `16-CONTEXT` | pending | `DEC-009`, `INV-11` | — | pending | pending | unplanned |
| `REQ-AGENT-001` | `15-AGENT-PLANE` | crates/agentcowork-types/src/delegation.rs, crates/agentcowork-blueprint/src/subagent.rs | `DEC-029`, `DEC-031`, `DEC-036`, `DM-016` | — | pending | pending | unplanned |
| `REQ-AGENT-002` | `15-AGENT-PLANE` | pending | `DEC-029`, `DEC-036`, `DM-016` | — | pending | pending | unplanned |
| `REQ-AGENT-003` | `15-AGENT-PLANE` | pending | `DEC-025`, `DEC-029` | — | pending | pending | unplanned |
| `REQ-AGENT-004` | `15-AGENT-PLANE` | crates/agentcowork-core/src/spool.rs | `DEC-022` | — | pending | pending | unplanned |
| `REQ-MEM-001` | `17-MEMORY` | pending | `DEC-018/019`, `DM-018` | — | pending | pending | unplanned |
| `REQ-MEM-002` | `17-MEMORY` | pending | `INV-09` | — | pending | pending | unplanned |
| `REQ-MEM-003` | `17-MEMORY` | pending | `DEC-019` | — | pending | pending | unplanned |
| `REQ-MEM-004` | `17-MEMORY` | pending | `INV-08` | — | pending | pending | unplanned |
| `REQ-MEM-005` | `17-MEMORY` | pending | `DM-018` | — | pending | pending | unplanned |
| `REQ-MEM-006` | `17-MEMORY` | pending | `INV-10`, `DEC-009/038` | — | pending | pending | unplanned |
| `REQ-MEM-007` | `17-MEMORY` | pending | `DEC-018` | — | pending | pending | unplanned |
| `REQ-MEM-008` | `17-MEMORY` | pending | `DEC-018` | — | pending | pending | unplanned |
| `REQ-MEM-009` | `17-MEMORY` | pending | `DEC-018`, `DM-018` | — | pending | pending | unplanned |
| `REQ-MEM-010` | `17-MEMORY` | pending | `INV-09`, `INV-24`, `DEC-039` | `EDGE-170` | pending | pending | unplanned |
| `REQ-MEM-011` | `17-MEMORY` | pending | `—` | — | pending | pending | unplanned |
| `REQ-MEM-012` | `17-MEMORY` | pending | `INV-22` | `EDGE-027` | pending | pending | unplanned |
| `REQ-MEM-013` | `17-MEMORY` | pending | `DEC-038`, `INV-10`, `DM-018` | — | pending | pending | unplanned |
| `REQ-MEM-014` | `17-MEMORY` | pending | `DEC-036/037`, `INV-11` | `EDGE-093/176` | pending | pending | unplanned |
| `REQ-MEM-015` | `17-MEMORY` | pending | `—` | — | pending | pending | unplanned |
| `REQ-MEM-016` | `17-MEMORY` | pending | `DEC-039`, `INV-09/24`, `DM-018` | `EDGE-170` | pending | pending | unplanned |
| `REQ-MEM-017` | `17-MEMORY` | pending | `—` | `EDGE-175` | pending | pending | unplanned |
| `REQ-MEM-018` | `17-MEMORY` | pending | `INV-06` | `EDGE-172/173` | pending | pending | unplanned |
| `REQ-MEM-019` | `17-MEMORY` | pending | `INV-22` | `EDGE-171` | pending | pending | unplanned |
| `REQ-MEM-020` | `17-MEMORY` | pending | `DEC-040`, `DM-024`, `INV-10` | `EDGE-178` | pending | pending | unplanned |
| `REQ-MEM-021` | `17-MEMORY` | pending | `DEC-027/041`, `DM-006` | — | pending | pending | unplanned |
| `REQ-MEM-022` | `17-MEMORY` | pending | `DEC-009/025/029/036/043` | `EDGE-157/158` | pending | pending | unplanned |
| `REQ-MEM-023` | `17-MEMORY` | pending | `DEC-042`, `INV-01/04/24` | — | pending | pending | unplanned |
| `REQ-MEM-024` | `17-MEMORY` | pending | `DEC-031/044` | — | pending | pending | unplanned |
| `REQ-MEM-025` | `17-MEMORY` | pending | `—` | `EDGE-112/177` | pending | pending | unplanned |
| `REQ-MEM-026` | `17-MEMORY` | pending | `DEC-032`, `DM-018` | `EDGE-179` | pending | pending | unplanned |
| `REQ-MEM-027` | `17-MEMORY` | pending | `—` | `EDGE-028` | pending | pending | unplanned |
| `REQ-MODEL-001` | `18-MODEL-ROUTING` | pending | `DEC-004`, `DM-025` | — | pending | pending | unplanned |
| `REQ-MODEL-002` | `18-MODEL-ROUTING` | pending | `DM-025` | — | pending | pending | unplanned |
| `REQ-MODEL-003` | `18-MODEL-ROUTING` | pending | `DEC-034` | — | pending | pending | unplanned |
| `REQ-MODEL-004` | `18-MODEL-ROUTING` | pending | `CTR-014` | — | pending | pending | unplanned |
| `REQ-MODEL-005` | `18-MODEL-ROUTING` | pending | `INV-02`, `CTR-013` | — | pending | pending | unplanned |
| `REQ-MODEL-006` | `18-MODEL-ROUTING` | pending | `DEC-034` | — | pending | pending | unplanned |
| `REQ-MODEL-007` | `18-MODEL-ROUTING` | pending | `DEC-034` | — | pending | pending | unplanned |
| `REQ-MODEL-008` | `18-MODEL-ROUTING` | pending | `DEC-034` | — | pending | pending | unplanned |
| `REQ-MODEL-009` | `18-MODEL-ROUTING` | pending | `DEC-034` | — | pending | pending | unplanned |
| `REQ-MODEL-010` | `18-MODEL-ROUTING` | pending | `INV-05` | — | pending | pending | unplanned |
| `REQ-MODEL-011` | `18-MODEL-ROUTING` | pending | `DEC-034` | — | pending | pending | unplanned |
| `REQ-MODEL-012` | `18-MODEL-ROUTING` | pending | `DEC-027` | — | pending | pending | unplanned |
| `REQ-RTENV-001` | `19-RUNTIME-ENVIRONMENTS` | pending | `INV-04`, `DEC-028` | — | pending | pending | unplanned |
| `REQ-RTENV-002` | `19-RUNTIME-ENVIRONMENTS` | pending | `DEC-028` | — | pending | pending | unplanned |
| `REQ-RTENV-003` | `19-RUNTIME-ENVIRONMENTS` | pending | `CTR-015`, `DM-015` | — | pending | pending | unplanned |
| `REQ-RTENV-004` | `19-RUNTIME-ENVIRONMENTS` | pending | `DEC-031` | — | pending | pending | unplanned |
| `REQ-RTENV-005` | `19-RUNTIME-ENVIRONMENTS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-RTENV-006` | `19-RUNTIME-ENVIRONMENTS` | pending | `INV-07` | — | pending | pending | unplanned |
| `REQ-RTENV-007` | `19-RUNTIME-ENVIRONMENTS` | pending | `DEC-024`, `INV-02` | — | pending | pending | unplanned |
| `REQ-RTENV-008` | `19-RUNTIME-ENVIRONMENTS` | pending | `INV-20`, `INV-24` | — | pending | pending | unplanned |
| `REQ-RTENV-009` | `19-RUNTIME-ENVIRONMENTS` | pending | `INV-16` | — | pending | pending | unplanned |
| `REQ-RTENV-010` | `19-RUNTIME-ENVIRONMENTS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-RTENV-011` | `19-RUNTIME-ENVIRONMENTS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-WF-001` | `20-WORKFLOW` | pending | `INV-16`, `DEC-033` | — | pending | pending | unplanned |
| `REQ-WF-002` | `20-WORKFLOW` | pending | `DM-021`, `DEC-008` | — | pending | pending | unplanned |
| `REQ-WF-003` | `20-WORKFLOW` | pending | `DEC-033` | — | pending | pending | unplanned |
| `REQ-WF-004` | `20-WORKFLOW` | pending | `CTR-016`, `INV-06` | — | pending | pending | unplanned |
| `REQ-WF-005` | `20-WORKFLOW` | pending | `DEC-033`, `DEC-022` | — | pending | pending | unplanned |
| `REQ-WF-006` | `20-WORKFLOW` | pending | `DEC-033` | — | pending | pending | unplanned |
| `REQ-WF-007` | `20-WORKFLOW` | pending | `DEC-033` | — | pending | pending | unplanned |
| `REQ-WF-008` | `20-WORKFLOW` | pending | `DEC-021`, `INV-17`, `CTR-012` | — | pending | pending | unplanned |
| `REQ-WF-009` | `20-WORKFLOW` | pending | `DEC-031`, `DEC-033` | — | pending | pending | unplanned |
| `REQ-WF-010` | `20-WORKFLOW` | pending | `DEC-008`, `CTR-001/CTR-009` | — | pending | pending | unplanned |
| `REQ-WF-011` | `20-WORKFLOW` | pending | `INV-07`, `INV-23` | — | pending | pending | unplanned |
| `REQ-WORLD-001` | `21-WORLD-MODEL` | pending | `DEC-011` | — | pending | pending | unplanned |
| `REQ-WORLD-002` | `21-WORLD-MODEL` | pending | `—` | — | pending | pending | unplanned |
| `REQ-WORLD-003` | `21-WORLD-MODEL` | crates/agentcowork-storage/src/identity.rs, crates/agentcowork-storage/src/walk.rs | `DM-026` | — | pending | pending | implemented |
| `REQ-WORLD-004` | `21-WORLD-MODEL` | crates/agentcowork-storage/src/usn.rs, crates/agentcowork-storage/src/usn_winapi.rs | `INV-20` | — | pending | pending | implemented |
| `REQ-WORLD-005` | `21-WORLD-MODEL` | crates/agentcowork-storage/src/usn.rs, crates/agentcowork-storage/src/usn_reader.rs | `INV-20` | — | pending | pending | implemented |
| `REQ-WORLD-006` | `21-WORLD-MODEL` | pending | `CTR-017` | — | pending | pending | unplanned |
| `REQ-WORLD-007` | `21-WORLD-MODEL` | pending | `INV-20` | — | pending | pending | unplanned |
| `REQ-WORLD-008` | `21-WORLD-MODEL` | pending | `INV-20` | — | pending | pending | unplanned |
| `REQ-WORLD-009` | `21-WORLD-MODEL` | pending | `INV-20`, `DEC-011` | — | pending | pending | unplanned |
| `REQ-WORLD-010` | `21-WORLD-MODEL` | crates/agentcowork-desktop/src/capture.rs, crates/agentcowork-desktop/src/readiness.rs, src-tauri/src/desktop_cmds.rs | `INV-24` | — | pending | pending | implemented |
| `REQ-WORLD-011` | `21-WORLD-MODEL` | pending | `CTR-017`, `INV-11` | — | pending | pending | unplanned |
| `REQ-OFFICE-001` | `22-OFFICE` | pending | `CTR-009`, `DEC-013` | — | pending | pending | unplanned |
| `REQ-OFFICE-002` | `22-OFFICE` | pending | `DEC-013` | — | pending | pending | unplanned |
| `REQ-OFFICE-003` | `22-OFFICE` | crates/agentcowork-office/src/resident.rs, src-tauri/src/office_cmds.rs, src-tauri/src/xlsx_cmds.rs | `DEC-013` | — | pending | pending | implemented |
| `REQ-OFFICE-004` | `22-OFFICE` | crates/agentcowork-office/src/atomic.rs, crates/agentcowork-office/src/resident.rs | `DEC-013` | — | pending | pending | implemented |
| `REQ-OFFICE-005` | `22-OFFICE` | pending | `DEC-023` | — | pending | pending | unplanned |
| `REQ-OFFICE-006` | `22-OFFICE` | crates/agentcowork-office/src/pdf/redact.rs, src-tauri/src/office_cmds.rs | `CTR-009` | — | pending | pending | implemented |
| `REQ-OFFICE-007` | `22-OFFICE` | crates/agentcowork-office/src/pdf/redact.rs, crates/agentcowork-office/src/conformance.rs | `DEC-015`, `INV-13` | — | pending | pending | implemented |
| `REQ-OFFICE-008` | `22-OFFICE` | pending | `INV-19`, `DEC-022` | — | pending | pending | unplanned |
| `REQ-OFFICE-009` | `22-OFFICE` | pending | `DEC-023` | — | pending | pending | unplanned |
| `REQ-OFFICE-010` | `22-OFFICE` | pending | `DEC-023` | — | pending | pending | unplanned |
| `REQ-OFFICE-011` | `22-OFFICE` | pending | `—` | — | pending | pending | unplanned |
| `REQ-BROWSER-001` | `23-BROWSER` | pending | `DEC-012` | — | pending | pending | unplanned |
| `REQ-BROWSER-002` | `23-BROWSER` | pending | `DEC-012` | — | pending | pending | unplanned |
| `REQ-BROWSER-003` | `23-BROWSER` | pending | `—` | — | pending | pending | unplanned |
| `REQ-BROWSER-004` | `23-BROWSER` | pending | `DM-026` | — | pending | pending | unplanned |
| `REQ-BROWSER-005` | `23-BROWSER` | pending | `—` | — | pending | pending | unplanned |
| `REQ-BROWSER-006` | `23-BROWSER` | pending | `—` | — | pending | pending | unplanned |
| `REQ-BROWSER-007` | `23-BROWSER` | pending | `—` | — | pending | pending | unplanned |
| `REQ-BROWSER-008` | `23-BROWSER` | pending | `DEC-011` | — | pending | pending | unplanned |
| `REQ-BROWSER-009` | `23-BROWSER` | pending | `INV-02` | — | pending | pending | unplanned |
| `REQ-BROWSER-010` | `23-BROWSER` | pending | `INV-05`, `INV-20` | — | pending | pending | unplanned |
| `REQ-BROWSER-011` | `23-BROWSER` | pending | `CTR-018` | — | pending | pending | unplanned |
| `REQ-BROWSER-012` | `23-BROWSER` | pending | `DEC-016`, `INV-21` | — | pending | pending | unplanned |
| `REQ-CUA-001` | `24-COMPUTER-USE` | pending | `DEC-011` | — | pending | pending | unplanned |
| `REQ-CUA-002` | `24-COMPUTER-USE` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CUA-003` | `24-COMPUTER-USE` | crates/agentcowork-desktop/src/uia.rs, crates/agentcowork-desktop/src/platform/win.rs | `DM-026` | — | pending | pending | implemented |
| `REQ-CUA-004` | `24-COMPUTER-USE` | crates/agentcowork-desktop/src/uia.rs | `—` | — | pending | pending | implemented |
| `REQ-CUA-005` | `24-COMPUTER-USE` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CUA-006` | `24-COMPUTER-USE` | crates/agentcowork-desktop/src/uia.rs, crates/agentcowork-desktop/src/ladder.rs | `—` | — | pending | pending | implemented |
| `REQ-CUA-007` | `24-COMPUTER-USE` | pending | `DEC-011` | — | pending | pending | unplanned |
| `REQ-CUA-008` | `24-COMPUTER-USE` | crates/agentcowork-desktop/src/capture.rs, crates/agentcowork-desktop/src/platform/win.rs | `DEC-015`, `INV-22` | — | pending | pending | implemented |
| `REQ-CUA-009` | `24-COMPUTER-USE` | pending | `DEC-021` | — | pending | pending | unplanned |
| `REQ-CUA-010` | `24-COMPUTER-USE` | pending | `DEC-016`, `INV-21` | — | pending | pending | unplanned |
| `REQ-CUA-011` | `24-COMPUTER-USE` | pending | `DEC-021`, `INV-04` | — | pending | pending | unplanned |
| `REQ-CUA-012` | `24-COMPUTER-USE` | pending | `DEC-022` | — | pending | pending | unplanned |
| `REQ-FILES-001` | `25-FILES` | crates/agentcowork-storage/src/identity.rs, crates/agentcowork-storage/src/walk.rs | `DM-026`, `CTR-024`, `INV-20` | — | pending | pending | implemented |
| `REQ-FILES-002` | `25-FILES` | crates/agentcowork-storage/src/identity.rs, crates/agentcowork-storage/src/dedup.rs, crates/agentcowork-storage/src/cleanup.rs | `DM-026` | — | pending | pending | implemented |
| `REQ-FILES-003` | `25-FILES` | pending | `CTR-024`, `INV-20` | — | pending | pending | unplanned |
| `REQ-FILES-004` | `25-FILES` | crates/agentcowork-storage/src/usn.rs, crates/agentcowork-storage/src/usn_reader.rs | `CTR-024`, `INV-20` | — | pending | pending | implemented |
| `REQ-FILES-005` | `25-FILES` | crates/agentcowork-storage/src/usn.rs | `INV-20` | — | pending | pending | implemented |
| `REQ-FILES-006` | `25-FILES` | pending | `CTR-024` | — | pending | pending | unplanned |
| `REQ-FILES-007` | `25-FILES` | pending | `DEC-029`, `CTR-024` | — | pending | pending | unplanned |
| `REQ-FILES-008` | `25-FILES` | pending | `DEC-029`, `INV-24` | — | pending | pending | unplanned |
| `REQ-FILES-009` | `25-FILES` | pending | `—` | — | pending | pending | unplanned |
| `REQ-FILES-010` | `25-FILES` | pending | `INV-10` | — | pending | pending | unplanned |
| `REQ-FILES-011` | `25-FILES` | pending | `DM-024`, `DEC-029`, `DEC-040` | — | pending | pending | unplanned |
| `REQ-FILES-012` | `25-FILES` | pending | `INV-20`, `INV-24` | — | pending | pending | unplanned |
| `REQ-CODE-001` | `26-CODE` | pending | `CTR-025` | — | pending | pending | unplanned |
| `REQ-CODE-002` | `26-CODE` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CODE-003` | `26-CODE` | pending | `DEC-027`, `CTR-025` | — | pending | pending | unplanned |
| `REQ-CODE-004` | `26-CODE` | pending | `CTR-025` | — | pending | pending | unplanned |
| `REQ-CODE-005` | `26-CODE` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CODE-006` | `26-CODE` | pending | `DEC-029`, `CTR-025` | — | pending | pending | unplanned |
| `REQ-CODE-007` | `26-CODE` | pending | `INV-03` | — | pending | pending | unplanned |
| `REQ-CODE-008` | `26-CODE` | pending | `INV-01/03`, `CTR-025` | — | pending | pending | unplanned |
| `REQ-CODE-009` | `26-CODE` | pending | `INV-22` | — | pending | pending | unplanned |
| `REQ-CODE-010` | `26-CODE` | pending | `CTR-025` | — | pending | pending | unplanned |
| `REQ-CODE-011` | `26-CODE` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CODE-012` | `26-CODE` | pending | `CTR-025` | — | pending | pending | unplanned |
| `REQ-SEARCH-001` | `27-SEARCH` | pending | `CTR-006`, `CTR-009` | — | pending | pending | unplanned |
| `REQ-SEARCH-002` | `27-SEARCH` | pending | `DEC-015`, `INV-13` | — | pending | pending | unplanned |
| `REQ-SEARCH-003` | `27-SEARCH` | pending | `INV-10` | — | pending | pending | unplanned |
| `REQ-SEARCH-004` | `27-SEARCH` | pending | `DEC-009`, `INV-11` | — | pending | pending | unplanned |
| `REQ-SEARCH-005` | `27-SEARCH` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SEARCH-006` | `27-SEARCH` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SEARCH-007` | `27-SEARCH` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SEARCH-008` | `27-SEARCH` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SEARCH-009` | `27-SEARCH` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SEARCH-010` | `27-SEARCH` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SEARCH-011` | `27-SEARCH` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SEARCH-012` | `27-SEARCH` | pending | `—` | — | pending | pending | unplanned |
| `REQ-COMMS-001` | `28-COMMS` | pending | `CTR-009` | — | pending | pending | unplanned |
| `REQ-COMMS-002` | `28-COMMS` | pending | `DEC-005`, `CTR-010` | — | pending | pending | unplanned |
| `REQ-COMMS-003` | `28-COMMS` | crates/agentcowork-core/src/connectors/mod.rs, crates/agentcowork-core/src/connectors/{graph,gmail,calendar,workspace}.rs | `INV-02`, `CTR-013` | — | pending | pending | implemented |
| `REQ-COMMS-004` | `28-COMMS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-COMMS-005` | `28-COMMS` | pending | `DEC-021`, `INV-07` | — | pending | pending | unplanned |
| `REQ-COMMS-006` | `28-COMMS` | pending | `DM-011` | — | pending | pending | unplanned |
| `REQ-COMMS-007` | `28-COMMS` | pending | `CTR-018`, `DEC-037` | — | pending | pending | unplanned |
| `REQ-COMMS-008` | `28-COMMS` | pending | `CTR-019` | — | pending | pending | unplanned |
| `REQ-COMMS-009` | `28-COMMS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-COMMS-010` | `28-COMMS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-COMMS-011` | `28-COMMS` | pending | `DEC-037` | — | pending | pending | unplanned |
| `REQ-COMMS-012` | `28-COMMS` | pending | `DEC-037` | — | pending | pending | unplanned |
| `REQ-COMMS-013` | `28-COMMS` | crates/agentcowork-core/src/tools.rs, crates/agentcowork-guard/src/netfloor.rs | `INV-02/05`, `DEC-016/037` | — | pending | pending | implemented |
| `REQ-ART-001` | `29-ARTIFACTS` | pending | `DM-019`, `INV-18` | — | pending | pending | unplanned |
| `REQ-ART-002` | `29-ARTIFACTS` | pending | `DM-019`, `INV-18` | — | pending | pending | unplanned |
| `REQ-ART-003` | `29-ARTIFACTS` | crates/agentcowork-core/src/tools.rs, crates/agentcowork-audit/src/receipt.rs | `DEC-022`, `INV-07`, `CTR-018` | — | pending | pending | implemented |
| `REQ-ART-004` | `29-ARTIFACTS` | pending | `CTR-018`, `INV-07` | — | pending | pending | unplanned |
| `REQ-ART-005` | `29-ARTIFACTS` | pending | `INV-23/24` | — | pending | pending | unplanned |
| `REQ-ART-006` | `29-ARTIFACTS` | pending | `DEC-014`, `DM-023` | — | pending | pending | unplanned |
| `REQ-ART-007` | `29-ARTIFACTS` | pending | `INV-11` | `EDGE-105` | pending | pending | unplanned |
| `REQ-ART-008` | `29-ARTIFACTS` | pending | `DEC-032` | `EDGE-100` | pending | pending | unplanned |
| `REQ-ART-009` | `29-ARTIFACTS` | pending | `DEC-032`, `INV-24` | — | pending | pending | unplanned |
| `REQ-ART-010` | `29-ARTIFACTS` | pending | `DEC-015` | — | pending | pending | unplanned |
| `REQ-ART-011` | `29-ARTIFACTS` | pending | `INV-18` | `EDGE-101` | pending | pending | unplanned |
| `REQ-ART-012` | `29-ARTIFACTS` | crates/agentcowork-core/src/tools.rs | `INV-07` | `EDGE-104` | pending | pending | implemented |
| `REQ-EVENTS-001` | `30-EVENTS` | pending | `INV-23`, `DEC-027` | — | pending | pending | unplanned |
| `REQ-EVENTS-002` | `30-EVENTS` | pending | `DM-008` | — | pending | pending | unplanned |
| `REQ-EVENTS-003` | `30-EVENTS` | pending | `INV-02`, `DM-008` | — | pending | pending | unplanned |
| `REQ-EVENTS-004` | `30-EVENTS` | pending | `DM-008` | — | pending | pending | unplanned |
| `REQ-EVENTS-005` | `30-EVENTS` | pending | `—` | `EDGE-076` | pending | pending | unplanned |
| `REQ-EVENTS-006` | `30-EVENTS` | pending | `INV-11`, `DEC-009` | — | pending | pending | unplanned |
| `REQ-EVENTS-007` | `30-EVENTS` | pending | `DEC-027/033` | — | pending | pending | unplanned |
| `REQ-EVENTS-008` | `30-EVENTS` | pending | `INV-23` | `EDGE-106` | pending | pending | unplanned |
| `REQ-EVENTS-009` | `30-EVENTS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-EVENTS-010` | `30-EVENTS` | pending | `DEC-027` | — | pending | pending | unplanned |
| `REQ-EVENTS-011` | `30-EVENTS` | pending | `DEC-009`, `INV-11` | `EDGE-079` | pending | pending | unplanned |
| `REQ-EVENTS-012` | `30-EVENTS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SKILL-001` | `31-SKILLS-PLUGINS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SKILL-002` | `31-SKILLS-PLUGINS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SKILL-003` | `31-SKILLS-PLUGINS` | pending | `DM-027` | — | pending | pending | unplanned |
| `REQ-SKILL-004` | `31-SKILLS-PLUGINS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SKILL-005` | `31-SKILLS-PLUGINS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SKILL-006` | `31-SKILLS-PLUGINS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SKILL-007` | `31-SKILLS-PLUGINS` | pending | `DEC-028` | — | pending | pending | unplanned |
| `REQ-SKILL-008` | `31-SKILLS-PLUGINS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SKILL-009` | `31-SKILLS-PLUGINS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-SKILL-010` | `31-SKILLS-PLUGINS` | crates/agentcowork-blueprint/src/skill_store.rs, crates/agentcowork-blueprint/src/confined_fs.rs, src-tauri/src/skills_cmds.rs | `—` | — | pending | pending | implemented |
| `REQ-SKILL-011` | `31-SKILLS-PLUGINS` | pending | `—` | `EDGE-094` | pending | pending | unplanned |
| `REQ-SKILL-012` | `31-SKILLS-PLUGINS` | pending | `—` | `EDGE-093` | pending | pending | unplanned |
| `REQ-SKILL-013` | `31-SKILLS-PLUGINS` | pending | `—` | `EDGE-096` | pending | pending | unplanned |
| `REQ-CHAN-001` | `32-CHANNELS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CHAN-002` | `32-CHANNELS` | pending | `INV-15` | — | pending | pending | unplanned |
| `REQ-CHAN-003` | `32-CHANNELS` | pending | `DEC-009`, `CTR-022` | — | pending | pending | unplanned |
| `REQ-CHAN-004` | `32-CHANNELS` | pending | `INV-11` | `EDGE-035`, `EDGE-155` | pending | pending | unplanned |
| `REQ-CHAN-005` | `32-CHANNELS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CHAN-006` | `32-CHANNELS` | pending | `DEC-021` | `EDGE-073` | pending | pending | unplanned |
| `REQ-CHAN-007` | `32-CHANNELS` | pending | `DEC-010` | — | pending | pending | unplanned |
| `REQ-CHAN-008` | `32-CHANNELS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CHAN-009` | `32-CHANNELS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CHAN-010` | `32-CHANNELS` | pending | `—` | — | pending | pending | unplanned |
| `REQ-CHAN-011` | `32-CHANNELS` | pending | `—` | `EDGE-075` | pending | pending | unplanned |
| `REQ-CHAN-012` | `32-CHANNELS` | pending | `—` | `EDGE-079` | pending | pending | unplanned |
| `REQ-CHAN-013` | `32-CHANNELS` | pending | `INV-16` | `EDGE-077` | pending | pending | unplanned |
| `REQ-VERIFY-001` | `34-EFFECT-VERIFICATION` | pending | `DEC-022`, `INV-19`, `DM-011` | — | pending | pending | unplanned |
| `REQ-VERIFY-002` | `34-EFFECT-VERIFICATION` | pending | `—` | `EDGE-162` | pending | pending | unplanned |
| `REQ-VERIFY-003` | `34-EFFECT-VERIFICATION` | pending | `DEC-046` | — | pending | pending | unplanned |
| `REQ-VERIFY-004` | `34-EFFECT-VERIFICATION` | pending | `DEC-022` | — | pending | pending | unplanned |
| `REQ-VERIFY-005` | `34-EFFECT-VERIFICATION` | pending | `CTR-018` | — | pending | pending | unplanned |
| `REQ-VERIFY-006` | `34-EFFECT-VERIFICATION` | pending | `—` | — | pending | pending | unplanned |
| `REQ-VERIFY-007` | `34-EFFECT-VERIFICATION` | pending | `—` | `EDGE-055`, `EDGE-160` | pending | pending | unplanned |
| `REQ-VERIFY-008` | `34-EFFECT-VERIFICATION` | pending | `DEC-015` | — | pending | pending | unplanned |
| `REQ-VERIFY-009` | `34-EFFECT-VERIFICATION` | pending | `—` | `EDGE-162` | pending | pending | unplanned |
| `REQ-VERIFY-010` | `34-EFFECT-VERIFICATION` | pending | `—` | `EDGE-163` | pending | pending | unplanned |
| `REQ-VERIFY-011` | `34-EFFECT-VERIFICATION` | pending | `—` | — | pending | pending | unplanned |
| `REQ-VERIFY-012` | `34-EFFECT-VERIFICATION` | pending | `INV-23` | — | pending | pending | unplanned |
| `REQ-VERIFY-013` | `34-EFFECT-VERIFICATION` | pending | `INV-19` | `EDGE-160` | pending | pending | unplanned |
| `REQ-PROD-003` | `12-TRUST` | pending | `INV-04`, `DEC-028` | — | pending | pending | unplanned |
| `REQ-PROD-004` | `15-AGENT-PLANE`, `12-TRUST` | pending | `INV-12`, `DEC-010` | — | pending | pending | unplanned |
| `REQ-PROD-005` | `AGENTCOWORK-UI`, `22-OFFICE` | pending | `INV-13`, `DEC-015` | — | pending | pending | unplanned |
| `REQ-PROD-006` | `42-EVIDENCE-MAP` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-001` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-002` | `AGENTCOWORK-UI` | pending | `DEC-015`, `INV-13` | — | pending | pending | unplanned |
| `REQ-UI-003` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-004` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-005` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-006` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-007` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-008` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-009` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-010` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-011` | `AGENTCOWORK-UI` | pending | `DM-014` | — | pending | pending | unplanned |
| `REQ-UI-012` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-013` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |
| `REQ-UI-014` | `AGENTCOWORK-UI` | pending | `—` | — | pending | pending | unplanned |

## 3.1 DEC-054 amendment rows (delivery task IDs pending; `TODO.md` intentionally unchanged)

| REQ | Module (doc) | Code paths | Design (DEC/DM/CTR) | Flow/Edge | Task (`TODO.md`) | Test | Status |
|---|---|---|---|---|---|---|---|
| `REQ-MISSION-001` | `35-MISSION` | pending | `DEC-054`, `INV-25/26`, `DM-028`, `CTR-027` | pending | pending | pending | unplanned |
| `REQ-MISSION-002` | `35-MISSION` | pending | `DEC-054`, `INV-27`, `DM-029/032` | pending | pending | pending | unplanned |
| `REQ-MISSION-003` | `35-MISSION`, `11-WORK` | pending | `DEC-054`, `INV-26`, `DM-038` | pending | pending | pending | unplanned |
| `REQ-MISSION-004` | `36-OUTCOME-AND-RECOVERY` | pending | `DEC-054`, `INV-28/30`, `CTR-028` | pending | pending | pending | unplanned |
| `REQ-MISSION-005` | `36-OUTCOME-AND-RECOVERY` | pending | `DEC-054`, `INV-29`, `CTR-029` | pending | pending | pending | unplanned |
| `REQ-MISSION-006` | `36-OUTCOME-AND-RECOVERY` | pending | `DEC-054`, `INV-31` | pending | pending | pending | unplanned |
| `REQ-MISSION-007` | `35-MISSION`, `12-TRUST` | pending | `DEC-054`, `CTR-012/027` | pending | pending | pending | unplanned |
| `REQ-ECO-001` | `46-ECOSYSTEM-ARCHITECTURE`, `32-CHANNELS` | pending | `DEC-054`, `INV-32`, `DM-014/039`, `CTR-030` | pending | pending | pending | unplanned |
| `REQ-ECO-002` | `46-ECOSYSTEM-ARCHITECTURE`, `31-SKILLS-PLUGINS` | pending | `DEC-054`, `INV-32/33`, `DM-037` | pending | pending | pending | unplanned |
| `REQ-ECO-003` | `32-CHANNELS`, `12-TRUST` | src-tauri/src/channel_b.rs (gap) | `DEC-054`, `INV-33` | pending | pending | pending | unplanned |
| `REQ-ECO-004` | `12-TRUST`, `34-EFFECT-VERIFICATION` | pending | `DEC-049/054`, `INV-34` | pending | pending | pending | unplanned |
| `REQ-ECO-005` | `15-AGENT-PLANE`, `35-MISSION` | pending | `DEC-054`, `CTR-021/027/030` | pending | pending | pending | unplanned |
| `REQ-ECO-006` | `13-CAPABILITY`, `23-BROWSER`, `24-COMPUTER-USE` | pending | `DEC-054`, `INV-34` | pending | pending | pending | unplanned |
| `REQ-LEARN-001` | `37-WORKFLOW-SKILL-LIFECYCLE` | pending | `DEC-054`, `INV-35` | pending | pending | pending | unplanned |
| `REQ-LEARN-002` | `20-WORKFLOW`, `37-WORKFLOW-SKILL-LIFECYCLE` | pending | `DEC-054`, `CTR-031` | pending | pending | pending | unplanned |
| `REQ-UXQ-001` | `38-EXPERIENCE-QUALITY` | pending | `DEC-054`, `CTR-027/028` | pending | pending | pending | unplanned |
| `REQ-UXQ-002` | `38-EXPERIENCE-QUALITY` | pending | `DEC-054` | pending | pending | pending | unplanned |

## 4. Maintenance

- Updated in the same change as the requirement or test it refers to; traceability edits ride with spec commits, not code commits.
- The matrix is checked in reviews of module passes: every accepted `REQ-*` must have a row; every row must have an owning module.
- Future tooling (a "spec compiler") can derive the missing-implementation and missing-test views mechanically from `ARCH/08-REQUIREMENTS.md` + this matrix; until then this file is maintained by hand in passes.

## 5. Related

- `ARCH/08-REQUIREMENTS.md` (requirement definitions) · `TODO.md` (tasks) · `ARCH/42-EVIDENCE-MAP.md` (acceptance evidence) · `ARCH/40-FLOWS.md` / `ARCH/41-EDGE-CASES.md` (flow and edge IDs)
- Process: `.agents/docs/spec-driven-development.md`
