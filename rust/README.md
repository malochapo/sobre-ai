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
- `dev-tools` enables the reference generator, `src/bin/gen_docs.rs` (built as
  the `gen_docs` example).
- `test-support` exposes helpers the integration tests call; never part of a
  release build.
- `jemalloc` (on by default) is the allocator.
- `ort-cuda`, `ort-rocm`, `ort-webgpu`, `ort-directml` and `ort-coreml` each
  turn on `dense` with a GPU execution provider.
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
├─ main.rs              entry point
├─ lib.rs               crate root; re-exports drive what dead_code can see
├─ cli/                 subcommands, dispatch, help; args.rs declares the command
│                       line (clap)
├─ server/              MCP server: registry, dispatch, path resolution, roots,
│                       output post-processing, background shell jobs
├─ tools/               the 15 MCP tools (registered/ holds their schemas)
├─ tool_defs/           tool definitions and their read-only/destructive hints
├─ mcp_stdio.rs         the MCP stdio transport: rmcp's, with a 32 MiB line cap
├─ instructions.rs      the MCP server instructions — the one rules channel
├─ core/                the engine: config, paths, compression, read cache,
│                       search and graph indexes, embeddings, session,
│                       knowledge, usage ledger, reference generators
├─ shell/               shell execution and the output-compression patterns
├─ compound_lexer.rs    splits `a && b | c` into the segments hooks may wrap
├─ rewrite_registry.rs  commands the pre-run hooks wrap in `sobre -c`
├─ hooks/               per-agent hook installers (Claude Code, Cursor, ...)
├─ hook_handlers/       the `sobre hook ...` commands agents call
├─ setup/               init, per-agent MCP config and the skill installer
├─ config_io.rs         backup-then-write for the agent config files init edits
├─ proxy/               the local provider proxy: forward, compress, meter
├─ proxy_setup/         points Claude Code, Codex and OpenCode at the proxy
├─ proxy_autostart.rs   the proxy's login service (launchd, systemd)
├─ lsp/                 language-server client behind sobre_refactor
├─ doctor/              `sobre doctor`: checks and --fix
├─ uninstall/           `sobre uninstall`: agent entries, data, binary
├─ ipc/                 child processes with a hard timeout
├─ terminal_ui.rs       progress and colours for the CLI
├─ engine/              in-process tool calls, for the integration tests only
├─ test_env.rs          environment helpers for tests
└─ bin/gen_docs.rs      the reference generator, behind `dev-tools`

skill/SKILL.md          the `sobre` skill, embedded in the binary
tests/                  integration suites
benches/                compression and efficiency benchmarks
```

How the parts fit at run time is in
[../docs/architecture.md](../docs/architecture.md).

## Generated files

`../docs/reference/generated/` (the MCP tool, config-key, CLI and
environment-variable references) is written by a generator, not by hand. Edit the source, then regenerate, or CI's drift
check fails:

```bash
cargo run --example gen_docs --features dev-tools           # rewrite
cargo run --example gen_docs --features dev-tools -- --check # verify
```

## License

Apache-2.0. See [../LICENSE](../LICENSE), [../NOTICE](../NOTICE) and
[../FORK.md](../FORK.md).
