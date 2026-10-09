# sobre — Rust engine

The single crate behind `sobre`: the CLI, the MCP server, the agent hooks and
the local proxy. See [../README.md](../README.md) for what the project is, and
[../INVARIANTS.md](../INVARIANTS.md) for the rules it holds itself to.

## Build

The toolchain is pinned in `rust-toolchain.toml`; run cargo from this
directory so rustup picks it up.

```bash
cargo build --release                    # local-only default
cargo build --release --features dense   # + local embeddings
```

Feature notes:

- `dense` pulls in ONNX embeddings and an in-process HNSW index. It is opt-in so
  a plain build needs no model download and no runtime preflight. Everything it
  adds still runs on your machine.
- `tree-sitter` (on by default) carries the AST parsers.
- `dev-tools` enables the generators under `src/bin/`.
- Never audit warnings under `--no-default-features`: `dead_code = "deny"`
  produces false positives when features are off.

## Check and test

```bash
cargo check --all-targets                   # what you run while editing
cargo check --all-targets --features dense  # CI checks both builds
cargo test                                  # the whole suite, not --lib
../scripts/preflight.sh full                # everything CI runs, locally
```

`--all-targets` is not optional: plain `cargo check` skips `#[cfg(test)]` blocks
in the library. Neither is running the full `cargo test`: the integration suites
under `tests/` pin a good deal of behaviour and `--lib` never runs them.

## Layout

```
src/
├─ main.rs            entry point
├─ lib.rs             crate root; re-exports drive what dead_code can see
├─ cli/               subcommands, dispatch, help
├─ core/              the engine: cache, compression, graph, session, config
├─ server/            MCP server: registry, dispatch, tool visibility
├─ tools/             the 15 MCP tools
├─ instructions.rs    the MCP server instructions — the one rules channel
├─ shell/             shell-output compression patterns
├─ proxy/             local provider proxy
├─ hooks/             per-agent installers (Claude Code, Cursor, ...)
├─ hook_handlers/     the `sobre hook ...` commands agents call
├─ setup/             init, per-agent MCP config and the skill installer
└─ bin/               dev-only generators, behind `dev-tools`

skill/SKILL.md        the `sobre` skill, embedded in the binary
tests/                integration suites
```

## Generated files

`../docs/reference/generated/` (the tool and config-key references) is written
by a generator, not by hand. Edit the source, then regenerate, or CI's drift
check fails:

```bash
cargo run --example gen_docs --features dev-tools           # rewrite
cargo run --example gen_docs --features dev-tools -- --check # verify
```

## License

Apache-2.0. See [../LICENSE](../LICENSE), [../NOTICE](../NOTICE) and
[../FORK.md](../FORK.md).
