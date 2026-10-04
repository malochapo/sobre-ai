# Appendix — Configuration Keys (generated)

<!-- GENERATED FILE — do not edit by hand. Run: `cargo run --example gen_docs --features dev-tools` -->

Source of truth: `rust/src/core/config/schema.rs`.

lean-ctx reads one file, `~/.lean-ctx/config.toml`. Below is every recognized key with its type and default.

## Top-level keys

Top-level configuration keys

- `allow_ide_config_dirs` (bool, default `null`) — Allow jailed ctx_* tools to read home-level IDE config dirs (registry-derived; covers all editors). Off by default — exposes other agents' sessions/credentials
- `allow_paths` (string[], default `[]`) — Additional paths allowed by PathJail (absolute)
- `allow_symlink_roots` (string[], default `[]`) — Trusted roots OUTSIDE $HOME lean-ctx may follow when an agent config is symlinked there (#596). Empty = strict $HOME-only
- `compression_level` (f64, default `null`) — Compression intensity 0.0 (none) – 1.0 (densest). Tunes read-mode compression continuously and picks the prose style by band: <0.2 off, <0.4 lite, <0.6 standard, <0.8 max, else raw. Empty = adaptive defaults with the lite prose style
- `debug_log` (bool, default `false`) — Opt-in (default off): write a human-readable debug log of intercepted MCP tool calls and hook routing decisions (lean-ctx vs native, with the reason) to <state_dir>/logs/debug.log. View with `lean-ctx debug-log`
- `extra_ignore_patterns` (string[], default `[]`) — Extra glob patterns to ignore in graph/overview/preload
- `extra_roots` (string[], default `[]`) — Extra project roots for multi-root workspaces (auto-added to PathJail allow-list)
- `hook_binary` (string?, default `null`) — Verbatim binary path/expression for generated agent-hook commands (e.g. $HOME/.local/bin/lean-ctx) — for settings files synced across machines with different usernames. Shell-expanded by the hook host at run time; doctor accepts it as current. Empty = automatic absolute-path resolution
- `passthrough_commands` (string[], default `[]`) — Commands whose output is passed through uncompressed (the command still runs normally)
- `path_jail` (bool?, default `null`) — Filesystem path jail. null/true = enforced (tools confined to the project root + allow_paths). false = the blanket "any path" opt-out — every tool path is allowed (for containers/sandboxes where the boundary is external). Compression and secret redaction are unaffected. Flip both planes at once with `lean-ctx yolo` / `lean-ctx secure`
- `project_root` (string?, default `null`) — Explicit project root directory. Prevents accidental home-directory scans
- `proxy_enabled` (bool?, default `null`) — Enable/disable the proxy layer. null = auto-detect, true = force on, false = force off
- `proxy_port` (u16?, default `null`) — Custom proxy port (default: 4444). Useful for multi-user systems.
- `read_only_roots` (string[], default `[]`) — Read-only sibling roots: reads allowed, writes always denied (edit/refactor/export)
- `rules_injection` (enum: shared | dedicated | off, default `shared`) — How rules load for CLAUDE.md/AGENTS.md/GEMINI.md agents: shared block, dedicated (no shared-file edits; SessionStart hook / instructions[] / context.fileName), or off (write no rules file — for hosts that supply their own steering or phase-isolated/non-caching harnesses).
- `rules_scope` (enum: both | global | project, default `both`) — Where agent rule files are installed.
- `sandbox_level` (u8, default `0`) — Sandbox strictness level (0=default, 1=strict, 2=paranoid)
- `shell_activation` (enum: always | agents-only | off, default `agents-only`) — Controls when the shell hook auto-activates aliases (agents-only since #699: transparent in plain human terminals)
- `shell_heavy_timeout_secs` (u64?, default `null`) — Shell command timeout (seconds) for heavy commands (cargo build/test, make, docker build, git commit/push). null = built-in 10-minute ceiling
- `shell_hook_disabled` (bool, default `false`) — Disable shell hook injection
- `shell_security` (string, default `enforce`) — Shell command gating: enforce (default, secure), warn (log only, never block) or off (skip the hard blocks; compression stays active)
- `shell_timeout_secs` (u64?, default `null`) — Shell command timeout (seconds) for normal commands. null = built-in 2-minute default
- `skip_agent_aliases` (bool, default `false`) — Do not install agent CLI aliases (claude, codex, gemini) into shell rc files. Existing alias blocks are removed on next setup

## `[index]`

Index-time file filters: declare the retrieval corpus explicitly (BM25 + graph + semantic + watch share one filter layer, #735)

- `exclude` (string[], default `[]`) — Globs dropped from the index corpus (root-relative, forward slashes), e.g. ["**/*.csv", "fixtures/**"]. Wins over include. CLI --exclude appends per run. Excluded files produce no chunks, graph nodes, or embeddings.
- `include` (string[], default `[]`) — When non-empty, ONLY matching files enter the index corpus, e.g. ["**/*.rs", "**/*.ts"]. Empty = no restriction. CLI --include replaces this set per run.
- `respect_gitignore` (bool, default `true`) — Honor .gitignore / global gitignore / .git/info/exclude during index walks. false indexes ignored files too (the vendor-directory guard still applies). CLI override: --no-gitignore / --respect-gitignore.

## `[proxy]`

Proxy upstream configuration for API routing

- `allow_custom_upstream` (bool, default `false`) — Allow a custom (non-allowlisted) HTTPS upstream host, e.g. a corporate gateway in front of the provider API. Opt-in; default false. Unlike the env var, this config flag reaches the managed (service-spawned) proxy started by `proxy enable`/`restart` (#590)
- `allow_insecure_http_upstream` (bool, default `false`) — Allow a non-loopback plaintext http:// upstream (trusted local network only, e.g. http://host.docker.internal:2455 in front of codex-lb). Opt-in; default false
- `anthropic_upstream` (string?, default `null`) — Custom upstream URL for Anthropic API proxy
- `gemini_upstream` (string?, default `null`) — Custom upstream URL for Gemini API proxy
- `openai_upstream` (string?, default `null`) — Custom upstream URL for OpenAI API proxy

## `[setup]`

Controls what lean-ctx injects during setup and updates. Fresh installs default to non-invasive (rules/skills off, MCP on).

- `auto_inject_rules` (bool?, default `null`) — Inject agent rule files during setup/update. null=auto (inject if already present), true=always, false=never
- `auto_update_mcp` (bool, default `true`) — Register lean-ctx MCP server in editor configs during setup/update

