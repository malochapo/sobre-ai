# Appendix — Command Line (generated)

<!-- GENERATED FILE — do not edit by hand. Run: `cargo run --example gen_docs --features dev-tools` -->

Source of truth: `rust/src/cli/args.rs`, the clap declarations of every command. They parse the command line (an unknown flag, a missing value or an unexpected argument exits 2 before anything runs) and print `sobre <command> --help`; `sobre help all` and this page are rendered from them.

Global: `sobre --version` (`-V`) prints the version; `--help` (`-h`) after any command but `-c` and `raw`, which take the rest of the line, prints its help.

## `sobre mcp`

Run the MCP server on stdin/stdout.

```text
sobre mcp
```

Agents start this themselves. Bare `sobre` does the same when stdin is not a terminal; in a terminal it prints a short quickstart instead.

## `sobre -c`

Run a shell command with compressed, labelled output.

```text
sobre -c <command>...
```

| Argument | Meaning |
|---|---|
| `<command>...` | The command: one quoted argument, or several joined with spaces |

What the Cursor and Gemini hooks run. The output says, on stderr, what was compressed; `sobre_expand` brings back the original. Exits with the command's own exit code.

## `sobre raw`

Run a shell command with its exact output, uncompressed.

```text
sobre raw <command>...
```

| Argument | Meaning |
|---|---|
| `<command>...` | The command: one quoted argument, or several joined with spaces |

Exits with the command's own exit code.

## `sobre call`

Call one MCP tool from the shell.

```text
sobre call <tool> [--project-root <path>] [--json <json>] [--json-file <path>]
```

| Argument | Meaning |
|---|---|
| `<tool>` | The tool's name: sobre_read, sobre_search, ... |
| `--project-root <path>` | The project the tool works in (default: the git root around the current directory) |
| `--json <json>` | The tool's arguments, as a JSON object |
| `--json-file <path>` | Read the arguments from this file |

Each call starts an empty session, as a new server does. Exits 1 when the tool reports an error, 2 on a usage error.

## `sobre init`

Connect agents to sobre: MCP entry, hooks, skill.

```text
sobre init [<agent>...] [--check] [--repair] [--json] [--agent <agent>]
```

| Argument | Meaning |
|---|---|
| `[<agent>...]` | The agents to connect |
| `--check` | List the files init would create or change, and write nothing |
| `--repair` | Ask no question (for scripts and CI) |
| `--json` | Machine-readable report; asks no question |
| `--agent <agent>` | Same as naming the agent; repeatable |

Without an agent: every installed agent gets its MCP entry, hooks and skill, and init asks once about the proxy and, in a build with embeddings, about downloading the model. With agents (claude, codex, copilot, cursor, gemini, opencode, vscode, windsurf, zed): only those, installed or not, with no question. Never edits shell rc files. Edits keep comments and key order; a file sobre edits for the first time is copied to <file>.sobre.bak. Exits 1 when an agent could not be connected. Undo it all: sobre uninstall.

## `sobre doctor`

Check the install: binary, config, agents, hooks, proxy, data.

```text
sobre doctor [--fix] [--json]
```

| Argument | Meaning |
|---|---|
| `--fix` | Repair the agents that have sobre, drop stale indexes, and say what was done |
| `--json` | Machine-readable output |

Exits 1 when a check fails. More checks: sobre doctor integrations, doctor overhead, doctor lint-context.

## `sobre doctor integrations`

Check each agent's integration.

```text
sobre doctor integrations [--fix] [--json]
```

| Argument | Meaning |
|---|---|
| `--fix` | Repair the integrations first |
| `--json` | Machine-readable output |

## `sobre doctor overhead`

The fixed context cost sobre adds to every request.

```text
sobre doctor overhead [--json] [--gate]
```

| Argument | Meaning |
|---|---|
| `--json` | Machine-readable output |
| `--gate` | Exit 1 when the cost is over the 8,000-token budget |

## `sobre doctor lint-context`

Low-signal or duplicate lines in the context sobre injects.

```text
sobre doctor lint-context [--json]
```

| Argument | Meaning |
|---|---|
| `--json` | Machine-readable output |

## `sobre gain`

Tokens saved, their value, real cost with the proxy, and overhead.

```text
sobre gain [--by-tool] [--daily] [--json] [--period <all|today|Nd>] [--limit <n>] [--model <model>] [--reset]
```

| Argument | Meaning |
|---|---|
| `--by-tool` | Savings per tool or command |
| `--daily` | Saved tokens and real cost per day |
| `--json` | Machine-readable output |
| `--period <all\|today\|Nd>` | The period to cover (default: all) |
| `--limit <n>` | Rows shown by --by-tool (default: 10) |
| `--model <model>` | Price saved tokens at this model's rate |
| `--reset` | Clear the usage ledger |

Every figure comes from the usage ledger (usage.jsonl). Real cost needs the proxy; money figures are estimates.

## `sobre index build`

Build the project's indexes.

```text
sobre index build [--force] [--root <path>]
```

| Argument | Meaning |
|---|---|
| `--force` | Drop the indexes first and rebuild from scratch |
| `--root <path>` | The project (default: the git root around the current directory) |

The graph and BM25 indexes, and the dense index in a build with local embeddings. Leave files out with `ignore` in config.toml.

## `sobre index status`

What is built, and how large.

```text
sobre index status [--json] [--root <path>]
```

| Argument | Meaning |
|---|---|
| `--json` | Machine-readable output |
| `--root <path>` | The project (default: the git root around the current directory) |

## `sobre config`

Show the config file (same as config show).

```text
sobre config
```

## `sobre config show`

Show the config file.

```text
sobre config show
```

## `sobre config set`

Change one key.

```text
sobre config set <key> <value> [--yes]
```

| Argument | Meaning |
|---|---|
| `<key>` | A key `sobre config` lists |
| `<value>` | Its new value |
| `-y, --yes` | Skip the confirmation a change that widens what sobre does asks for |

Booleans take true/false (also yes/no, 1/0); a list takes comma-separated values. The value is checked before the file is written; the file keeps its comments. `default`, an empty value or the key's default value removes the key, so its default applies. Exits 2 on an unknown key or a value sobre would not use.

## `sobre config path`

Print the config file's path.

```text
sobre config path
```

## `sobre config init`

Write the config file with every key documented.

```text
sobre config init
```

Keeps an existing file as config.toml.sobre.bak.

## `sobre embeddings`

The local ONNX runtime and embedding model (same as embeddings status).

```text
sobre embeddings
```

## `sobre embeddings status`

The local ONNX runtime and embedding model (builds with local embeddings).

```text
sobre embeddings status
```

## `sobre embeddings provision`

Download the ONNX runtime and the model.

```text
sobre embeddings provision [--force]
```

| Argument | Meaning |
|---|---|
| `--force` | Download again even when present |

## `sobre proxy start`

Run the API proxy (in the foreground unless --detach).

```text
sobre proxy start [--port <n>] [--detach]
```

| Argument | Meaning |
|---|---|
| `-p, --port <n>` | The port to listen on (default proxy.port) |
| `-d, --detach` | Run in the background |

## `sobre proxy stop`

Unload the autostart unit and stop the proxy.

```text
sobre proxy stop [--port <n>]
```

| Argument | Meaning |
|---|---|
| `-p, --port <n>` | The proxy's port (default: the port it was last started on, else proxy.port) |

## `sobre proxy restart`

Restart the proxy so it re-reads config.toml.

```text
sobre proxy restart [--port <n>]
```

| Argument | Meaning |
|---|---|
| `-p, --port <n>` | The proxy's port (default: the port it was last started on, else proxy.port) |

## `sobre proxy status`

The proxy's config, process and live upstreams.

```text
sobre proxy status [--port <n>]
```

| Argument | Meaning |
|---|---|
| `-p, --port <n>` | The proxy's port (default: the port it was last started on, else proxy.port) |

## `sobre proxy enable`

Turn the proxy on.

```text
sobre proxy enable [--force]
```

| Argument | Meaning |
|---|---|
| `--force` | Also rewire agents that point to another endpoint |

Sets proxy.enabled, installs the autostart unit (launchd on macOS, a systemd user service on Linux) and points the agents at the proxy. Where there is no service manager (Windows, Linux without a systemd user session), it needs a proxy started by hand first (sobre proxy start -d), and refuses without one. Exits 1 when the proxy does not run.

## `sobre proxy disable`

Turn the proxy off and restore the agents' endpoints.

```text
sobre proxy disable
```

## `sobre proxy cleanup`

Remove proxy URLs left in agent configs while the proxy is off.

```text
sobre proxy cleanup
```

## `sobre proxy token`

Print the proxy's bearer token.

```text
sobre proxy token [--quiet]
```

| Argument | Meaning |
|---|---|
| `-q, --quiet` | Print the token alone |

## `sobre hook`

Internal: run by the hooks init installs.

```text
sobre hook <compress|cursor|gemini|read-dedup|observe>
```

| Argument | Meaning |
|---|---|
| `<compress\|cursor\|gemini\|read-dedup\|observe>` | The hook that fired |

## `sobre uninstall`

Remove sobre from this machine.

```text
sobre uninstall [--keep-config] [--keep-binary]
```

| Argument | Meaning |
|---|---|
| `--keep-config` | Keep config.toml, for a later reinstall |
| `--keep-binary` | Leave the sobre binary in place |

Removes the proxy (autostart unit, process, the URLs it put in agent configs); sobre's MCP entry, hooks, skill and plugin from every agent (a file that held nothing else is deleted); all sobre data; and the binary the agents run, unless cargo installed it (cargo uninstall sobre) or it is a build in a cargo target directory. On Windows a running binary cannot be deleted: delete it once uninstall has exited. Every change is listed as it is made. Running agents keep their sobre server until they restart. Any other argument is an error: nothing is removed.

## `sobre help`

The common commands; `help all` prints the full reference.

```text
sobre help [all]
```

| Argument | Meaning |
|---|---|
| `[all]` | Print every command, flag and exit code instead |

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | The command ran and failed: a tool error (`call`), a failed check (`doctor`, `doctor integrations`, an error from `doctor lint-context`, `doctor overhead --gate`), an agent not connected (`init`), a proxy that would not start, a `config set` not confirmed |
| 2 | Usage error: an unknown command, flag or argument, or a flag without its value. Nothing ran. |

`-c` and `raw` exit with the command's own exit code.
