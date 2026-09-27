# Windows v1 validation artifacts

This directory contains the operator-facing artifacts for validating the
Windows v1 candidate on a real Windows host. They are validation instructions,
not a release qualification record.

## Files

- [`windows-v1-runbook.md`](windows-v1-runbook.md) — the Windows setup,
  standard-gate, live-gate, inspection, and evidence procedure.
- [`../../scripts/run-windows-v1-validation.ps1`](../../scripts/run-windows-v1-validation.ps1)
  — a fail-closed PowerShell runner. It requires an explicit `-Repo`, copies the
  current working tree to the fixed path
  `C:\Users\sonali\Desktop\tests\AgentCowork`, and writes a timestamped evidence
  directory under `C:\Users\sonali\Desktop\tests\evidence`.

## Safety contract

- The source path is never guessed. Omitting `-Repo` is an error that names the
  required source-path contract.
- The copy includes tracked and untracked implementation files, including
  uncommitted changes. It is **not** a clean checkout.
- Git commit, branch, status, a redacted patch, and hashes/names for untracked
  files are recorded before validation. Git history is not copied.
- Secret-shaped files, credential/key material, local databases, logs, build
  caches, `node_modules`, Rust `target` output, and generated sidecar/build
  output are excluded from the copy and checked again after copying.
- Standard checks run by default. Live checks require `-Live`; missing tools,
  credentials, hosts, artifacts, or manual evidence are recorded as
  `BLOCKED`, never as `PASS`.
- The runner uses per-command logs and native exit codes, writes
  `results.csv`, `summary.json`, and `evidence.sha256`, and exits nonzero when
  any result is `FAIL` or `BLOCKED`.

## Current source inventory

The runbook cites the repository files that define the commands and version
requirements rather than treating an old checklist as authoritative. The key
starting points are `package.json:6-32`, `ui/package.json:7-13`,
`packages/coordinator/package.json:8-13`, `crates/Cargo.toml:1-36`,
`rust-toolchain.toml:1-3`, and `src-tauri/Cargo.toml:1-11`.

The later Windows operator should review the runbook and execute it; this
change only adds the documentation and runner. No PowerShell or Windows
commands were executed while creating these artifacts.
