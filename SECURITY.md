# Security policy (P70.F5)

AgentCowork runs AI agents with real authority over the user's machine, so the
security contract is the product. This file states the model, how to report a
problem, and which floors are *enforced in code* rather than promised.

## Reporting a vulnerability

Open a private security advisory on the repository
(`Security → Report a vulnerability`) rather than a public issue. Please
include: the version (`About` shows it, or `agentcowork-core --version`), the platform,
the exact steps, and — if a guard floor was bypassed — the audit event ids from
`~/.agentcowork/audit.ndjson`. Do not include live credentials; the audit ledger
never contains them by design, and neither should your report.

We aim to acknowledge within 3 working days. This is a v1-stage project: there
is no bug-bounty programme, and no version is under long-term support yet
(`docs/release/rollout-and-hotfix.md` §G3).

## The model (summary — authority: `ARCH/12-TRUST.md`, DEC-049/054)

**The sidecar proposes; the Rust core disposes for Core-mediated calls.** Every Core-mediated externally visible mutating effect requires
an authorization ticket minted in Rust (`agentcowork-guard`), and Core-held provider API keys
never leave the vault (`agentcowork-vault`). An external agent's native file, shell, network and extension effects follow its own policy and environment; they are not covered by Core tickets or the Core audit trail (DEC-054). A compromised renderer cannot mint a
ticket, approve one, or write the audit log — that split is tested, not assumed
(`P48.2`, the KERNEL GATE item).

| Layer | Enforced by | Where |
|---|---|---|
| Outbound network floor (SSRF) | `agentcowork-guard::netfloor` | Core-mediated outbound calls |
| Path floor (traversal) | `agentcowork-guard::pathfloor` | Core-mediated file effects |
| Secret-file protection | `agentcowork-guard::protected_paths` | `.env*`, `.ssh/`, `.aws/`, `id_rsa`, … |
| Single-use tickets + nonce | `agentcowork-guard` | Core-mediated effects |
| Three-party approval window | dedicated `guard.html` renderer | approval only |
| Credential custody | `agentcowork-vault` (SQLCipher) | keys are vault-only; the sidecar never holds one |
| Audit integrity | `agentcowork-audit` Merkle chain | Core-mediated mutating operations; native reports carry distinct provenance |
| Third-party MCP containment | ticket-gated, audited, net-floored; filesystem confinement **only on Linux with bwrap** | see below |

## What is *not* contained (stated, not implied)

- **Third-party MCP servers on Windows/macOS run `Ambient`** — with the inherited
  environment and no filesystem namespace. They are ticket-gated, audited and
  net-floored, but they are **not** confined. Settings → Diagnostics reports the
  posture the host actually delivers (`P70.D7`), and the reason is recorded as
  `P49.5`.
- **Agents are user-installed external programs.** An agent the user installs
  can run with the user's own authority. AgentCowork governs effects it requests
  through Core capabilities, not the agent's native internals or native effects.
- **Windows is not yet qualified on a real host** (`P70.E8`); the native
  sandbox backends for Windows/macOS are unbuilt.

## Release integrity

Release artifacts are produced only by CI from a tagged commit, with recorded
provenance (`P70.A6`); an unsigned Windows build fails the release job rather
than shipping (`P70.B1`, `docs/signing.md`). `SHA256SUMS`, a CycloneDX SBOM and
the third-party notices are attached to every release, and the updater accepts
only manifests signed by the pinned minisign key (`docs/updating.md` §1).

## Automated floors

`scripts/e2e/security-gate.mjs` (S1–S6) runs in CI on all three OSes and in the
release workflow; `scripts/check-arch-invariants.mjs` blocks the structural
regressions (no credential custody in TypeScript, one authorization decider, one
auth vocabulary, one canonical schema). A release candidate additionally passes
`node scripts/release-qualify.mjs`, which reports the security gate as `RUNNABLE`
or `PASS` — never as `BLOCKED`-but-assumed.
