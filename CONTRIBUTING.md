# Contributing

`sobre` is a small personal project and is not looking for feature
contributions. If you have found a bug, an issue is welcome. Any change is held
to the rules in [INVARIANTS.md](INVARIANTS.md).

There is no CLA. Contributions are under Apache-2.0, like the rest of the
repository.

## Working in this codebase

Two rules shape every change:

**Names lie.** A module's name can say less than what it holds, or something
else. Read what a thing does before acting on what it is called.

**The compiler is the only reliable oracle.** Grep decides what to *attempt*;
only the compiler decides whether it worked.

```bash
cd rust
cargo check --all-targets                  # the default, local-only build
cargo check --all-targets --features dense # with local embeddings
```

CI checks both builds. The one local gate that mirrors it is
`scripts/preflight.sh` (fmt, clippy on both builds, rustdoc, generated docs,
file size); `scripts/preflight.sh full` adds the whole test suite and the
library tests of the build with embeddings. CI also checks dependency licences
and advisories with cargo-deny (`rust/deny.toml`), which the script does not:
run `cargo deny check` from `rust/` for that. The git
hooks are opt-in: `make setup-hooks` points git at `.githooks/`, whose
pre-commit checks formatting and whose pre-push runs `scripts/preflight.sh`
(`SKIP_PREFLIGHT=1 git push` skips it).

On a pull request, CI runs Lint and the test suite on Linux. macOS and Windows
run on push to `main` and on demand (Actions → CI → Run workflow): their
minutes bill at 10x and 2x on a private repository. Run them by hand before
merging anything platform-specific (paths, shells, file locking).

`--all-targets` matters: plain `cargo check` skips `#[cfg(test)]` blocks in the
library, so a broken test module compiles clean without it.

Never audit warnings under `--no-default-features`: `dead_code = "deny"` reports
false positives when features are off.

## Tests

```bash
cd rust && cargo test
```

Run cargo from `rust/`: rustup reads the pinned toolchain
(`rust/rust-toolchain.toml`) from the current directory, so
`--manifest-path rust/Cargo.toml` from the root builds with whatever toolchain
is the default.

Run the whole thing, not `--lib`. The library tests are the bulk of the count,
but a good deal of behaviour is pinned by the integration suites under
`rust/tests/`, and `--lib` never touches them.

Every pull request into `develop` or `main` must keep the suite green on Linux,
macOS and Windows. A failure is a real failure: fix it, never skip it.

## Contracts

Each file in `docs/contracts/` is listed, with its status, in
`core::contracts::contract_docs()`. A frozen one (`usage-ledger-v1.md`) does
not change: a new version goes in a new `-v2.md` file. A typo fix in a frozen
file updates `docs/contracts/frozen-hashes.json`:

```bash
cd rust && SOBRE_UPDATE_FROZEN_HASHES=1 cargo test --test main contracts_frozen
```

## Removing code

What works:

1. **Neutralise behaviour first, keep the shape.** Make the function return the
   constant it now always returns, leave its signature alone. This compiles
   immediately and reaches the intended behaviour.
2. **Let the compiler enumerate what died.** Note that `dead_code` does not see
   `pub` items, and the modules in `lib.rs` are `pub`, so it will not find
   everything.
3. **Cut, with an assertion on the block's bounds.** Pattern-based bulk edits
   silently eat the wrong lines; assert on the first and last line of what you
   are deleting.

When a test asserts behaviour that can no longer occur, prefer retargeting it
onto the guarantee that survives over deleting it. A test that pinned a real
property is worth keeping even when the mechanism it pinned has changed.

## License

Apache-2.0. See [LICENSE](LICENSE), [NOTICE](NOTICE) and [FORK.md](FORK.md).
