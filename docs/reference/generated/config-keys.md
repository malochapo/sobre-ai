# Appendix — Configuration Keys (generated)

<!-- GENERATED FILE — do not edit by hand. Run: `cargo run --example gen_docs --features dev-tools` -->

Source of truth: `rust/src/core/config/schema.rs`.

lean-ctx reads one file, `~/.lean-ctx/config.toml`. Below is every recognized key with its type and default.

## Top-level keys

Top-level configuration keys

- `allow_paths` (string[], default `[]`) — Paths the tools may read and write besides the project root; each is also searched as an extra root. ["/"] opens the path jail
- `compression_level` (f64, default `null`) — Compression intensity 0.0 (none) – 1.0 (densest). Tunes read-mode compression continuously and picks the prose style by band: <0.2 off, <0.4 lite, <0.6 standard, <0.8 max, else raw. Empty = adaptive defaults with the lite prose style
- `debug_log` (bool, default `false`) — Opt-in (default off): write a human-readable debug log of intercepted MCP tool calls and hook routing decisions (lean-ctx vs native, with the reason) to <state_dir>/logs/debug.log. View with `lean-ctx debug-log`
- `hook_binary` (string?, default `null`) — Verbatim binary path/expression for generated agent-hook commands (e.g. $HOME/.local/bin/lean-ctx) — for settings files synced across machines with different usernames. Shell-expanded by the hook host at run time; doctor accepts it as current. Empty = automatic absolute-path resolution
- `ignore` (string[], default `[]`) — Globs kept out of the index and project walks, on top of .gitignore (always respected)
- `passthrough_commands` (string[], default `[]`) — Commands whose output is passed through uncompressed (the command still runs normally)
- `read_only_roots` (string[], default `[]`) — Read-only sibling roots: reads allowed, writes always denied (edit/refactor/export)
- `shell_hook` (bool, default `true`) — Install the shell hook that routes common commands through sobre
- `shell_timeout_secs` (u64?, default `null`) — Shell command timeout (seconds) for normal commands. null = built-in 2-minute default

## `[proxy]`

The local API proxy: switch, port, upstream URLs and model routing

- `enabled` (bool?, default `null`) — Run the local API proxy and point installed agents at it. null = not decided yet (init asks), false = never touch it
- `port` (u16?, default `null`) — Port the proxy listens on (default: 4444)
- `routing` (table, default `{}`) — Model routing: [proxy.routing.aliases] maps a requested model to "model" or "provider:model"; [proxy.routing.tiers] maps fast/standard/premium to a target. Active as soon as one rule is written
- `upstreams.anthropic` (string?, default `null`) — Upstream URL for the anthropic provider (unset = its default). Writing a URL is the opt-in for a custom or plaintext host
- `upstreams.bedrock` (string?, default `null`) — Upstream URL for the bedrock provider (unset = its default; bedrock is served only when set). Writing a URL is the opt-in for a custom or plaintext host
- `upstreams.commandcode` (string?, default `null`) — Upstream URL for the commandcode provider (unset = its default). Writing a URL is the opt-in for a custom or plaintext host
- `upstreams.gemini` (string?, default `null`) — Upstream URL for the gemini provider (unset = its default). Writing a URL is the opt-in for a custom or plaintext host
- `upstreams.grok-chat` (string?, default `null`) — Upstream URL for the grok-chat provider (unset = its default). Writing a URL is the opt-in for a custom or plaintext host
- `upstreams.ollama` (string?, default `null`) — Upstream URL for the ollama provider (unset = its default). Writing a URL is the opt-in for a custom or plaintext host
- `upstreams.openai` (string?, default `null`) — Upstream URL for the openai provider (unset = its default). Writing a URL is the opt-in for a custom or plaintext host
- `upstreams.xai` (string?, default `null`) — Upstream URL for the xai provider (unset = its default). Writing a URL is the opt-in for a custom or plaintext host

