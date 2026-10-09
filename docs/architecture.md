# Architecture

How `sobre` is put together: the processes it runs, how a request flows
through them, where its state lives, and the rules that shape all of it. The
reference pages list every tool, key, command and variable; this page explains
how they fit. Paths in `code` are relative to `rust/src/` unless they start
with `~` or name a top-level file.

## 1. Scope

`sobre` sits between a coding agent and a repository. The agent asks for a
file, a search or a shell command, and `sobre` answers with the smallest
output that still does the job. Every output it changes says so, and the
original stays reachable.

Six invariants ([INVARIANTS.md](../INVARIANTS.md)) define the project, and
each one shows up in the code:

| Invariant | Where it shows |
|---|---|
| 1. At most 20 tools | `server/registry.rs` registers 15; a test in `server/tool_visibility.rs` fails past 20 |
| 2. One config file, no cascade | `core/config/`: one `config.toml`, 11 keys pinned by `config::schema::key_cap`; no variable changes behaviour (`core/env_vars.rs` lists them all) |
| 3. One `tools/list` per build | the list never depends on the client, the config or the session; the dense build adds one parameter |
| 4. Local and personal | no account, telemetry or remote store; the network is reached only by the proxy and by `embeddings provision` |
| 5. The agent host decides | no allow or deny list, no path jail, no approval written into a host; every change to an output is labelled |
| 6. Use what exists | maintained crates for the protocol, parsing, HTTP and file formats; the code that predates the rule is listed in issue #69 |

## 2. Component map

One crate, one binary. The modules group into five layers:

```
                 agent host (Claude Code, Codex, Cursor, …)
        stdio JSON-RPC │        hook JSON │        HTTP │
                       ▼                  ▼             ▼
   ┌────────────── entry points (cli/dispatch) ───────────────────┐
   │ mcp server       hook handlers      proxy       CLI commands │
   │ server/, tools/  hook_handlers/     proxy/      cli/, doctor/│
   │ mcp_stdio.rs     rewrite_registry   proxy_setup/ uninstall/  │
   │                  compound_lexer                  setup/      │
   └──────────────┬──────────────┬─────────────┬──────────────────┘
                  ▼              ▼             ▼
   ┌────────────────────────── engine ────────────────────────────┐
   │ shell/ (exec, output policy), core/patterns/ (compression)   │
   │ core/ config, paths, read cache, compression, tee, redaction │
   │       search index, BM25, code graph, embeddings (dense)     │
   │       session, knowledge, checkpoints (shadow git), usage    │
   │ lsp/ (language servers for sobre_refactor)                   │
   └──────────────────────────────────────────────────────────────┘
                  ▼
   ~/.config/sobre   ~/.local/share/sobre   ~/.local/state/sobre   ~/.cache/sobre
```

`rust/README.md` maps every directory under `src/`.

## 3. Processes and lifetimes

`main.rs` installs a panic hook that writes the crash log, then hands
`argv[1]` to `cli/dispatch/mod.rs`, which picks the mode. On macOS a process
re-parented to launchd first re-executes itself under a seatbelt sandbox (see
[Security](#10-security-model)).

| Process | Started by | Lives | Talks over |
|---|---|---|---|
| MCP server (`sobre`, `sobre mcp`) | the agent host | as long as the host keeps it; exits when its parent changes (checked every 5 s) | stdin/stdout, JSON-RPC |
| Hook handler (`sobre hook <action>`) | the agent host, once per event | one event: 5 s watchdog, 15 s fail-open for the pre-run rewrites | JSON on stdin and stdout |
| `sobre -c`, `sobre raw` | a Cursor or Gemini hook, or a person | one command; exits with its code | the terminal |
| Proxy (`sobre proxy start`) | a LaunchAgent, a systemd user unit, `--detach`, or the MCP server | until stopped | HTTP on `127.0.0.1` |
| CLI commands (`init`, `doctor`, `gain`, `call`, …) | a person | one command | the terminal |
| Language servers | `sobre_refactor`, one per language and project | the MCP server's lifetime | LSP over pipes |
| Background shell jobs | `sobre_shell` | threads of the MCP server; kept 5 minutes after they end | in process |

**MCP server startup** (`cli/dispatch/server.rs`):
- backs off when it has been restarted more than 8 times in 60 s;
- holds the `mcp-startup` lock for up to 3 s;
- sizes its thread pools down when other `sobre` processes run.

During the handshake it terminates orphaned `sobre` servers (parent PID 1). When
`proxy.enabled = true`, no login service runs and nothing answers on the port,
it also starts the proxy detached. It builds no index at startup: indexes are
built when a tool first needs them.

**Proxy lifetime** (`proxy_autostart.rs`):
- **macOS:** a LaunchAgent (`~/Library/LaunchAgents/com.sobre.proxy.plist`) keeps it alive.
- **Linux:** a systemd user unit (`~/.config/systemd/user/sobre-proxy.service`) restarts it on failure.
- **Windows:** it runs only when started by hand.
- **Stopping:** `proxy stop` signals a process only after its `/health` answers as the `sobre` proxy.

## 4. Request flows

### An MCP tool call

1. `mcp_stdio.rs` reads a message. It accepts both JSON lines and
   `Content-Length` framing.
2. `server/server_handler.rs` answers `initialize`. It records the client
   name and sets the project root: the host's workspace, else the git root
   around the working directory. It adds the configured `search_roots` as extra
   roots, skipping broad ones, and starts storage maintenance in the background.
3. `tools/list` returns the registry: 15 tools, descriptions condensed by
   `compression_level`, read-only and destructive hints added
   (`tool_defs/`). It never announces `listChanged`.
4. `tools/call` → `server/call_tool.rs`:
   1. On the first call, the server asks the host for its roots (`roots/list`,
      up to 5 s, under `roots_gate`).
   2. `server/dispatch/` resolves every path argument against the project root
      and the session's working directory (`server/multi_path.rs`, `core/path_resolve.rs`). An
      absolute path outside the project re-roots the session, and the output
      says `[sobre: project root is now …]`.
   3. The index a tool needs is warmed (`core/index_orchestrator.rs`): the
      in-memory search index for `sobre_search`, a background graph and BM25
      build for `sobre_graph`, `sobre_compose` and `sobre_review`.
   4. The handler runs on the blocking pool under a 120 s watchdog
      (`sobre_shell` has its own timeouts).
   5. Re-read, re-run and `sobre_expand` signals may lower compression for two
      minutes, announced in the output (`core/refetch_signals.rs`).
   6. `sobre_session` and `sobre_knowledge` output is condensed by
      `server/post_process.rs`, unless the check in `core/output_verification.rs`
      finds a path or identifier damaged.
   7. The compression is appended to the usage ledger (`core/usage/`), and the
      session is saved when enough has changed.
   8. A failing tool sets `isError`, and `structuredContent.exitCode` where it
      has one.

### A hook, per host

Hooks never approve, deny or block anything. Each host gets one of two
strategies (`hooks/agents/`, `hook_handlers/`):

- **Post-tool compression** (`hook compress`). The command runs as the agent
  wrote it. Afterwards, the output the model sees is replaced by a compressed
  copy ending in `[sobre: output compressed, A → B tokens; full output: <path>]`.
  - Hosts: Claude Code (`PostToolUse`), Copilot CLI (`postToolUse`) and OpenCode (plugin).
  - File reads (`cat`, `head`, `tail`) are left alone.
- **Labelled pre-run rewrite** (`hook cursor`, `hook gemini`). A command is
  wrapped as `sobre -c '<command>'` only when all of these hold:
  - it is a single command, with no chain, pipe, redirect or substitution;
  - it is not a file read;
  - it is one of the 52 commands in `rewrite_registry.rs`;
  - none of the host's own command rules tells the two forms apart (`hook_handlers/host_rules.rs`).

Two more hooks help the read cache:
- `hook read-dedup` turns a Claude Code re-read of an unchanged file into an
  `[unchanged]` stub.
- `hook observe` records the active model, transcript and session. It also
  clears the dedup state when the host compacts its context.

### `sobre -c`

`shell/exec.rs` checks these in order:
- `SOBRE_WRAPPED` is set (a command `sobre` itself started): the command passes through.
- A nested `sobre -c` is collapsed.
- An interactive or long-running command (`shell/compress/passthrough.rs` plus
  `passthrough_commands`), or output redirected to a file, runs with its output
  untouched.

Otherwise `shell/pipeline.rs` captures the output and compresses it with the
first matching pattern (`core/patterns/`). It stores the full output and prints
`[sobre: output compressed, A → B tokens; full output: <path>]` on stderr.

### The proxy

`proxy/mod.rs` serves `/v1/messages`, `/v1/chat/completions`,
`/v1/responses`, `/v1/models`, `/health` and `/status` on `127.0.0.1`.

**Guards:**
- A Host guard accepts only loopback names.
- Every route except `/health` needs the proxy's token or a provider key.

**Forwarding** (`proxy/forward/`):
- decompresses the body, compresses tool results, and forwards allow-listed headers to the upstream;
- never adds a credential;
- records the provider's reported usage in the ledger.

**What it compresses:**
- Anthropic `tool_result` blocks, Chat Completions `tool` messages, and Responses `function_call_output` items.
- Never: file reads, sobre's own tool results, results under 512 bytes, or tools whose name contains `serena`.
- Each rewritten result ends with `[sobre: compressed by the proxy, …; full original at <path>, or sobre_expand(id=…)]`.

It also prunes old history in strides of 16 messages, never before the last
prompt-cache breakpoint. It re-reads `proxy.upstreams` every 5 s.

### `init` and `uninstall`

**`init`** (`setup/`) runs three steps for each agent:
1. writes the MCP entry (`core/editor_registry/`);
2. installs the hooks and the skill (`hooks/`, `setup/skill.rs`);
3. points the agent at the proxy when it is enabled (`proxy_setup/`).

The first edit of an existing file copies it to `<file>.sobre.bak` (`config_io.rs`).
A file `sobre` creates gets an empty `.sobre.bak`. The README's table lists
every file per agent.

**`uninstall`** (`uninstall/`) undoes this. It first removes the proxy:
the login service, the process, and the addresses recorded in
`proxy_wiring.json`. Then it removes every agent entry, hook, plugin and skill,
then the data directories, then the binary. A file restored from its
`.sobre.bak` loses only `sobre`'s entries.

## 5. Compression and recovery

Every path that shortens output keeps the original and says where it is:

| Path | Original kept at | Brought back by |
|---|---|---|
| `sobre_shell`, `sobre -c`, `hook compress` | `<state>/tee/<slug>_<hash>.log` | `sobre_expand(id=…)`, or reading the file |
| the proxy | `<state>/tee/proxy_<hash>.log` | `sobre_expand(id=…)` |
| `sobre_read` modes other than `full` | the file itself | `sobre_read mode=full` (or `raw=true`) |
| an `[unchanged]` re-read stub | the file itself | `sobre_read fresh=true` |

Stored originals are redacted, owner-only (0600) and deleted after 24 hours.
The originals the proxy keeps are only those of 512 bytes or more.
`sobre_expand` accepts only names it created, resolved inside `<state>/tee`.

The labels are the contract: every shortened output carries a `[sobre: …]`
line naming what changed. When re-reads show the agent lacked something,
compression drops for two minutes, and that change is labelled too.

## 6. State and data layout

The four directories and their overrides are in the README
([Where data lives](../README.md#where-data-lives)). `<h>` below is the
project hash: a short hash of the canonical project root
(`core/project_hash.rs`), so two clones are two projects.

**Config** (`~/.config/sobre`)

| Path | Holds |
|---|---|
| `config.toml` | the configuration |
| `config.toml.sobre.bak` | the file as it was before `config init` rewrote it |

**Data** (`~/.local/share/sobre`)

| Path | Holds | Kept |
|---|---|---|
| `usage.jsonl` | the usage ledger ([contract](contracts/usage-ledger-v1.md)) | until `gain --reset` |
| `sessions/<id>.json` | saved sessions | until `sobre_session action=cleanup` (older than 7 days) |
| `sessions/<id>_snapshot.txt` | compaction snapshots | with their session |
| `knowledge/<h>/knowledge.json`, `relations.json`, `gotchas.json`, `embeddings.json` | facts, links between them, pitfalls, fact embeddings | 200 facts, 50 patterns, 100 history entries; unused facts archived after 90 days |
| `memory/archive/…` | archived facts, history and patterns | the newest 16 per store |
| `graphs/<h>/graph.db`, `graph.meta.json`, `call_graph.json.zst`, `health.json` | the code graph | rebuilt when stale; removed after 48 h unused, or by `doctor --fix` when the project is gone |
| `vectors/<h>/bm25_index.bin.zst`, `embeddings.bin`, `project_root.txt` | BM25 and dense indexes | as above; BM25 is not saved above 512 MB |
| `read_cache/stub_index.json` | which file versions a conversation has already received (no content) | 1024 entries |
| `shadow/<h>/git/` | `sobre_checkpoint` snapshots, a git repository apart from yours | until uninstall |
| `exports/` | knowledge and session exports with no `path` given | until you delete them |
| `detected_model.json`, `active_transcript.json`, `last_compaction.json` | what `hook observe` recorded | overwritten |
| `diagnostics.json` | recent compiler and linter errors | 15 minutes, 200 entries |
| `doctor/latest.json` | the last `doctor --fix` report | overwritten |
| `session_token` | the proxy token (0600) | until uninstall |
| `logs/crash.log` | panics (0600) | rotated at 1 MB |
| `onnxruntime/<version>/` | ONNX Runtime from `embeddings provision` | until uninstall |
| `.<name>.lock`, `.mcp-server-starts.log` | cross-process locks and the restart counter | released or reclaimed |

**State** (`~/.local/state/sobre`)

| Path | Holds | Kept |
|---|---|---|
| `tee/` | full outputs behind compressed ones (0600) | 24 hours |
| `hooks/dedup/`, `hooks/read/` | hook state shared between hook processes | 1 minute; 24 hours or the next compaction |
| `cooccurrence/<h>.json` | files read together, for ranking | bounded, decaying |
| `agent_runtime_env.json` | the agent's `CODEX_*`/`CLAUDE_*`/`OPENCODE_*`/`GEMINI_*` variables, never credentials (0600) | ignored after 2 hours |
| `proxy.log`, `proxy.port` | a detached proxy's output and port | appended; overwritten |
| `proxy_wiring.json` | what the proxy wiring wrote into agent configs, and what it replaced | until disabled |
| `logs/debug.log` | the opt-in debug log | rotated at 5 MB |
| `logs/proxy.stdout.log`, `logs/proxy.stderr.log` | the proxy's output under launchd (macOS) | not rotated |

**Cache** (`~/.cache/sobre`): `models/<model>/`, the embedding model and its
pinned hashes (`model.lock.json`).

**Outside these directories:**
- the agent configurations `init` edits (with their `.sobre.bak`), and the skill and plugin files it owns;
- the proxy's login service file;
- files in your repository that the agent asks `sobre_patch`, `sobre_refactor` or
  `sobre_checkpoint restore` to change. They are written through a temporary
  sibling and a rename.

`sobre` writes nothing else into a repository, and nothing into the system temp directory.

## 7. Concurrency

**Within one process.** `rust/LOCK_ORDERING.md` lists every lock and its order:
- per-file locks serialize reads and edits from concurrent subagents (`core/path_locks.rs`);
- the per-project build registry, the graph build gate, the shell job table and the language-server table;
- the server's session, cache and roots state behind tokio locks.

**Between processes.** Several MCP servers, hooks, the CLI and the proxy share
the data and state directories:
- **Locks.** A lock is a `.<name>.lock` file created exclusively, holding its owner's PID, and reclaimed when that PID is dead or the lock is stale (`core/startup_guard.rs`).
  - Locks: server startup, startup maintenance, and each project's graph, BM25 and search builds.
  - The process that loses a build race waits for the winner and reuses its index; a graph scan waits up to 2 minutes.
- **The usage ledger** is appended under an exclusive file lock.
- **Other writes** go to a temporary sibling, then a rename (`core/atomic_fs.rs`, `config_io.rs`, session and index writers). A reader sees the old file or the new one, never half of one.
- **Per process:** the session, the read cache, the search index, background jobs and language servers. Every process starts an empty session; `sobre_session action=load` restores a saved one.

## 8. Configuration and builds

**Loading** (`core/config/loader.rs`):
- `config.toml` is read when first needed and again whenever its content changes.
- A file that does not parse is ignored whole: every key takes its default, and a warning goes to stderr.
- An unknown key is ignored, and `doctor` names it.
- The proxy re-reads its upstreams every 5 s and keeps the last good value.
- Nothing else is a setting: no profile, no variable that changes behaviour, nothing a previous run remembers as a value.

**Builds:**

| | Default build | Dense build (`--features dense`) |
|---|---|---|
| Semantic search | BM25 | BM25, dense or hybrid (`sobre_search`'s `mode`) |
| `tools/list` | 15 tools | the same, plus `sobre_search`'s `mode` |
| ONNX Runtime | — | loaded at run time: `ORT_DYLIB_PATH`, else the copy `embeddings provision` installed, else the system's |
| `init` | asks about the proxy | also asks about downloading the model |

**Other features:**
- `tree-sitter` (default): 27 grammars.
- `jemalloc` (default): the allocator.
- `ort-cuda`, `ort-rocm`, `ort-webgpu`, `ort-directml`, `ort-coreml`: GPU builds of `dense`.
- `dev-tools`: the reference generator.
- `test-support`: helpers for the integration tests.

## 9. Agent integration

The README's tables ([What init writes](../README.md#what-init-writes),
[Shell output](../README.md#shell-output)) give each agent's files and its
shell strategy. The hook events behind them:

| Agent | Hook events |
|---|---|
| Claude Code | `PostToolUse` Bash → `hook compress`; `PostToolUse` Read → `hook read-dedup`; `hook observe` on PostToolUse, UserPromptSubmit, Stop, PreCompact, SessionStart, SessionEnd |
| Codex | `hook observe` on PostToolUse, SessionStart, SessionEnd |
| Copilot CLI, VS Code | `postToolUse` bash/powershell → `hook compress`; `hook observe` |
| Cursor | `preToolUse` Shell → `hook cursor`; `hook observe` on sessionStart, preCompact |
| Gemini CLI | BeforeTool `run_shell_command` → `hook gemini`; AfterTool → `hook observe` |
| OpenCode | plugin: `tool.execute.after` → `hook compress` |
| Windsurf | `hook observe` |
| Zed | none |

Only Claude Code, Codex and OpenCode can be pointed at the proxy. Gemini CLI
does not go through it.

## 10. Security model

[SECURITY.md](../SECURITY.md) states the threat model.

**`sobre` is no gate.** It runs as you, reads and writes where the agent
points, and runs commands as written. The agent host decides what may run
(invariant 5).

**What it does:**
- **Refuses broad roots** (`/`, `$HOME`, agent config directories) as a
  *project root*, never as a path. This covers MCP roots, `search_roots`,
  `sobre call --project-root` and graph scans (`core/pathutil.rs`).
- **Masks secrets** in its own output: shell output, `sobre_read` (except
  `raw=true`), hook output and stored originals (`core/redaction.rs`,
  `core/secret_detection.rs`).
- **Keeps its files private.**
  - Directories that exist are set to 0700 when resolved.
  - The token, sessions, stored originals, the crash log and the captured variables are 0600.
  - `usage.jsonl` gets the default mode inside its 0700 directory.
- **Locks down the proxy.** It listens on loopback only, checks the Host header, and requires its token or a provider key. It never adds a key.
- **Avoids macOS privacy prompts.** On macOS, the proxy started by its LaunchAgent, and a `sobre` process re-parented to launchd, run under `sandbox-exec` with `~/Documents`, `~/Desktop` and `~/Downloads` denied. A background process therefore never triggers a privacy prompt (`core/tcc_guard_sandbox.rs`).

**Network.**
- The proxy reaches only the upstreams you configure.
- `embeddings provision` downloads ONNX Runtime from Microsoft's GitHub releases (SHA-256 pinned) and the model from HuggingFace (pinned revision, hashes recorded on first download).
- The CLI talks to the local proxy only.
- There is no telemetry and no update check.

## 11. Language servers and parsers

`sobre_refactor` starts a language server per language and project, found on
`PATH` (`lsp/config.rs`):

| Language | Server |
|---|---|
| Rust | `rust-analyzer` |
| TypeScript, JavaScript | `typescript-language-server --stdio` |
| Python | `pylsp` |
| Go | `gopls` |
| Java, Kotlin, Ruby, C, C++, C# | `<language>-language-server` when one is on `PATH` |

Requests time out after 30 s, initialisation after 60 s.

`sobre_outline`, `sobre_read mode=signatures` and the graph parse 27 languages
with tree-sitter: Rust, TypeScript (and TSX), JavaScript, Python, Go, Java, C,
C++, Ruby, C#, Kotlin, Swift, PHP, Bash, Dart, Scala, Elixir, Zig, GDScript,
Lua, Luau, OCaml, Haskell, Julia, Solidity, Nix and PowerShell. Vue and Svelte
files are parsed through their script blocks. The post-edit syntax check covers
21 of them, Rust through Luau in that list.

## 12. Reference

| Page | What it lists | Source of truth |
|---|---|---|
| [mcp-tools.md](reference/generated/mcp-tools.md) | every tool, action and parameter | `tools/registered/` |
| [config-keys.md](reference/generated/config-keys.md) | every config key | `core/config/schema/` |
| [cli.md](reference/generated/cli.md) | every command, flag and exit code | `cli/spec.rs` |
| [env-vars.md](reference/generated/env-vars.md) | every environment variable | `core/env_vars.rs` |
| [contracts/](contracts/) | on-disk formats: usage ledger, session bundle, graph, knowledge | the contract files |
| [LOCK_ORDERING.md](../rust/LOCK_ORDERING.md) | every lock and its order | checked by a test |

## 13. Changing it

- Start from the invariants. A new tool names the one it replaces. A new key
  replaces one: the cap is 15 and a test pins the list. No variable may change
  behaviour. A need a maintained crate meets is met by the crate, not by new
  code of our own.
- The four reference pages are generated:
  `cargo run --example gen_docs --features dev-tools` rewrites them, and CI
  fails when they drift.
  - A new command or flag goes into `cli/spec.rs`; a test fails when a handler reads a flag the table lacks.
  - A new variable goes into `core/env_vars.rs`; a test fails when the code reads one it does not list.
  - A tool argument goes into its schema; a test fails when a handler reads one the schema does not declare.
- The fixed per-turn cost of the tool schemas and instructions is pinned by a
  test (`core/context_overhead.rs`). Raise it only in the commit that justifies
  the growth.
- Any change to an output is labelled, and its original stays reachable.
