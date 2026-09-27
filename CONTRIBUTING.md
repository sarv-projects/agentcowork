# Contributing (P70.F5)

## Before you start

Read [`ARCH/00-INDEX.md`](ARCH/00-INDEX.md) — the architecture door — and
[`AGENTS.md`](AGENTS.md), which is the durable contract for any agent or human
working here. [`AGENTCOWORK-SPEC.md`](AGENTCOWORK-SPEC.md) is the product
contract; [`TODO.md`](TODO.md) is delivery status, not a wish list.

## The invariants you must not break

1. **The sidecar proposes; the Rust core disposes.** Every mutating effect needs
   a Rust-minted authorization ticket.
2. **Provider keys live only in the vault.** No credential custody in
   TypeScript — `scripts/check-arch-invariants.mjs` fails the build on it.
3. **No capability claim without evidence.** If a surface cannot do something,
   it says so in place (that is why several switches render disabled with the
   reason attached).

## Running the checks

```bash
(cd crates && cargo test)                    # Rust workspace
(cd crates && cargo clippy --all-targets --all-features -- -D warnings)
pnpm -r test                                    # JS/TS suites
pnpm --filter ui tsc --noEmit                # UI typecheck
node scripts/check-doc-sync.mjs              # docs ↔ capability census
node scripts/check-doc-refs.mjs               # cross-document references resolve
node scripts/ipc-parity.mjs                  # UI ↔ Tauri command parity
node scripts/release-qualify.mjs             # the release gate (plan mode)
```

Every gate named above runs in CI; a PR that turns one red is not mergeable.

## Changes that touch the release surface

- A new file, moved module or renamed crate: re-run
  `node scripts/check-doc-refs.mjs`.
- A new durable store: register it in `crates/agentcowork-core/src/store_schema.rs`
  and add its row to `PACKAGING.md` §6 (`check-store-schemas.mjs` enforces both).
- A new version number: change it **once** in
  `src-tauri/tauri.conf.json` — `check-versions.mjs` fails if the lockstep
  consumers disagree.
- A new public-facing claim: state what it does *not* do in the same breath.

## Commits

Small, coherent, descriptive commits. Commit messages describe the software
change and its motivation — never the tool, model, assistant or agent that
happened to make the edit. See `AGENTS.md` §7 for the list of names that must
never appear in history, comments or generated files.

## Licence

Contributions are accepted under the repository's dual licence
(`LICENSE-MIT`, `LICENSE-APACHE`). Third-party additions must pass
`node scripts/check-licences.mjs`; the skill store keeps its own split
(store *code* MIT, store *content* CC0 — `AGENTS.md` §8).
