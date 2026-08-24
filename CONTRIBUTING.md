# Contributing

`sobre` is a personal project under active reduction. It is not looking for
feature contributions right now, and most of the work in flight is *removing*
code rather than adding it. If you have found a bug, an issue is welcome.

There is no CLA. Contributions are under Apache-2.0, like the rest of the
repository.

## Working in this codebase

The code is inherited from a much larger product and is being cut down. Two
consequences shape every change:

**Names lie.** A module called `addons` held artifact-execution infrastructure,
not a marketplace. A file called `cheatsheet_cmd.rs` held the implementation of
an unrelated live command. Read what a thing does before acting on what it is
called.

**The compiler is the only reliable oracle.** Grep decides what to *attempt*;
only the compiler decides whether it worked.

```bash
cargo check --all-targets                  # the default, local-only build
cargo check --all-targets --features dense # local embeddings + GPU providers
cargo check --all-targets --all-features   # what CI checks
```

`--all-targets` matters: plain `cargo check` skips `#[cfg(test)]` blocks in the
library, so a broken test module compiles clean without it.

Never audit warnings under `--no-default-features`: `dead_code = "deny"` reports
false positives when features are off.

## Tests

```bash
cargo test --manifest-path rust/Cargo.toml
```

Run the whole thing, not `--lib`. The library tests are the bulk of the count,
but a good deal of behaviour is pinned by the integration suites under
`rust/tests/`, and `--lib` never touches them.

The suite is not fully green: a number of failures pre-date the fork or need
tools that may not be on your machine (a `python` on `PATH`, for instance).
Before claiming a change is clean, compare against a baseline run of the same
suite on the commit you started from — the absolute count is not the signal, the
delta is.

## Removing code

Most changes here are deletions. What works:

1. **Neutralise behaviour first, keep the shape.** Make the function return the
   constant it now always returns, leave its signature alone. This compiles
   immediately and reaches the intended behaviour.
2. **Let the compiler enumerate what died.** Note that `dead_code` does not see
   `pub` items re-exported from `lib.rs`, so it will not find everything.
3. **Cut, with an assertion on the block's bounds.** Pattern-based bulk edits
   silently eat the wrong lines; assert on the first and last line of what you
   are deleting.

When a test asserts behaviour that can no longer occur, prefer retargeting it
onto the guarantee that survives over deleting it. A test that pinned a real
property is worth keeping even when the mechanism it pinned has changed.

## License

Apache-2.0. See [LICENSE](LICENSE), [NOTICE](NOTICE) and [FORK.md](FORK.md).
