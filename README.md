# sobre

**Context engineering for coding agents, on your machine and nowhere else.**

`sobre` sits between your coding agent and your repository. The agent asks for a
file, a search, a shell command; `sobre` answers with the smallest thing that
still does the job — signatures instead of whole files, a compressed shell
transcript instead of ten thousand lines of build output, a cached re-read
instead of the same file three times.

It is a command-line program and an MCP server. It runs locally, stores its
state locally, and talks to nothing over the network unless you point it at
something yourself.

> **Status: not released.** This repository is under active reduction. Interfaces
> change without notice and there is no upgrade path between commits. Do not
> depend on it yet.

## What it is not

`sobre` is a reduced hard fork of [lean-ctx](https://github.com/yvgude/lean-ctx)
by Yves Gugger (Apache-2.0). lean-ctx is a broader product: hosted plans, team
servers, shared indexes, an addon marketplace, a plugin system. Those are real
features and they are not defects — they are simply not what this fork is for.

`sobre` keeps one shape: **one machine, one person, one config file.** Anything
that only makes sense with a second user, a second machine or a paid tier has
been removed. If you want the full product, use lean-ctx.

See [FORK.md](FORK.md) for the statement of changes required by Apache-2.0, and
[NOTICE](NOTICE) for attribution.

## The rules this fork holds itself to

These are enforced, not aspirational — see [INVARIANTS.md](INVARIANTS.md) for
the measured state of each.

1. **A small tool surface.** At most 20 MCP tools, and no universal invoker that
   smuggles the rest back in behind one name.
2. **One config file.** No profile cascade, no per-workspace override chain, no
   environment variable that quietly changes what a tool does.
3. **One answer to `tools/list`.** The same tools for every client, every
   session, every configuration. A surface that varies is a surface nobody can
   reason about.
4. **Nothing to sell.** No accounts, no plans, no telemetry, no shared instance.
5. **A ceiling on size.** The Rust under `rust/src` stays below a fixed line
   budget, so growth has to displace something.

Invariants 1 and 5 hold today. Invariant 2 does not yet — the config still
carries far more keys than the rule allows, and reducing it is ongoing work.

## Build

Requires a recent stable Rust toolchain.

```bash
cargo build --release --manifest-path rust/Cargo.toml
```

The default build is local-only. Optional local embeddings and GPU execution
providers are opt-in:

```bash
cargo build --release --manifest-path rust/Cargo.toml --features dense
```

Everything under `dense` still runs on your machine — it downloads a model, not
your code.

## License

Apache-2.0. See [LICENSE](LICENSE).
