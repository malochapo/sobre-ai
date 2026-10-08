# Statement of changes

`sobre` is a **reduced hard fork** of [lean-ctx](https://github.com/yvgude/lean-ctx)
by Yves Gugger, licensed under the Apache License 2.0.

This document is the notice of modification required by
**Apache-2.0 section 4(b)**. It applies to the work as a whole: `sobre` is a
substantially modified derivative, and every file in this repository should be
assumed to be modified relative to upstream unless the git history shows
otherwise.

## Provenance

- Fork point: upstream `main` at `4e65c53d4`, corresponding to lean-ctx v3.9.12.
- The **complete upstream git history is preserved** in this repository. Every
  commit authored by Yves Gugger and the upstream contributors remains intact
  and attributable. `git log` is the authoritative record of what came from
  where.
- Upstream is *not* merged automatically. `sobre` is an independent project, not a downstream
  distribution of lean-ctx.

## Relationship to upstream

`sobre` is not a competitor to lean-ctx and does not present itself as a newer,
official or endorsed version of it. lean-ctx aims to be a broad context
platform; `sobre` deliberately aims at a fraction of that scope. Users who need
the full feature surface should use lean-ctx.

Per **Apache-2.0 section 6**, this fork receives no trademark licence. The names
`lean-ctx`, `LeanCTX` and the associated marks, logos and domains belong to
their owner and are used here only descriptively, to identify the origin of this
work.

Contributors to `sobre` are free to contribute to lean-ctx in parallel, and are
encouraged to send genuine bug fixes upstream first.

## What was removed and why

The goal of the fork is **subtraction**, not addition. Categories removed:

| Removed | Reason |
|---|---|
| Mascot, achievements, sprite and ASCII-art subsystem (`core/buddy`) | Gamification; no bearing on the tool's job |
| Cloud server, accounts, billing, org gateway, team server | `sobre` is community open source with nothing to commercialise |
| Web fetching (`ctx_url_read`), LLM-backed features, multi-agent coordination | Outside the scope of a local context layer |
| Web dashboard, background daemon and HTTP server | One local process per agent session; no listening service besides the opt-in proxy |
| Addon marketplace, plugin system, OCLA event bus, WASM runtime | Extension machinery with no extensions to run |
| JetBrains backend, and agents beyond eight | Support only what two maintainers can test |
| Shared session mode | One machine, one person |
| Learning loop (mode predictor, feedback, bandits, threshold learning) | Stores that were written but never read back |
| Cognition loop, knowledge bridge, episodic and procedural memory, findings recorded automatically from tool output | Background or hidden writers the agent never asked for; memory holds only what the agent remembers |
| About twenty usage stores (stats, savings ledger, cost attribution, heatmap, event log, audit trail, Context IR, proxy counters) and the views built on them (Wrapped, energy, CEP score, `tools health`, `output-savings`, `audit`) | One local usage ledger in the OpenTelemetry log format; `gain` computes every figure from it |
| Signed context snapshots and their timeline (`snapshot`), the cognition activity registry and the proxy request analysis (`introspect`) | A time machine and a self-diagnostic for subsystems sobre no longer has; neither is a context layer's job |
| Graph reports and side views: communities, central and bridge nodes, cycles, entry points, smells, enrichment with commits and tests (`ctx_graph explain|diff|cycles|entrypoints|enrich|context`, smells in `ctx_review`), and the CLI-only `repomap`, `smells`, `graph export-html|--heatmap|parity|snapshot` | The graph answers questions about code (dependencies, dependents, impact, symbols, calls); it does not grade it |
| About thirty CLI commands: shell twins of the tools (`read`, `grep`, `glob`, `find`, `ls`, `diff`, `deps`, `graph`, `session`, `sessions`, `knowledge`), reports and self-checks (`benchmark`, `conformance`, `health`, `visualize`, `verify-cache`, `ghost`, `gotchas`, `learn`), cache and log viewers (`cache`, `tee`, `slow-log`, `debug-log`), the transcript compactor (`compact`), the `terse` shortcut for `compression_level`, the live price refresh, shell completions, `dev-install`, `codesign-setup`, the `exec` alias of `-c`, and the top-level `status`, `stop` and `restart` | `sobre call` runs any tool from a shell; `doctor` reports and repairs the install; `proxy stop` and `proxy restart` manage the proxy |
| The downstream packages, language SDKs and editor extensions | A distribution surface two maintainers can support |
| Marketing, blog and email material | Not source code |
| Multiple tool-visibility modes, tool profiles, per-client quirks, `ctx_context`, `ctx_execute`, and `ctx_callgraph` and `ctx_impact` as tools of their own | Replaced by a single fixed surface of 15 tools; `sobre_graph` answers call and impact questions too |
| 150+ configuration keys, profiles and environment overrides | One config file with 11 keys |
| Per-project settings files (`.lean-ctx.json`, `.lean-ctx-artifacts.json`, `.lean-ctx-id`), `multi-repo.toml`, user output filters, behaviour-changing environment variables, settings kept in the session | Behaviour comes from `config.toml` alone |
| The shell hook: rc-file aliases with `track` / `compress` / `off` modes, the `.zshenv` / `.bashenv` redirect, agent aliases, `env.sh`, `lean-ctx -t` | Agent shell commands go through the agent's own hook; sobre never edits shell files |
| Per-repository install (`init --project`, Copilot / VS Code configs inside the repo) | Installed once, at user level |
| Rules files written into agent instruction files, hook modes that deny native tools, the `harden`, `rules` and `export-rules` commands | Guidance travels in one place: the MCP server instructions, plus an on-demand skill |
| The output archive and its full-text index | `sobre_expand(id)` is the one way back to output that compression cut |
| The shell command gate (allow list, blocked commands), the path jail, read-only roots, refusals of secret-looking paths, the loop throttle | The agent host decides what may run, be read or be written; sobre never decides |
| Approval settings written into the host (`autoApprove`, Gemini `trust`, plan-mode permissions), `allow` decisions from hooks | Same: sobre never approves anything on the host's behalf |
| The native Read/Grep redirect and the `cat` / `grep` / `ls` rewrites | They changed what the host's own tools returned; shell output is now compressed after the fact, or wrapped whole in `sobre -c` |
| The output sanitizer, the Windows `;` → `&&` rewrite | Silent changes to output or commands; anything sobre changes is now labelled |
| Proxy extras: Amazon Bedrock signing, model routing and aliases, Anthropic ↔ OpenAI translation, the `/providers` registry, the `/v1/compress` and `/v1/retrieve` gateway API, the local model catalog, prefix replay, sticky tools, SSE keepalives, the WebSocket bridge, injected usage options, OpenRouter and LiteLLM cost accounting, trace ids, and Gemini traffic | The proxy compresses tool results for the agents sobre wires to it, labels every rewrite and records billed usage; it does not route, translate, sign or otherwise change requests, and agents are wired through their own configuration, never shell rc files |
| The contributor licence agreement | Contributions are under Apache-2.0, like the rest of the repository |

## What was renamed

The table above uses the upstream names for what was removed. What stayed
was renamed:

- the binary and its commands: `lean-ctx` is `sobre`;
- the MCP tools: upstream's `ctx_*` tools are `sobre_*` (`ctx_read` is
  `sobre_read`), so a host that lists tools from several servers shows
  whose they are.

The complete removal log, with measured line counts and coupling for each
module, is maintained by the maintainers alongside the repository.

## Licence

`sobre` remains under the Apache License 2.0. See [`LICENSE`](LICENSE) for the
licence text and [`NOTICE`](NOTICE) for the attribution notices, which reproduce
the upstream notices as required by section 4(d).
