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

Invariants 1 to 5 hold today: 17 tools, one fixed surface; one config file
with 13 keys and no per-project files, profiles or behaviour-changing
environment variables; no network request you did not ask for; and about
170,000 lines of production Rust under a 200,000 ceiling.

## Build

The Rust toolchain is pinned in `rust/rust-toolchain.toml`; `rustup` installs
it on the first build.

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

## Use

The binary is still called `lean-ctx`; renaming it is pending.

```bash
lean-ctx init            # every detected agent
lean-ctx init claude     # one agent: claude, codex, copilot, cursor, gemini,
                         # opencode, windsurf or zed
lean-ctx doctor          # check the install
lean-ctx uninstall       # remove everything init wrote
```

`init` registers the MCP server with the agent, installs the agent's own hooks
where the host supports them (they route its shell commands through
`lean-ctx -c`), and installs the `sobre` skill for agents that load skills
(Claude Code, Codex, Cursor, Copilot, OpenCode). Everything goes into user-level
configuration, never into a repository. It writes nothing into your shell rc
files or your own instruction files (`CLAUDE.md`, `AGENTS.md`, `.cursor/rules`,
…): the agent learns how to use the tools from the MCP server instructions, and
from the skill when it opens it. Native tools are never denied.

## License

Apache-2.0. See [LICENSE](LICENSE).
