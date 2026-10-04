# Appendix — Configuration Keys (generated)

<!-- GENERATED FILE — do not edit by hand. Run: `cargo run --example gen_docs --features dev-tools` -->

Source of truth: `rust/src/core/config/schema.rs`.

lean-ctx reads one file, `~/.lean-ctx/config.toml`. Below is every recognized key with its type and default.

## Top-level keys

Top-level configuration keys

- `allow_auto_reroot` (bool, default `false`) — Allow automatic project-root re-rooting when absolute paths outside the jail are seen
- `allow_ide_config_dirs` (bool, default `null`) — Allow jailed ctx_* tools to read home-level IDE config dirs (registry-derived; covers all editors). Off by default — exposes other agents' sessions/credentials
- `allow_paths` (string[], default `[]`) — Additional paths allowed by PathJail (absolute)
- `allow_symlink_roots` (string[], default `[]`) — Trusted roots OUTSIDE $HOME lean-ctx may follow when an agent config is symlinked there (#596). Empty = strict $HOME-only
- `auto_capture` (bool, default `true`) — Automatic knowledge capture from tool findings
- `auto_mode_learning` (bool, default `false`) — Opt-in: let adaptive learning signals (predictor, bandit, heatmap, adaptive policy, bounce/path memory) influence `auto` mode. Off by default for a deterministic, I/O-light cascade (capability guards + size/task heuristic only) that keeps output byte-stable for prompt caching.
- `bm25_max_cache_mb` (u64, default `128`) — Maximum BM25 cache file size in MB
- `bypass_hints` (enum: on | off | aggressive, default `on`) — Bypass-hint mode: when agents use native Read/Grep instead of lean-ctx tools, a hint is appended to the next tool response. on (default), off, aggressive (hint on every call, no cooldown).
- `cache_max_tokens` (usize, default `0`) — Token budget for the in-memory ctx_read cache (0 = built-in default 500k). When exceeded, least-valuable entries are evicted immediately via RRF (recency x frequency x size) so reads never block; eviction is not deferred to the staleness TTL
- `cache_policy` (enum(aggressive|safe|off), default `aggressive`) — Cache policy for ctx_read: aggressive (13-tok stubs), safe (map on hit), off (always disk)
- `compression_aggressiveness` (f64, default `null`) — Global compression intensity 0.0 (lossless) – 1.0 (max), mapped onto read modes/entropy/IB. Empty = per-mode defaults
- `compression_level` (enum: off | lite | standard | max, default `lite`) — Unified output-style level for the model's prose (not tool-output compression). lite=plain concise (default), standard/max=denser symbolic 'power modes'
- `content_defined_chunking` (bool, default `false`) — Enable Rabin-Karp chunking for cache-optimal output ordering
- `crush_verbatim_json` (bool, default `false`) — Opt-in: losslessly crush array-heavy JSON from verbatim data commands (gh api, jq, kubectl get -o json, curl). Off by default keeps them verbatim. Reshapes only when it at least halves the payload; fully reconstructible
- `debug_log` (bool, default `false`) — Opt-in (default off): write a human-readable debug log of intercepted MCP tool calls and hook routing decisions (lean-ctx vs native, with the reason) to <state_dir>/logs/debug.log. View with `lean-ctx debug-log`
- `delta_explicit` (boolean, default `true`) — Serve explicit full/lines re-reads of changed cached files as diffs (opt-in).
- `enable_wakeup_ctx` (bool, default `true`) — Append wakeup briefing (facts, session summary) to ctx_overview output. Set false to reduce context bloat when calling ctx_overview frequently.
- `extra_ignore_patterns` (string[], default `[]`) — Extra glob patterns to ignore in graph/overview/preload
- `extra_roots` (string[], default `[]`) — Extra project roots for multi-root workspaces (auto-added to PathJail allow-list)
- `graph_index_max_files` (u64, default `15000`) — Maximum files in graph index. 0 = unlimited (default). Set >0 to cap for constrained systems
- `hook_binary` (string?, default `null`) — Verbatim binary path/expression for generated agent-hook commands (e.g. $HOME/.local/bin/lean-ctx) — for settings files synced across machines with different usernames. Shell-expanded by the hook host at run time; doctor accepts it as current. Empty = automatic absolute-path resolution
- `max_disk_mb` (u64, default `0`) — Simplified disk budget in MB (0 = disabled). Distributes: archive ~25%, BM25 ~10%
- `max_index_threads` (usize, default `0`) — Cap rayon threads for the CPU-heavy index build (0 = all cores). Bounds per-instance CPU so concurrent sessions don't saturate the host on startup
- `max_ram_percent` (u8, default `5`) — Soft process-RSS target as % of system RAM (1-50, default 5); eviction/throttling policy, not an OS hard cap
- `max_staleness_days` (u32, default `0`) — Auto-purge data older than N days (0 = disabled). Flows into archive.max_age_hours
- `memory_cleanup` (enum: aggressive | shared, default `aggressive`) — Controls how aggressively memory is freed when idle
- `memory_profile` (enum: low | balanced | performance, default `performance`) — Controls RAM vs feature trade-off (performance = max quality)
- `minimal_overhead` (bool, default `true`) — Skip session/knowledge/gotcha blocks in MCP instructions
- `no_degrade` (boolean, default `false`) — Disable all automatic read-mode degradation.
- `output_density` (enum: normal | terse | ultra, default `normal`) — Controls how dense/compact MCP tool output is formatted
- `passthrough_commands` (string[], default `[]`) — Commands whose output is passed through uncompressed (the command still runs normally)
- `passthrough_urls` (string[], default `[]`) — URLs to pass through without proxy interception
- `path_jail` (bool?, default `null`) — Filesystem path jail. null/true = enforced (tools confined to the project root + allow_paths). false = the blanket "any path" opt-out — every tool path is allowed (for containers/sandboxes where the boundary is external). Compression and secret redaction are unaffected. Flip both planes at once with `lean-ctx yolo` / `lean-ctx secure`
- `permission_inheritance` (enum: off | on, default `off`) — Mirror the host IDE's permission rules onto lean-ctx tools (v1: OpenCode). When on, ctx_shell honors your bash/rm * rules instead of bypassing them.
- `prefer_native_editor` (bool, default `false`) — Disable lean-ctx edit tools (ctx_patch, ctx_refactor) so the host's native editor handles edits (#454)
- `preserve_compact_formats` (string[], default `["toon"]`) — Already-compact output formats preserved verbatim instead of recompressed (e.g. ["toon"]). Set to [] to disable
- `project_root` (string?, default `null`) — Explicit project root directory. Prevents accidental home-directory scans
- `proxy_enabled` (bool?, default `null`) — Enable/disable the proxy layer. null = auto-detect, true = force on, false = force off
- `proxy_loopback_open` (bool, default `false`) — Skip ALL proxy authentication on loopback binds. MCP/HTTP clients work without tokens.
- `proxy_port` (u16?, default `null`) — Custom proxy port (default: 4444). Useful for multi-user systems.
- `proxy_require_token` (bool, default `false`) — Require lean-ctx Bearer token authentication and disable provider API key fallback
- `proxy_timeout_ms` (u64?, default `null`) — Proxy reachability timeout in ms (default: 200).
- `read_dedup` (enum: auto | on | off, default `auto`) — Controls the PostToolUse native-Read re-read dedup. auto (default): replace only re-reads of unchanged files with the compact stub, and only on guard hosts (Claude Code / CodeBuddy) where the PreToolUse redirect is off — first reads stay byte-identical and the read-before-write guard is untouched. on: dedup wherever the hook fires. off: never replace a Read result
- `read_only_roots` (string[], default `[]`) — Read-only sibling roots: reads allowed, writes always denied (edit/refactor/export)
- `read_redirect` (enum: auto | on | off, default `auto`) — Controls the native-Read → ctx_read redirect hook. auto (default): redirect everywhere except hosts with a native read-before-write guard (Claude Code / CodeBuddy), where rewriting Read to a temp copy breaks native Write/Edit (#637). on: always redirect. off: never redirect native Read (ctx_read MCP tool + Grep/Glob redirect stay active)
- `recovery_hints` (enum: off | minimal | full, default `minimal`) — Verbosity of the reactive recovery footer on compressed output (path-first, MCP-optional)
- `reference_results` (bool, default `false`) — Store large tool outputs as references instead of inline content
- `response_verbosity` (enum: normal | compact | minimal, default `normal`) — Controls how verbose tool responses are
- `rules_injection` (enum: shared | dedicated | off, default `shared`) — How rules load for CLAUDE.md/AGENTS.md/GEMINI.md agents: shared block, dedicated (no shared-file edits; SessionStart hook / instructions[] / context.fileName), or off (write no rules file — for hosts that supply their own steering or phase-isolated/non-caching harnesses).
- `rules_scope` (enum: both | global | project, default `both`) — Where agent rule files are installed.
- `sandbox_level` (u8, default `0`) — Sandbox strictness level (0=default, 1=strict, 2=paranoid)
- `savings_footer` (enum: auto | always | never, default `always`) — Controls visibility of token savings footers: always (default, show on every response), never, auto (context-dependent).
- `shadow_mode` (bool, default `true`) — Default on: denies native tools at the permission level, forcing agents to use ctx_* MCP tools for maximum compression. Disable with shadow_mode = false if you prefer native tools.
- `shell_activation` (enum: always | agents-only | off, default `agents-only`) — Controls when the shell hook auto-activates aliases (agents-only since #699: transparent in plain human terminals)
- `shell_heavy_timeout_secs` (u64?, default `null`) — Shell command timeout (seconds) for heavy commands (cargo build/test, make, docker build, git commit/push). null = built-in 10-minute ceiling
- `shell_hook_disabled` (bool, default `false`) — Disable shell hook injection
- `shell_security` (string, default `enforce`) — Shell command gating: enforce (default, secure), warn (log only, never block) or off (skip the hard blocks; compression stays active)
- `shell_timeout_secs` (u64?, default `null`) — Shell command timeout (seconds) for normal commands. null = built-in 2-minute default
- `skip_agent_aliases` (bool, default `false`) — Do not install agent CLI aliases (claude, codex, gemini) into shell rc files. Existing alias blocks are removed on next setup
- `slow_command_threshold_ms` (u64, default `5000`) — Commands taking longer than this (ms) are recorded in the slow log. Set to 0 to disable
- `symbol_map_auto` (bool, default `false`) — Opt-in: α-code identifier substitution in aggressive reads (>50-file projects). Off by default — abbreviated symbols hinder editing/refactoring
- `tee_mode` (enum: never | failures | highcompression | always, default `highcompression`) — Controls when shell output is tee'd to disk for later retrieval
- `terse_agent` (enum: off | lite | full | ultra, default `off`) — Controls agent output verbosity via instructions injection
- `theme` (string, default `default`) — Dashboard color theme

## `[archive]`

Settings for the zero-loss compression archive (large tool outputs saved to disk)

- `enabled` (bool, default `true`) — Enable zero-loss compression archive
- `ephemeral` (bool, default `true`) — Replace large results with summary+ref (ctx_expand to retrieve)
- `ephemeral_min_tokens` (usize, default `2000`) — Minimum output tokens before the ephemeral firewall replaces inline body with summary+ref
- `inline_max_bytes` (usize, default `32768`) — Maximum ctx_shell(inline=true) output size in bytes before archive/firewall handling
- `max_age_hours` (u64, default `48`) — Maximum age of archived entries before cleanup
- `max_disk_mb` (u64, default `500`) — Maximum total disk usage for the archive
- `threshold_chars` (usize, default `800`) — Minimum output size (chars) to trigger archiving

## `[autonomy]`

Controls autonomous background behaviors (preload, dedup, consolidation)

- `auto_consolidate` (bool, default `true`) — Auto-consolidate knowledge periodically
- `auto_dedup` (bool, default `true`) — Auto-deduplicate repeated reads
- `auto_preload` (bool, default `true`) — Auto-preload related files on first read
- `auto_related` (bool, default `true`) — Auto-load graph-related files
- `cognition_loop_enabled` (bool, default `true`) — Enable the background cognition loop (periodic knowledge consolidation)
- `cognition_loop_interval_secs` (u64, default `3600`) — Seconds between cognition loop iterations
- `cognition_loop_max_steps` (u8, default `9`) — Maximum steps per cognition loop iteration (>= 9 enables observation synthesis)
- `cognition_synthesis_min_cluster` (usize, default `3`) — Minimum facts per entity before observation synthesis writes a summary (needs cognition_loop_max_steps >= 9)
- `consolidate_cooldown_secs` (u64, default `120`) — Minimum seconds between consolidation runs
- `consolidate_every_calls` (u32, default `25`) — Consolidate knowledge every N tool calls
- `dedup_threshold` (usize, default `8`) — Number of repeated reads before dedup triggers
- `enabled` (bool, default `true`) — Enable autonomous background behaviors
- `silent_preload` (bool, default `true`) — Suppress preload notifications in output

## `[boundary_policy]`

Cross-project knowledge sharing

- `universal_gotchas_enabled` (bool, default `true`) — Load universal (cross-project) gotchas

## `[context]`

Fixed-context budget accounting (#964)

- `budget_tokens` (usize, default `8000`) — Fixed per-session context budget (tool schemas + MCP instructions + auto-loaded rules + wakeup briefing). `doctor overhead` warns past this; `doctor overhead --gate` exits non-zero for CI. 0 disables the warning
- `proactive_expansion` (bool, default `true`) — Inject relevant CCR archives into later tool responses
- `proactive_expansion_budget_tokens` (usize, default `2000`) — Maximum proactive archive tokens per tool response
- `proactive_expansion_max_age_secs` (u64, default `3600`) — Maximum age of CCR content eligible for proactive expansion
- `proactive_expansion_threshold` (f64, default `0.6`) — Minimum normalized BM25 score for proactive expansion

## `[cost]`

Model declaration for measured-vs-estimated cost reporting

- `default_model` (string?, default `null`) — Fallback pricing model for MCP-only IDEs whose real model lean-ctx cannot observe (Cursor, Copilot, Windsurf, …). Unset → blended heuristic. Per-IDE overrides live in [cost.models]
- `prices` (table?, default `[]`) — Operator price overrides per model, USD per million tokens: [cost.prices."<model>"] with input_per_m / output_per_m / cache_write_per_m / cache_read_per_m. For negotiated enterprise rates (committed-use discounts, Azure PTU, zero-rated internal models); overrides embedded and live catalog rows, only a provider-measured bill beats it

## `[embedding]`

Semantic-embedding engine settings (model selection for ctx_semantic_search)

- `auto_download` (bool, default `null`) — Download the embedding model in the background on first semantic need (default: allowed). Set false for air-gapped machines; semantic features then stay off until a model is provided manually.
- `deterministic` (bool, default `null`) — Pin embedding inference to a single CPU thread with no GPU provider so vectors are bit-identical across machines (default: off, multi-threaded GPU-capable path). Extractive prose ranking is already deterministic via score quantization; enable this only for cross-machine reproducibility, at a throughput cost.
- `dimensions` (integer, default `null`) — Declared embedding width for hf: custom models (fallback only — the real width is probed from the ONNX graph at load time). Built-in models ignore this key.
- `model` (string, default `minilm`) — Local ONNX embedding model for ctx_semantic_search. One of: minilm (all-MiniLM-L6-v2, 384d, default), nomic (768d) — or any HuggingFace repo with an ONNX export via hf:org/repo[@revision] (e.g. hf:jinaai/jina-embeddings-v2-base-code for code). Switching models re-indexes once on the next search.

## `[gain]`

Token-savings recap publishing (gain --publish / auto-publish)

- `auto_publish` (bool, default `false`) — Automatically (re)publish your Wrapped recap when you run `lean-ctx gain` (opt-in, off by default; throttled and sends only an aggregate payload)
- `auto_publish_interval_hours` (u64, default `24`) — Minimum hours between automatic publishes (throttle; default 24)
- `display_name` (string?, default `null`) — Optional display name shown on your published card / leaderboard entry
- `last_auto_publish` (string?, default `null`) — Timestamp of the last automatic publish (written by lean-ctx for throttling — not meant to be edited)
- `leaderboard` (bool, default `true`) — When auto-publishing, also list the card on the public opt-in leaderboard

## `[graph]`

Code-graph settings, including traversal (co-access) edges learned from sessions

- `traversal_edges` (bool, default `true`) — Learn co-access edges from real sessions (files surfaced together), surface them as decaying `co_access` graph edges, and boost recall by them. Set false for a purely static AST-only graph.

## `[ide_paths]`

Per-IDE allowed paths. Keys are agent names (cursor, codex, opencode, antigravity, etc.), values are arrays of paths to index for that agent

_No sub-keys (presence of the section toggles the feature)._

## `[index]`

Index-time file filters: declare the retrieval corpus explicitly (BM25 + graph + semantic + watch share one filter layer, #735)

- `exclude` (string[], default `[]`) — Globs dropped from the index corpus (root-relative, forward slashes), e.g. ["**/*.csv", "fixtures/**"]. Wins over include. CLI --exclude appends per run. Excluded files produce no chunks, graph nodes, or embeddings.
- `include` (string[], default `[]`) — When non-empty, ONLY matching files enter the index corpus, e.g. ["**/*.rs", "**/*.ts"]. Empty = no restriction. CLI --include replaces this set per run.
- `respect_gitignore` (bool, default `true`) — Honor .gitignore / global gitignore / .git/info/exclude during index walks. false indexes ignored files too (the vendor-directory guard still applies). CLI override: --no-gitignore / --respect-gitignore.

## `[loop_detection]`

Loop detection settings for preventing repeated identical tool calls

- `blocked_threshold` (u32, default `0`) — Repetitions before blocking. 0 = disabled
- `normal_threshold` (u32, default `2`) — Repetitions before reducing output
- `reduced_threshold` (u32, default `4`) — Repetitions before further reducing output
- `search_group_limit` (u32, default `10`) — Maximum unique searches within a loop window
- `tool_total_limits` (table, default `{"ctx_read":100,"ctx_search":80,"ctx_semantic_search":60,"ctx_shell":50}`) — Per-tool total call limits within a session. Keys are tool names, values are max calls
- `window_secs` (u64, default `300`) — Time window in seconds for loop detection

## `[lsp]`

LSP server binary overrides. Map language name to custom binary path

- `go` (string?, default `null`) — Custom path to gopls binary
- `python` (string?, default `null`) — Custom path to pylsp binary
- `rust` (string?, default `null`) — Custom path to rust-analyzer binary
- `typescript` (string?, default `null`) — Custom path to typescript-language-server binary

## `[memory.embeddings]`

Embeddings memory settings for semantic search

- `max_facts` (usize, default `2000`) — Maximum number of embedding facts stored

## `[memory.episodic]`

Episodic memory budgets (session episodes)

- `max_actions_per_episode` (usize, default `50`) — Maximum actions tracked per episode
- `max_episodes` (usize, default `500`) — Maximum number of episodes retained
- `summary_max_chars` (usize, default `200`) — Maximum characters in episode summary

## `[memory.gotcha]`

Gotcha memory settings (project-specific warnings and pitfalls)

- `default_decay_rate` (f32, default `0.03`) — Default decay rate for gotcha importance
- `max_gotchas_per_project` (usize, default `100`) — Maximum gotchas stored per project
- `retrieval_budget_per_room` (usize, default `10`) — Maximum gotchas retrieved per room per query

## `[memory.knowledge]`

Knowledge memory budgets (facts, patterns, gotchas)

- `contradiction_threshold` (f32, default `0.5`) — Confidence threshold for contradiction detection
- `max_facts` (usize, default `200`) — Maximum number of knowledge facts stored per project
- `max_history` (usize, default `100`) — Maximum history entries retained
- `max_patterns` (usize, default `50`) — Maximum number of patterns stored
- `recall_facts_limit` (usize, default `10`) — Maximum facts returned per recall query
- `relations_limit` (usize, default `40`) — Maximum number of relations returned
- `rooms_limit` (usize, default `25`) — Maximum number of rooms returned
- `timeline_limit` (usize, default `25`) — Maximum number of timeline entries returned

## `[memory.lifecycle]`

Knowledge lifecycle policy (decay, staleness, dedup)

- `archetype_aware_decay` (bool, default `false`) — Scale Ebbinghaus stability by fact archetype so structural evidence decays slower than inference (default false)
- `base_stability_days` (f32, default `90.0`) — Characteristic memory stability (days) for the Ebbinghaus curve
- `decay_rate` (f32, default `0.01`) — Rate at which knowledge confidence decays over time
- `forgetting_model` (string, default `ebbinghaus`) — Forgetting curve: ebbinghaus (default, exponential + spacing) or linear
- `low_confidence_threshold` (f32, default `0.3`) — Threshold below which facts are considered low-confidence
- `reclaim_enabled` (bool, default `true`) — Master switch for the proactive capacity reclaim (#995). false trims only the overflow (escape hatch, no headroom); eviction stays lossless either way
- `reclaim_headroom_pct` (f32, default `0.25`) — Proactive headroom on a capacity reclaim: settle a full store at 1 - this fraction (0.25 = 75%) instead of churning at the cap. Lossless — the reclaimed tail is archived and restorable
- `similarity_threshold` (f32, default `0.85`) — Similarity threshold for deduplication
- `stale_days` (i64, default `30`) — Days after which unused facts are considered stale

## `[memory.procedural]`

Procedural memory budgets (learned patterns)

- `max_procedures` (usize, default `100`) — Maximum number of learned procedures stored
- `max_window_size` (usize, default `10`) — Maximum window size for pattern analysis
- `min_repetitions` (usize, default `3`) — Minimum repetitions before a pattern is stored
- `min_sequence_len` (usize, default `2`) — Minimum sequence length for procedure detection

## `[model_context_windows]`

Per-model context-window overrides in tokens. Keys are model names (case-insensitive), values override every registry layer — use for models the bundled/local registry does not know yet. Bracketed window markers in the model name itself (e.g. `claude-opus-4-8[1m]`) are parsed automatically and need no entry here. Example: `[model_context_windows]\n"my-custom-model" = 500000`

_No sub-keys (presence of the section toggles the feature)._

## `[proxy]`

Proxy upstream configuration for API routing

- `allow_custom_upstream` (bool, default `false`) — Allow a custom (non-allowlisted) HTTPS upstream host, e.g. a corporate gateway in front of the provider API. Opt-in; default false. Unlike the env var, this config flag reaches the managed (service-spawned) proxy started by `proxy enable`/`restart` (#590)
- `allow_insecure_http_upstream` (bool, default `false`) — Allow a non-loopback plaintext http:// upstream (trusted local network only, e.g. http://host.docker.internal:2455 in front of codex-lb). Opt-in; default false
- `anthropic_upstream` (string?, default `null`) — Custom upstream URL for Anthropic API proxy
- `cache_align_relocate` (bool, default `false`) — Opt-in active cache-aligner relocate (#974). When on, the proxy rewrites an unanchored Anthropic system prompt into a stable block (volatile values - ISO dates/datetimes, UUIDs, git SHAs - replaced by constant placeholders) carrying the cache_control breakpoint, plus an uncached trailing block that re-states the relocated values. The cacheable prefix then stays byte-stable turn-to-turn and finally caches; only the small tail is reprocessed. Anthropic-only, Treatment-arm, gated on a client that anchored nothing and on Anthropic's minimum cacheable size. Deterministic (#498) and idempotent. The cache_aligner telemetry is the precursor that quantifies the saving. Default false
- `cache_aligner` (bool, default `true`) — Cache-aligner volatile-field telemetry (#940), on by default. The proxy scans each unanchored Anthropic system prompt for volatile, cache-busting fields (ISO dates/datetimes, UUIDs, git SHAs) and reports how many it found on /status cache_safety (volatile_system_requests, volatile_fields_detected) - purely to quantify how much prompt-cache the client leaks. Measurement only: the request body is never mutated, so it is strictly cache-safe, which is why it ships on for every proxy (#986 premium defaults). The deterministic scan is the precursor to the opt-in tail-relocate below. Set false to opt out of the per-request scan. Default true
- `cache_breakpoint` (bool, default `false`) — Opt-in active prompt-cache breakpoint injection for Anthropic (#939). When on and the client set no cache_control of its own, the proxy adds one cache_control: {type:ephemeral} marker to the system field so an otherwise-uncached, stable system prompt bills later turns at the cached rate (the win a raw API client leaves on the table). Anthropic-only: OpenAI/Gemini cache prefixes automatically and ignore the marker, so those paths stay byte-unchanged. Deterministic, never adds a second breakpoint, and skipped below Anthropic's minimum cacheable size. Default false
- `cache_policy` (bool, default `true`) — Cache-economics (#986), on by default. Enables prompt-cache miss attribution telemetry (per turn, classify the outcome as cold start / warm reuse / TTL lapse / prefix change and report cumulative gauges on /status cache_attribution) plus a net-cost gate on the cold-prefix repack that skips re-seeding prefixes too small to be cached (below Anthropic's ~1024-token minimum). The telemetry never mutates the body and the gate only makes repacking more conservative, so it can never bust a cache that would otherwise have been kept - both halves are strictly safe, so every proxy gets them out of the box (#986 premium defaults). Set false to opt out (drops the /status attribution gauges and the per-request prefix hash). Default true
- `ccr_inband` (bool, default `false`) — Opt-in in-band CCR retrieval for a remote proxy with no shared filesystem (#493). When on, a lossy stub advertises a compact <lc_expand:HASH> marker instead of a local tee path; when the model echoes that marker, the proxy splices the verbatim original (from its local tee store) back inline next turn — one turn of latency, no MCP/filesystem on the agent host. The splice is a strict no-op on marker-less turns, so it never perturbs the provider cache prefix unless the model asked to expand. Default false
- `cold_prefix_repack` (bool, default `false`) — Opt-in big-gap cold-prefix repack (#480): on a session-resume request the proxy may predict (from idle time vs the provider cache TTL) that the client-cached prefix has already expired, then prune that now-cold prefix to re-seed a leaner cache and keep applying the same deterministic compression on later turns so warm follow-ups hit it (sticky; baselines persist across restarts, #499). A wrong guess re-bills cache reads as writes (~12x), so default false
- `compress_protect` (string[], default `[]`) — File-path globs whose reads are never compressed (#1150): a matching path is returned verbatim (full) by the read tools, for files where exact bytes matter more than token savings (golden snapshots, byte-asserted fixtures, security-sensitive configs). Globs (*/**/?) match the path and its file name, so *.snap, **/golden/**, tests/fixtures/* all work. Empty (default) protects nothing — the lossless crushers and beneficial gate already keep compression safe; this is an explicit escape hatch
- `cost_response_header` (string, default `null`) — Extra response header carrying the upstream gateway's billed USD for the turn (e.g. a corporate gateway's cost header). LiteLLM's x-litellm-response-cost is always recognized. Measured header costs beat table estimates; body-reported costs (OpenRouter usage.cost) beat headers
- `counterfactual_metering` (bool, default `false`) — Opt-in counterfactual savings metering (#701): each rewritten Anthropic /v1/messages request fires a free count_tokens probe with the original, uncompressed body, concurrently with the real forward. The provider-counted answer is paired with the same response's billed usage — provider-authoritative savings receipts ('would have cost N, billed M') instead of local tokenizer estimates, shown as verified_savings on /status. The probe never mutates or delays the forwarded request; failures degrade to the estimate. Default false (one extra free HTTP call per compressed request)
- `effort` (enum: off | minimal | low | medium | high, default `off`) — Cache-safe cross-provider reasoning-effort control (#834). off (default) = no-op. minimal|low|medium|high pins the model's reasoning depth across providers: lean-ctx translates it to OpenAI reasoning_effort / reasoning.effort, Anthropic output_config.effort, and Gemini thinkingConfig (thinkingLevel on 3.x, thinkingBudget on 2.5 pro/flash), only on models that accept it and only when the client didn't set its own value. The level is a constant, so it never breaks the provider prompt cache (unlike per-turn effort routing). Anthropic is dialed only when the client already requested adaptive thinking
- `gemini_upstream` (string?, default `null`) — Custom upstream URL for Gemini API proxy
- `history_mode` (enum: cache-aware | rolling | off, default `cache-aware`) — History pruning strategy. cache-aware: frozen boundaries that keep provider prompt caches valid (default). rolling: legacy moving window (max raw savings, breaks prompt caching). off: never prune
- `live_compress` (bool, default `true`) — Live-compress non-protected tool_result content on the wire (#481). Default true. Set false for a meter-only proxy — real billed/cache token metering with zero request rewriting (combine with history_mode = "off" and no role_aggressiveness for a byte-unchanged body)
- `live_compress_exclude` (string[], default `["serena"]`) — Tool-name patterns (case-insensitive substring) whose tool_result is never live-compressed — treated as protected, like a file read (#481). Unset protects Serena's code-reading tools; set an explicit list to narrow it, or [] to disable
- `meter_openai_usage` (bool, default `true`) — Inject stream_options.include_usage into streamed OpenAI Chat Completions so the final chunk reports real token usage for the measured spend meter. Default true
- `openai_upstream` (string?, default `null`) — Custom upstream URL for OpenAI API proxy
- `output_holdout` (f64, default `0.0`) — Fraction 0.0-1.0 of conversations placed in the output-savings control arm (#895). 0 (default) = no holdout (every conversation is output-shaped). When > 0, a deterministic cohort = blake3(system + first user message) puts ~this fraction in a control arm that skips output-shaping (effort control + verbosity steer) but is still metered, yielding an honest measured output-token reduction (lean-ctx output-savings). The cohort is a pure function of conversation identity, so a conversation keeps one arm across all turns - cache-safe
- `prose_ranker` (enum: auto | extractive | truncate, default `auto`) — How the proxy squeezes prose it must shrink (#895). auto (default) and extractive use embedding-based extractive ranking — keeping the most central sentences instead of just the prefix — when the local embedding engine is available, else fall back to truncation; truncate keeps the original deterministic FIFO squeeze and never loads the engine. Wire rewrites are memoized per content so the engine's cold→warm transition never changes an already-emitted frozen-region rewrite (cache-safe, #448/#498)
- `verbosity_steer` (bool, default `false`) — Opt-in cache-safe wire verbosity steer (#895). When true, the proxy appends a single constant 'be concise' instruction to the last user turn of each request - output-shaping for raw API clients that do not load lean-ctx rules. The suffix is constant and appended strictly after the last cache_control breakpoint (a new trailing text block, never modifying a cache-anchored block), so the provider prompt-cache prefix stays byte-stable. Under an output_holdout the control arm skips it so its effect is measured. Default false

## `[proxy.role_aggressiveness]`

Opt-in per-role prose compression for the proxy's frozen request region (#710). Assistant turns are always passed through verbatim

- `system` (f64, default `null`) — Opt-in prose compression intensity (0.0–1.0) for system prompts in the proxy's frozen request region. Unset = leave untouched. Higher = more aggressive. Cache-safe (deterministic, never touches the client-cached prefix)
- `user` (f64, default `null`) — Opt-in prose compression intensity (0.0–1.0) for free-text user turns (never tool results) in the proxy's frozen request region. Unset = leave untouched

## `[search]`

Hybrid search weights for ctx_semantic_search (BM25 + dense vector + SPLADE + graph proximity)

- `bm25_candidates` (usize, default `75`) — Number of BM25 candidates to retrieve before fusion
- `bm25_weight` (f64, default `1.0`) — BM25 lexical search weight in RRF fusion
- `dense_candidates` (usize, default `75`) — Number of dense candidates to retrieve before fusion
- `dense_enabled` (bool, default `true`) — Enable the dense (embedding) retrieval path. false → hybrid search ranks with BM25 + graph + rerank (+ SPLADE) only, skipping the embedding engine and the persistent embeddings.json (lighter footprint, no embed latency). An explicit mode=dense query still forces dense.
- `dense_weight` (f64, default `1.0`) — Dense vector search weight in RRF fusion
- `splade_weight` (f64, default `0.5`) — SPLADE expansion weight (0.0 to disable)

## `[secret_detection]`

Secret/credential detection and redaction settings

- `custom_patterns` (array, default `[]`) — Additional regex patterns to detect as secrets
- `enabled` (bool, default `true`) — Enable secret/credential detection in tool outputs
- `exclude_patterns` (array, default `[]`) — Subtractive allowlist: matches covered by these regexes are never reported or redacted (#718)
- `redact` (bool, default `true`) — Redact detected secrets from output

## `[sensitivity]`

Per-item sensitivity model with a uniform policy floor (#212)

- `action` (string, default `redact`) — How to enforce the floor: redact (mask spans) or drop (withhold item)
- `enabled` (bool, default `false`) — Enable the per-item sensitivity policy floor (no-op when false)
- `policy_floor` (string, default `secret`) — Block items at/above this level: public|internal|confidential|secret

## `[setup]`

Controls what lean-ctx injects during setup and updates. Fresh installs default to non-invasive (rules/skills off, MCP on).

- `auto_inject_rules` (bool?, default `null`) — Inject agent rule files during setup/update. null=auto (inject if already present), true=always, false=never
- `auto_update_mcp` (bool, default `true`) — Register lean-ctx MCP server in editor configs during setup/update

## `[summaries]`

AI session summaries: periodic, semantically-recallable session digests

- `enabled` (bool, default `true`) — Record periodic, semantically-recallable AI session summaries (what was done, files, decisions).
- `every_n_turns` (u32, default `25`) — Tool calls between automatic session summaries (gated by the auto-checkpoint cadence).
- `max_kept` (u32, default `100`) — Maximum session summaries kept per project (oldest pruned first).

