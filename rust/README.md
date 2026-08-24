# sobre — Rust engine

The single crate behind `sobre`: the CLI, the MCP server, the shell hook and the
local proxy. See [../README.md](../README.md) for what the project is, and
[../INVARIANTS.md](../INVARIANTS.md) for the rules it holds itself to.

> **Not released.** Interfaces change without notice.

## Build

```bash
cargo build --release          # local-only default
cargo build --release --features dense   # + local embeddings and GPU providers
```

Feature notes:

- `dense` pulls in ONNX embeddings and an in-process HNSW index. It is opt-in so
  a plain build needs no model download and no runtime preflight. Everything it
  adds still runs on your machine.
- `tree-sitter` (on by default) carries the AST parsers.
- `http-server` (on by default) carries the local HTTP surface.
- Never audit warnings under `--no-default-features`: `dead_code = "deny"`
  produces false positives when features are off.

## Check and test

```bash
cargo check --all-targets                   # what you run while editing
cargo check --all-targets --features dense
cargo check --all-targets --all-features    # what CI checks
cargo test                                  # the whole suite, not --lib
```

`--all-targets` is not optional: plain `cargo check` skips `#[cfg(test)]` blocks
in the library. Neither is running the full `cargo test`: the integration suites
under `tests/` pin a good deal of behaviour and `--lib` never runs them.

## Layout

```
src/
├─ main.rs            entry point
├─ lib.rs             crate root; re-exports drive what dead_code can see
├─ cli/               subcommands, dispatch, completion, help
├─ core/              the engine: cache, compression, graph, session, config
├─ server/            MCP server: registry, dispatch, tool visibility
├─ tools/             the MCP tools themselves
├─ shell/             shell-output compression patterns
├─ proxy/             local provider proxy
├─ hooks/             per-agent installers (Claude Code, Cursor, ...)
├─ dashboard/         local HTTP dashboard
└─ bin/               dev-only generators, behind `required-features`

tests/                integration suites
data/                 bundled generated snapshots (see gen_registry)
```

## Generated files

`data/grammar_registry.json` is a generated snapshot, not a hand-maintained
document. Edit it through the generator, or CI's drift check fails:

```bash
cargo run --example gen_registry --features dev-tools           # rewrite
cargo run --example gen_registry --features dev-tools -- --check # verify
```

A library test asserts the same property, so `cargo test` catches drift without
waiting for CI.

## License

Apache-2.0. See [../LICENSE](../LICENSE), [../NOTICE](../NOTICE) and
[../FORK.md](../FORK.md).
