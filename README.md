# sobre

**Context tools for coding agents that run on your machine and nowhere else.**

`sobre` sits between your coding agent and your repository. The agent asks for a
file, a search or a shell command; `sobre` answers with the smallest output that
still does the job — signatures instead of whole files, a compressed build log
instead of ten thousand lines, a short stub instead of the same file read three
times. Every output it changes says so, and the original stays reachable.

It works with Claude Code, Codex, Copilot CLI, Cursor, Gemini CLI, OpenCode,
VS Code, Windsurf and Zed.

```bash
tar xzf sobre-<target>.tar.gz -C ~/.local/bin   # a release binary, see Install
sobre init                                      # connect every agent it finds
sobre doctor                                    # check the wiring
```

Then restart your agent.

## Install

### A release binary

Each [release](https://github.com/malochapo/sobre-ai/releases) has two builds
per platform. `sobre-dense-…` adds local embeddings (search by meaning); the
other one does everything else.

| Platform | Without embeddings | With embeddings |
|---|---|---|
| Linux x86_64 | `sobre-x86_64-unknown-linux-musl.tar.gz` | `sobre-dense-x86_64-unknown-linux-gnu.tar.gz` |
| Linux arm64 | `sobre-aarch64-unknown-linux-musl.tar.gz` | `sobre-dense-aarch64-unknown-linux-gnu.tar.gz` |
| macOS Apple silicon | `sobre-aarch64-apple-darwin.tar.gz` | `sobre-dense-aarch64-apple-darwin.tar.gz` |
| macOS Intel | `sobre-x86_64-apple-darwin.tar.gz` | none (see [Platform notes](#platform-notes)) |
| Windows x64 | `sobre-x86_64-pc-windows-msvc.zip` | `sobre-dense-x86_64-pc-windows-msvc.zip` |

`SHA256SUMS` lists every file's checksum:
`sha256sum --check --ignore-missing SHA256SUMS`.

Each archive holds the one binary. Put it in a directory on your `PATH` that it
will stay in:

```bash
mkdir -p ~/.local/bin
tar xzf sobre-aarch64-apple-darwin.tar.gz -C ~/.local/bin
```

On Windows, unzip `sobre.exe` into a folder on your `PATH`.

### From source

You need [rustup](https://rustup.rs) — a distribution's own `cargo` ignores the
toolchain pinned in `rust/rust-toolchain.toml` — and a C compiler: the Xcode
Command Line Tools on macOS, `build-essential` on Debian and Ubuntu, the Visual
Studio Build Tools on Windows. A release build takes several minutes.

```bash
git clone https://github.com/malochapo/sobre-ai
cd sobre-ai
make install        # installs ~/.local/bin/sobre
```

Add `~/.local/bin` to your `PATH` if it is not there. `make install` builds
without embeddings; with them, or on Windows where there is no `make`, run cargo
from `rust/` so rustup picks up the pinned toolchain:

```bash
cd rust
cargo install --path . --locked --root ~/.local --features dense
```

### Install before you connect

`init` writes the binary's absolute path into every agent's configuration. Put
the binary where it will stay first: a binary run from `rust/target/release`
breaks every agent at the next `cargo clean`. If you move it, run
`sobre doctor --fix` from the new location.

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

All five hold today: 15 tools, one fixed surface per build; one config file
with 11 keys and no per-project files, profiles or behaviour-changing
environment variables; no network request you did not ask for; and no gate,
approval or hidden transform of sobre's own.

## Connect agents

```bash
sobre init               # every agent it detects
sobre init claude        # one agent: claude, codex, copilot, cursor, gemini,
                         # opencode, vscode, windsurf or zed
sobre init --check       # what init would change; writes nothing
```

Run with no agent named, in a terminal, `init` asks two questions:

- whether to turn on the local [proxy](#proxy);
- in a build with embeddings, whether to download ONNX Runtime and the
  embedding model now (about 120 MB).

Then restart the agent, or reload its MCP servers, so it starts `sobre`.

### What `init` writes

Everything goes into user-level configuration, never into a repository. `init`
writes nothing into your shell rc files or your own instruction files
(`CLAUDE.md`, `AGENTS.md`, `.cursor/rules`, …): the agent learns how to use the
tools from the MCP server instructions, and from the skill when it opens it.

The first time `sobre` edits an existing file, it copies it to
`<file>.sobre.bak` next to it; `uninstall` restores from that copy. A file
`sobre` creates gets an empty `<file>.sobre.bak`, so `uninstall` knows it can
delete the file when nothing but `sobre`'s entries is left in it.

| Agent | MCP server entry | Hooks | Skill |
|---|---|---|---|
| Claude Code | `~/.claude.json` | `~/.claude/settings.json`: compress shell output, stub unchanged re-reads, session observer | `~/.claude/skills/sobre/` |
| Codex | `~/.codex/config.toml`, or the active profile's `<name>.config.toml` | `~/.codex/hooks.json`: session observer; sets `[features] hooks = true` | `~/.codex/skills/sobre/` |
| Copilot CLI | `~/.copilot/mcp-config.json` | `~/.copilot/hooks/hooks.json`: compress shell output, session observer | `~/.copilot/skills/sobre/` |
| Cursor | `~/.cursor/mcp.json` | `~/.cursor/hooks.json`: wrap shell commands, session observer | `~/.cursor/skills/sobre/` |
| Gemini CLI | `~/.gemini/settings.json` | the same file: wrap shell commands, session observer | — |
| OpenCode | `~/.config/opencode/opencode.json` | the plugin `~/.config/opencode/plugins/sobre.js`: compress shell output | `~/.config/opencode/skills/sobre/` |
| VS Code | `mcp.json` in VS Code's user settings directory | the Copilot CLI hooks file | `~/.copilot/skills/sobre/` |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` | `~/.codeium/windsurf/hooks.json`: session observer | — |
| Zed | `~/.config/zed/settings.json` (under `$XDG_CONFIG_HOME` on Linux when set, `%APPDATA%\Zed\` on Windows) | — | — |

`CLAUDE_CONFIG_DIR` and `CODEX_HOME` move those agents' files, and `init`
follows them. The session observer records which conversation and
model are active, so that `sobre_read` can stub a file re-read in the same
conversation and `sobre gain` can price savings.

### Shell output

Where the host allows it, a hook compresses the output of the agent's own shell
commands:

| Agent | Shell output compression |
|---|---|
| Claude Code, Copilot CLI, OpenCode | After the command ran: the output the model sees is replaced by a compressed copy. The command itself is never changed. |
| Cursor, Gemini CLI | Before the command runs: a single known command (`git`, `cargo`, `npm`, …) is wrapped in `sobre -c`, only when none of the host's own command rules tells the two forms apart. |
| Codex, VS Code, Windsurf, Zed | None: these hosts would need sobre to approve the command, can only block it, or have no hooks. Use the `sobre_shell` tool. |

Compressed output always ends with a `[sobre: …]` line that says so and where
the full output was kept. `sobre` never approves, denies or blocks anything:
what may run, be read or be written is the agent host's decision, and `init`
writes no approval setting into its configuration.

## Check it works

```bash
sobre doctor                  # the install, the agents, the data directories
sobre doctor integrations     # each agent's MCP entry, hooks and skill
sobre doctor --fix            # rewire the agents, prune stale caches
```

`doctor --fix` repairs what sobre wrote in your user configuration. It never
edits files in a repository: when a project's own `.vscode/mcp.json` or
`.cursor/mcp.json` also registers `sobre`, `doctor` names the file, and you
remove the entry if you do not want it there.

In the agent, the MCP server list shows `sobre` with 15 tools (`/mcp` in Claude
Code). From a shell, without an agent:

```bash
sobre call sobre_compose --project-root "$PWD"   # the project overview the agent gets
sobre -c "git status"                            # a command, compressed
sobre gain                                       # what has been saved so far
```

## Everyday use

Keep working with your agent as before: it reaches for `sobre`'s tools when they
fit, and its native tools keep working. The 15 tools
([reference](docs/reference/generated/mcp-tools.md)):

| Tool | What it does |
|---|---|
| `sobre_compose` | The project overview, or the files and source that matter for a task |
| `sobre_read` | A file at the right granularity: signatures, map, a line range, the diff since the last read, anchored for editing, or full |
| `sobre_search` | Regex, symbol and semantic search, and code related to a line |
| `sobre_glob`, `sobre_tree`, `sobre_outline` | Files by name, directory shape, the symbols of a file |
| `sobre_graph` | Callers, callees and call chains; the files a change affects |
| `sobre_review` | What a change to a file touches: dependents, call sites, tests |
| `sobre_patch` | Edits by line anchors, unique replacements, new files |
| `sobre_refactor` | References, definitions and rename previews through the language server |
| `sobre_shell` | Shell commands with compressed output, in the background if long |
| `sobre_expand` | The full original behind a compressed output |
| `sobre_checkpoint` | Snapshots of the agent's changes, kept apart from your git history |
| `sobre_session`, `sobre_knowledge` | The session's task and decisions; facts kept across sessions |

Each build has one fixed list of tools and parameters: the build with
embeddings adds a single parameter, `sobre_search`'s `mode`. The reference
lists the build without embeddings.

The same compression is available from a shell:

```bash
sobre -c "cargo test"                  # compressed
sobre raw "cargo test"                 # exact output
sobre call sobre_read --json '{"path": "src/main.rs", "mode": "signatures"}'
```

`sobre call` prints what the tool returned and exits 1 when the tool reports
an error, as `isError` does for the agent; a wrong tool name or argument exits 2.

## Configure

One file, `~/.config/sobre/config.toml`, with 11 keys. Every key has a default,
so the file can stay empty.

```bash
sobre config                               # every key and the value in effect
sobre config set compression_level 0.5     # change one key
sobre config set compression_level default # remove it: the default applies
sobre config init                          # rewrite it with every key documented
sobre config path                          # where it is
```

`config set` edits the file in place, keeping its comments. It refuses a value
the proxy or the tools would not use (an upstream without `http://` or
`https://`, port 0, a level outside 0–1) and a key it does not know (exit 2).
Setting a key to its default, or to `default`, removes it from the file. A
boolean takes `true`/`false` (also `1`/`0`, `yes`/`no`); a list takes
comma-separated values. Keys that redirect traffic ask for confirmation, or
`--yes`.

The keys are `search_roots`, `ignore`, `passthrough_commands`,
`shell_timeout_secs`, `hook_binary`, `compression_level`, `debug_log`,
`embedding_model`, `proxy.enabled`, `proxy.port` and `proxy.upstreams` (one key,
whatever it holds). [config-keys.md](docs/reference/generated/config-keys.md)
documents each.

`compression_level` runs from 0 to 1 and defaults to 0.3. Below 0.2 it is off;
above, it condenses the prose in tool and shell output (each condensed output
says so), shortens the tool descriptions from 0.4, and adds an `OUTPUT STYLE`
line to the MCP instructions that asks the agent to write its own replies more
tersely, from "concise" to "raw-dense". When the agent keeps re-reading or
re-running to get back what was cut, `sobre` lowers compression for the rest of
the session and says so.

## Local embeddings

The `sobre-dense-…` builds, and source builds with `--features dense`, can
search by meaning: `sobre_search(action="semantic")` with `mode="dense"` or
`mode="hybrid"`. They run a small embedding model on your machine with ONNX
Runtime, loaded at run time.

```bash
sobre embeddings provision   # Microsoft's ONNX Runtime (CPU) and the model
sobre embeddings status      # what sobre will load
sobre index build            # build the indexes now rather than on first search
```

Instead of `provision`, `ORT_DYLIB_PATH` can point at an ONNX Runtime library you
already have (1.24 or later). `embedding_model` chooses the model. A GPU needs a
source build with a GPU feature (`--features ort-cuda`, `ort-rocm`,
`ort-webgpu`, `ort-directml` or `ort-coreml`) and `ORT_DYLIB_PATH` pointing at a
GPU build of ONNX Runtime.

## Measure

Every compression `sobre` makes, on every path, is appended to one local file,
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

```bash
sobre proxy enable      # start it at login and point the agents at it
sobre proxy status      # running? which port, which upstreams
sobre proxy disable     # stop it and put the agents back
```

`sobre proxy enable` starts a local proxy on `127.0.0.1` and points the agents
that can use it at it, each through its own configuration: Claude Code
(`settings.json`), Codex (`config.toml`) and OpenCode (`opencode.json`). It
forwards Anthropic Messages and OpenAI Chat Completions and Responses traffic,
compresses the tool results inside each request, and records the usage the
provider reports. Every tool result it rewrites ends with a `[sobre: …]` line
that says so and where the full original was kept; the rest of the request body
goes upstream as the agent sent it.

It starts at login through a LaunchAgent on macOS
(`~/Library/LaunchAgents/com.sobre.proxy.plist`) and a systemd user service on
Linux (`~/.config/systemd/user/sobre-proxy.service`). If the service cannot
start the proxy (no user session, for instance), `proxy enable` fails and
changes nothing. Windows has no login service: start the proxy by hand with
`sobre proxy start --detach`, then run `sobre proxy enable`, which refuses to
wire the agents while nothing answers on the port. Once it is enabled, the MCP
server also starts it when nothing answers.

The port is `proxy.port`. Unset, it is 4444 + uid mod 1000 on Unix, so two
people on one machine get different ports (uid 1000 gets 4444, macOS's first
user, 501, gets 4945), and 4444 on Windows.

The proxy forwards the key the agent sends and never adds one, so it works
only with API keys: a subscription login (Claude Pro or Max, ChatGPT) keeps
talking to its provider directly. It refuses requests that carry neither a
provider key nor its own token (`sobre proxy token`). Gemini CLI does not go
through it. The upstreams are `proxy.upstreams.anthropic` and
`proxy.upstreams.openai`; a local model server that speaks the Anthropic API,
such as Ollama, is reached by pointing `anthropic` at it:

```bash
sobre config set proxy.upstreams.anthropic http://127.0.0.1:11434 --yes
```

If Claude Code already points at an endpoint of your own (`ANTHROPIC_BASE_URL`
in its `settings.json`, remote or local, such as LiteLLM or Ollama), `proxy
enable` leaves it alone and says so. `proxy enable --force` puts the proxy in
front of it instead: it saves that endpoint as `proxy.upstreams.anthropic`
(unless you set one), prints where, and `proxy disable` or `uninstall` puts it
back. A Codex `openai_base_url` or an OpenCode `baseURL` of your own is never
replaced.

`sobre` keeps what it wrote into each agent's configuration, and what it
replaced, in `proxy_wiring.json` in its state directory; `proxy disable`,
`proxy cleanup`, `doctor --fix` and `uninstall` undo exactly that and nothing
else. `proxy disable` also stops the proxy, wherever it was started from.
`sobre proxy cleanup` removes the proxy addresses left in agent configurations
while the proxy is off.

## Where data lives

The same layout on every platform, under your home directory:

| Directory | Holds | Override |
|---|---|---|
| `~/.config/sobre` | `config.toml` | `SOBRE_CONFIG_DIR`, `XDG_CONFIG_HOME` |
| `~/.local/share/sobre` | the usage ledger, sessions, memory, search and graph indexes, checkpoints, the proxy token, the managed ONNX Runtime | `SOBRE_DATA_DIR`, `XDG_DATA_HOME` |
| `~/.local/state/sobre` | logs, the debug log, the full outputs behind compressed ones (kept 24 hours), the hooks' shared state (`hooks/`) and the proxy wiring record (`proxy_wiring.json`) | `SOBRE_STATE_DIR`, `XDG_STATE_HOME` |
| `~/.cache/sobre` | embedding models | `SOBRE_CACHE_DIR`, `XDG_CACHE_HOME` |

`SOBRE_*_DIR` names the directory itself; `XDG_*_HOME` names its parent. With
`debug_log = true`, `sobre` writes a readable log of tool calls and of what its
hooks changed; `sobre doctor` prints where.

## Upgrade

Replace the binary at the same path — download the new release, or
`make install` again — and restart your agents. With the proxy on, also run
`sobre proxy restart`. If the binary's path changed, run `sobre doctor --fix`
from the new one.

## Uninstall

```bash
sobre uninstall
```

`uninstall` lists each change as it makes it. It removes:

- `sobre`'s entries from every agent configuration, its hooks, plugin and
  skills; a file it edited goes back to its `.sobre.bak` copy when nothing else
  changed in it;
- the proxy, its login service and the addresses it wrote into agent
  configurations;
- the config, data, state and cache directories — usage history, sessions,
  memory, indexes and models included;
- the binary, unless it sits in a build directory or under `~/.cargo` (run
  `cargo uninstall sobre` then).

`--keep-config` keeps `config.toml`, `--keep-binary` keeps the binary. Any
other argument is an error, and nothing is removed.

## Platform notes

- **Linux.** The builds without embeddings are static and run on any
  distribution. The `sobre-dense-…` builds need glibc 2.28 or later, which rules
  out Alpine and other musl distributions: ONNX Runtime has to be loaded at run
  time, which a static binary cannot do.
- **macOS.** The binaries are neither signed nor notarized. If Gatekeeper
  refuses to run one, clear the download flag:
  `xattr -d com.apple.quarantine ~/.local/bin/sobre`. Intel Macs have no
  `sobre-dense` build because Microsoft ships no ONNX Runtime for them: build
  from source with `--features dense`, install ONNX Runtime with
  `brew install onnxruntime`, and point `ORT_DYLIB_PATH` at its
  `libonnxruntime.dylib`.
- **Windows.** No `make` and no proxy login service (see [Proxy](#proxy)).
  `uninstall` cannot delete the `sobre.exe` it runs from: delete it by hand
  afterwards.

## License

Apache-2.0. See [LICENSE](LICENSE).
