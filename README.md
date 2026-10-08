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
5. **The agent host decides.** `sobre` allows, blocks and hides nothing: it
   labels every change it makes and keeps the original reachable.

All five hold today: 15 tools, one fixed surface; one config file with 11 keys
and no per-project files, profiles or behaviour-changing environment
variables; no network request you did not ask for; and no gate, approval or
hidden transform of sobre's own.

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


```bash
sobre init            # every detected agent
sobre init claude     # one agent: claude, codex, copilot, cursor, gemini,
                      # opencode, windsurf or zed
sobre doctor          # check the install
sobre uninstall       # remove everything init wrote
```

`init` registers the MCP server with the agent and installs the `sobre` skill
for agents that load skills (Claude Code, Codex, Cursor, Copilot, OpenCode).
Where the host allows it, it also adds a hook that compresses the output of the
agent's own shell commands:

| Agent | Shell output compression |
|---|---|
| Claude Code, Copilot CLI, OpenCode | After the command ran: the output the model sees is replaced by a compressed copy. The command itself is never changed. |
| Cursor, Gemini CLI | Before the command runs: a single known command (`git`, `cargo`, `npm`, …) is wrapped in `sobre -c`, only when none of the host's own command rules tells the two forms apart. |
| Codex, VS Code, Windsurf | None: these hosts would need sobre to approve the command, or can only block it. Use the `sobre_shell` tool. |

Compressed output always ends with a `[sobre: …]` line that says so and where
the full output was kept. sobre never approves, denies or blocks anything: what
may run, be read or be written is the agent host's decision, and `init` writes
no approval setting into its configuration.

Everything goes into user-level configuration, never into a repository. `init`
writes nothing into your shell rc files or your own instruction files
(`CLAUDE.md`, `AGENTS.md`, `.cursor/rules`, …): the agent learns how to use the
tools from the MCP server instructions, and from the skill when it opens it.

## Measuring

Every compression sobre makes, on every path, is appended to one local file,
`usage.jsonl`, in the OpenTelemetry log format
([contract](docs/contracts/usage-ledger-v1.md)). Nothing sends it anywhere.
`sobre gain` reads it:

```bash
sobre gain                  # tokens saved, value, sobre's own overhead
sobre gain --by-tool        # per tool or command
sobre gain --daily          # per day
```

Tokens saved are measured. What they are worth in money is only measured when
the optional proxy sits between the agent and its provider: it records the
tokens each call was billed, and `gain` prices them. Without the proxy, `gain`
prices saved tokens at a model's input rate and says it is an estimate.

## Proxy

`sobre proxy enable` starts a local proxy on `127.0.0.1` and points the
agents that can use it at it, each through its own configuration: Claude Code
(`settings.json`), Codex (`config.toml`) and OpenCode (`opencode.json`). It
forwards Anthropic Messages and OpenAI Chat Completions and Responses traffic,
compresses the tool results inside each request, and records the usage the
provider reports. Every tool result it rewrites ends with a `[sobre: …]` line
that says so and where the full original was kept; the rest of the request body
goes upstream as the agent sent it.

The proxy forwards the key the agent sends and never adds one, so it works
only with API keys: a subscription login (Claude Pro or Max, ChatGPT) keeps
talking to its provider directly. Gemini CLI does not go through it. The
upstreams are `proxy.upstreams.anthropic` and `proxy.upstreams.openai`; a local
model server that speaks the Anthropic API, such as Ollama, is reached by
pointing `anthropic` at it.

If Claude Code already points at an endpoint of your own (`ANTHROPIC_BASE_URL`
in its `settings.json`), `proxy enable` leaves it alone and says so.
`proxy enable --force` puts the proxy in front of it instead: it saves that
endpoint as `proxy.upstreams.anthropic` (unless you set one), prints where, and
`proxy disable` or `uninstall` puts it back.

## License

Apache-2.0. See [LICENSE](LICENSE).
