# Appendix — Environment Variables (generated)

<!-- GENERATED FILE — do not edit by hand. Run: `cargo run --example gen_docs --features dev-tools` -->

Source of truth: `rust/src/core/env_vars.rs`; a test fails when the code reads a variable it does not list. No variable changes what sobre does (invariant 2): they say where files are, carry a secret, or come from the host. Settings live in config.toml only.

## Where sobre keeps its files

| Variable | Meaning |
|---|---|
| `SOBRE_CONFIG_DIR` | Directory of config.toml (default `$XDG_CONFIG_HOME/sobre`, else `~/.config/sobre`) |
| `SOBRE_DATA_DIR` | Usage ledger, sessions, knowledge, indexes, checkpoints, models (default `$XDG_DATA_HOME/sobre`, else `~/.local/share/sobre`) |
| `SOBRE_STATE_DIR` | Logs, ports, captured agent variables (default `$XDG_STATE_HOME/sobre`, else `~/.local/state/sobre`) |
| `SOBRE_CACHE_DIR` | Rebuildable caches (default `$XDG_CACHE_HOME/sobre`, else `~/.cache/sobre`) |
| `XDG_CONFIG_HOME` | Base of the config directory |
| `XDG_DATA_HOME` | Base of the data directory |
| `XDG_STATE_HOME` | Base of the state directory |
| `XDG_CACHE_HOME` | Base of the cache directory |
| `HOME` | The home directory |
| `USERPROFILE` | The home directory on Windows |
| `HOMEDRIVE` | With HOMEPATH, the home directory on Windows when USERPROFILE is unset |
| `HOMEPATH` | See HOMEDRIVE |
| `APPDATA` | Windows: where Claude Desktop, VS Code, Windsurf and Zed keep their config |
| `ORT_DYLIB_PATH` | Build with local embeddings: the ONNX Runtime library to load. Wins over the one `sobre embeddings provision` installs; a relative path is read from the current directory |

## Secrets

| Variable | Meaning |
|---|---|
| `SOBRE_PROXY_TOKEN` | The proxy's bearer token. Unset: sobre generates one and keeps it in the data directory (`sobre proxy token` prints it) |

## Agents and hosts

| Variable | Meaning |
|---|---|
| `CLAUDE_CONFIG_DIR` | Where Claude Code keeps its config (default `~/.claude`) |
| `CODEX_HOME` | Where Codex keeps its config (default `~/.codex`) |
| `CODEX_PROFILE` | The Codex profile whose config `init codex` edits |
| `COPILOT_HOME` | Where Copilot CLI keeps its config (default `~/.copilot`) |
| `CLAUDE_PROJECT_DIR` | Set by Claude Code: the project root when the client sends no MCP roots |
| `WORKSPACE_FOLDER_PATHS` | Set by Cursor: the workspace folders, used as project roots |
| `CURSOR_TASK_ID` | Set by Cursor for a subagent: its reads get their own re-read stubs |
| `ANTHROPIC_API_KEY` | Its presence tells `proxy enable` that Claude Code and OpenCode use an API key; the value is never read or stored |
| `ANTHROPIC_AUTH_TOKEN` | As ANTHROPIC_API_KEY, for Claude Code |
| `CODEX_API_KEY` | Its presence tells `proxy enable` that Codex uses an API key; the value is never read or stored |
| `CODEX_*` | Variables starting CODEX_, CLAUDE_, OPENCODE_ or GEMINI_ (credentials excluded) that a hook or `sobre -c` sees are kept in `state/agent_runtime_env.json` and passed to `sobre_shell` commands, which the long-lived MCP server would not otherwise see (CODEX_THREAD_ID, …) |
| `CLAUDE_*` | See CODEX_ |
| `OPENCODE_*` | See CODEX_ |
| `GEMINI_*` | See CODEX_ |

## Platform

| Variable | Meaning |
|---|---|
| `SHELL` | The shell `sobre_shell` runs; a non-POSIX shell (fish, nu) is replaced by zsh, bash or sh |
| `COMSPEC` | Windows: the shell when no PowerShell is found |
| `MSYSTEM` | Windows: set under Git Bash/MSYS, where bash is preferred |
| `MINGW_PREFIX` | As MSYSTEM |
| `PATH` | Where `doctor` and `sobre_refactor` look for binaries |
| `LANG` | With LC_ALL and LC_CTYPE: when none names a UTF-8 locale, commands run with one |
| `LC_ALL` | See LANG |
| `LC_CTYPE` | See LANG |
| `NO_COLOR` | Set: no ANSI colours |
| `RUST_LOG` | Log filter for sobre's own diagnostics (stderr, the proxy log) |
| `USER` | Linux, build with local embeddings: the per-user Nix profile searched for ONNX Runtime |
| `USERNAME` | Windows: limits the stray-process cleanup to this user's processes |
| `HOMEBREW_PREFIX` | Build with local embeddings: where Homebrew installs ONNX Runtime |
| `LD_LIBRARY_PATH` | Build with local embeddings: searched for ONNX Runtime (DYLD_LIBRARY_PATH on macOS) |
| `DYLD_LIBRARY_PATH` | See LD_LIBRARY_PATH |

## Set by sobre

| Variable | Meaning |
|---|---|
| `SOBRE_WRAPPED` | Set on every command sobre runs. A `sobre -c` that sees it passes the command through uncompressed, so a nested call never compresses twice. Any process can set it, with that effect |
| `SOBRE_SEATBELT` | macOS: set on the proxy started by its LaunchAgent, which runs under `sandbox-exec` with ~/Documents, ~/Desktop and ~/Downloads denied, so a background process never triggers a privacy prompt |
| `BASH_ENV` | Set empty, with ENV, on the shells sobre starts, so no startup file runs |
| `ENV` | See BASH_ENV |
| `ANTHROPIC_BASE_URL` | Written into Claude Code's settings by `proxy enable`, removed by `proxy disable` |

## Tests only

| Variable | Meaning |
|---|---|
| `SOBRE_TCC_STANDALONE` | Unit tests: forces the macOS standalone-process check |
| `SOBRE_HANDOFF_TEST_PORT` | A helper process of the proxy handoff test |
| `SOBRE_PERF_GATE` | Set to 1: runs the BM25 performance gate |
| `SOBRE_UPDATE_FROZEN_HASHES` | Rewrites docs/contracts/frozen-hashes.json (see CONTRIBUTING) |

## Build time

| Variable | Meaning |
|---|---|
| `SOBRE_RELEASE_VERSION` | The version a release build reports (`--version`, `doctor`, the MCP server info): the release workflow sets it to the tag's version, for example `0.1.0-rc.2`. Without it, sobre reports the crate version |

