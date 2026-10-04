//! Core config sections (root, ide_paths, lsp, archive, search, embedding).
//! Split out of `schema/mod.rs`; `use super::*` re-imports the key-builder
//! helpers and `SectionSchema`.

#[allow(clippy::wildcard_imports)]
use super::*;
use std::collections::BTreeMap;

pub(super) fn build(sections: &mut BTreeMap<String, SectionSchema>) {
    let cfg = crate::core::config::Config::default();
    let mut root = BTreeMap::new();
    root.insert(
        "tee_mode".into(),
        key_enum(
            &["never", "failures", "highcompression", "always"],
            "highcompression",
            "Controls when shell output is tee'd to disk for later retrieval",
        ),
    );
    root.insert(
        "recovery_hints".into(),
        key_enum(
            &["off", "minimal", "full"],
            "minimal",
            "Verbosity of the reactive recovery footer on compressed output (path-first, MCP-optional)",
        ),
    );
    root.insert(
        "output_density".into(),
        key_enum(
            &["normal", "terse", "ultra"],
            "normal",
            "Controls how dense/compact MCP tool output is formatted",
        ),
    );
    root.insert(
        "passthrough_commands".into(),
        key(
            "string[]",
            serde_json::json!(cfg.passthrough_commands),
            "Commands whose output is passed through uncompressed (the command still runs normally)",
        ),
    );
    root.insert(
        "passthrough_urls".into(),
        key(
            "string[]",
            serde_json::json!(cfg.passthrough_urls),
            "URLs to pass through without proxy interception",
        ),
    );
    root.insert(
            "preserve_compact_formats".into(),
            key(
                "string[]",
                serde_json::json!(cfg.preserve_compact_formats),
                "Already-compact output formats preserved verbatim instead of recompressed (e.g. [\"toon\"]). Set to [] to disable",
            ),
        );
    root.insert(
        "crush_verbatim_json".into(),
        key(
            "bool",
            serde_json::json!(cfg.crush_verbatim_json),
            "Opt-in: losslessly crush array-heavy JSON from verbatim data commands (gh api, jq, kubectl get -o json, curl). Off by default keeps them verbatim. Reshapes only when it at least halves the payload; fully reconstructible",
        ),
    );
    root.insert("slow_command_threshold_ms".into(), key("u64", serde_json::json!(cfg.slow_command_threshold_ms), "Commands taking longer than this (ms) are recorded in the slow log. Set to 0 to disable"));
    root.insert(
        "theme".into(),
        key(
            "string",
            serde_json::json!(cfg.theme),
            "Dashboard color theme",
        ),
    );
    root.insert(
            "enable_wakeup_ctx".into(),
            key(
                "bool",
                serde_json::json!(cfg.enable_wakeup_ctx),
                "Append wakeup briefing (facts, session summary) to ctx_overview output. Set false to reduce context bloat when calling ctx_overview frequently.",
            ),
        );
    root.insert(
        "prefer_native_editor".into(),
        key(
            "bool",
            serde_json::json!(cfg.prefer_native_editor),
            "Disable lean-ctx edit tools (ctx_patch, ctx_refactor) so the host's native editor handles edits (#454)",
        ),
    );
    root.insert(
        "no_degrade".into(),
        key(
            "boolean",
            serde_json::json!(cfg.no_degrade),
            "Disable all automatic read-mode degradation.",
        ),
    );
    root.insert(
        "delta_explicit".into(),
        key(
            "boolean",
            serde_json::json!(cfg.delta_explicit),
            "Serve explicit full/lines re-reads of changed cached files as diffs (opt-in).",
        ),
    );
    root.insert(
        "rules_scope".into(),
        key_enum(
            &["both", "global", "project"],
            "both",
            "Where agent rule files are installed.",
        ),
    );
    root.insert(
        "rules_injection".into(),
        key_enum(
            &["shared", "dedicated", "off"],
            "shared",
            "How rules load for CLAUDE.md/AGENTS.md/GEMINI.md agents: shared block, \
             dedicated (no shared-file edits; SessionStart hook / instructions[] / \
             context.fileName), or off (write no rules file — for hosts that supply \
             their own steering or phase-isolated/non-caching harnesses).",
        ),
    );
    root.insert(
        "permission_inheritance".into(),
        key_enum(
            &["off", "on"],
            "off",
            "Mirror the host IDE's permission rules onto lean-ctx tools (v1: \
             OpenCode). When on, ctx_shell honors your bash/rm * rules instead of \
             bypassing them.",
        ),
    );
    root.insert(
        "extra_ignore_patterns".into(),
        key(
            "string[]",
            serde_json::json!(cfg.extra_ignore_patterns),
            "Extra glob patterns to ignore in graph/overview/preload",
        ),
    );
    root.insert(
        "terse_agent".into(),
        key_enum(
            &["off", "lite", "full", "ultra"],
            "off",
            "Controls agent output verbosity via instructions injection",
        ),
    );
    root.insert(
            "compression_level".into(),
            key_enum(
            &["off", "lite", "standard", "max"],
            "lite",
            "Unified output-style level for the model's prose (not tool-output compression). lite=plain concise (default), standard/max=denser symbolic 'power modes'",
        ),
        );
    root.insert(
        "compression_aggressiveness".into(),
        key(
            "f64",
            serde_json::json!(cfg.compression_aggressiveness),
            "Global compression intensity 0.0 (lossless) – 1.0 (max), mapped onto read modes/entropy/IB. Empty = per-mode defaults",
        ),
    );
    root.insert(
        "allow_paths".into(),
        key(
            "string[]",
            serde_json::json!(cfg.allow_paths),
            "Additional paths allowed by PathJail (absolute)",
        ),
    );
    root.insert(
        "allow_ide_config_dirs".into(),
        key(
            "bool",
            serde_json::json!(cfg.allow_ide_config_dirs),
            "Allow jailed ctx_* tools to read home-level IDE config dirs (registry-derived; covers all editors). Off by default — exposes other agents' sessions/credentials",
        ),
    );
    root.insert(
        "extra_roots".into(),
        key(
            "string[]",
            serde_json::json!(cfg.extra_roots),
            "Extra project roots for multi-root workspaces (auto-added to PathJail allow-list)",
        ),
    );
    root.insert(
        "read_only_roots".into(),
        key(
            "string[]",
            serde_json::json!(cfg.read_only_roots),
            "Read-only sibling roots: reads allowed, writes always denied (edit/refactor/export)",
        ),
    );
    root.insert(
        "allow_symlink_roots".into(),
        key(
            "string[]",
            serde_json::json!(cfg.allow_symlink_roots),
            "Trusted roots OUTSIDE $HOME lean-ctx may follow when an agent config is symlinked there (#596). Empty = strict $HOME-only",
        ),
    );
    root.insert(
        "content_defined_chunking".into(),
        key(
            "bool",
            serde_json::json!(false),
            "Enable Rabin-Karp chunking for cache-optimal output ordering",
        ),
    );
    root.insert(
        "minimal_overhead".into(),
        key(
            "bool",
            serde_json::json!(true),
            "Skip session/knowledge/gotcha blocks in MCP instructions",
        ),
    );
    root.insert(
        "symbol_map_auto".into(),
        key(
            "bool",
            serde_json::json!(false),
            "Opt-in: α-code identifier substitution in aggressive reads (>50-file projects). Off by default — abbreviated symbols hinder editing/refactoring",
        ),
    );
    root.insert(
        "auto_mode_learning".into(),
        key(
            "bool",
            serde_json::json!(false),
            "Opt-in: let adaptive learning signals (predictor, bandit, heatmap, adaptive policy, bounce/path memory) influence `auto` mode. Off by default for a deterministic, I/O-light cascade (capability guards + size/task heuristic only) that keeps output byte-stable for prompt caching.",
        ),
    );
    root.insert(
        "auto_capture".into(),
        key(
            "bool",
            serde_json::json!(true),
            "Automatic knowledge capture from tool findings",
        ),
    );
    root.insert(
            "cache_policy".into(),
            key(
            "enum(aggressive|safe|off)",
            serde_json::json!("aggressive"),
            "Cache policy for ctx_read: aggressive (13-tok stubs), safe (map on hit), off (always disk)",
        ),
        );
    root.insert(
        "bypass_hints".into(),
        key_enum(
            &["on", "off", "aggressive"],
            "on",
            "Bypass-hint mode: when agents use native Read/Grep instead of lean-ctx \
             tools, a hint is appended to the next tool response. on (default), off, \
             aggressive (hint on every call, no cooldown).",
        ),
    );
    root.insert(
        "cache_max_tokens".into(),
        key(
            "usize",
            serde_json::json!(cfg.cache_max_tokens),
            "Token budget for the in-memory ctx_read cache (0 = built-in default 500k). When exceeded, least-valuable entries are evicted immediately via RRF (recency x frequency x size) so reads never block; eviction is not deferred to the staleness TTL",
        ),
    );
    root.insert(
            "shadow_mode".into(),
            key(
            "bool",
            serde_json::json!(true),
            "Default on: denies native tools at the permission level, forcing agents to use ctx_* MCP tools for maximum compression. Disable with shadow_mode = false if you prefer native tools.",
        ),
        );
    root.insert(
        "read_redirect".into(),
        key_enum(
            &["auto", "on", "off"],
            "auto",
            "Controls the native-Read → ctx_read redirect hook. auto (default): redirect everywhere except hosts with a native read-before-write guard (Claude Code / CodeBuddy), where rewriting Read to a temp copy breaks native Write/Edit (#637). on: always redirect. off: never redirect native Read (ctx_read MCP tool + Grep/Glob redirect stay active)",
        ),
    );
    root.insert(
        "read_dedup".into(),
        key_enum(
            &["auto", "on", "off"],
            "auto",
            "Controls the PostToolUse native-Read re-read dedup. auto (default): replace only re-reads of unchanged files with the compact stub, and only on guard hosts (Claude Code / CodeBuddy) where the PreToolUse redirect is off — first reads stay byte-identical and the read-before-write guard is untouched. on: dedup wherever the hook fires. off: never replace a Read result",
        ),
    );
    root.insert(
        "debug_log".into(),
        key(
            "bool",
            serde_json::json!(false),
            "Opt-in (default off): write a human-readable debug log of intercepted MCP tool calls and hook routing decisions (lean-ctx vs native, with the reason) to <state_dir>/logs/debug.log. View with `lean-ctx debug-log`",
        ),
    );
    root.insert(
        "shell_hook_disabled".into(),
        key(
            "bool",
            serde_json::json!(false),
            "Disable shell hook injection",
        ),
    );
    root.insert(
        "shell_activation".into(),
        key_enum(
            &["always", "agents-only", "off"],
            "agents-only",
            "Controls when the shell hook auto-activates aliases (agents-only since #699: transparent in plain human terminals)",
        ),
    );
    root.insert(
        "skip_agent_aliases".into(),
        key(
            "bool",
            serde_json::json!(cfg.skip_agent_aliases),
            "Do not install agent CLI aliases (claude, codex, gemini) into shell rc files. Existing alias blocks are removed on next setup",
        ),
    );
    root.insert(
        "bm25_max_cache_mb".into(),
        key(
            "u64",
            serde_json::json!(cfg.bm25_max_cache_mb),
            "Maximum BM25 cache file size in MB",
        ),
    );
    root.insert(
            "graph_index_max_files".into(),
            key(
                "u64",
                serde_json::json!(cfg.graph_index_max_files),
                "Maximum files in graph index. 0 = unlimited (default). Set >0 to cap for constrained systems",
            ),
        );
    root.insert(
        "memory_profile".into(),
        key_enum(
            &["low", "balanced", "performance"],
            "performance",
            "Controls RAM vs feature trade-off (performance = max quality)",
        ),
    );
    root.insert(
        "memory_cleanup".into(),
        key_enum(
            &["aggressive", "shared"],
            "aggressive",
            "Controls how aggressively memory is freed when idle",
        ),
    );
    root.insert(
            "savings_footer".into(),
            key_enum(
            &["auto", "always", "never"],
            "always",
            "Controls visibility of token savings footers: always (default, show on every response), never, auto (context-dependent). ",
        ),
        );
    root.insert(
        "max_ram_percent".into(),
        key(
            "u8",
            serde_json::json!(cfg.max_ram_percent),
            "Soft process-RSS target as % of system RAM (1-50, default 5); eviction/throttling policy, not an OS hard cap",
        ),
    );
    root.insert(
        "max_disk_mb".into(),
        key(
            "u64",
            serde_json::json!(cfg.max_disk_mb),
            "Simplified disk budget in MB (0 = disabled). Distributes: archive ~25%, BM25 ~10%",
        ),
    );
    root.insert(
        "max_index_threads".into(),
        key(
            "usize",
            serde_json::json!(cfg.max_index_threads),
            "Cap rayon threads for the CPU-heavy index build (0 = all cores). Bounds per-instance CPU so concurrent sessions don't saturate the host on startup",
        ),
    );
    root.insert(
        "max_staleness_days".into(),
        key(
            "u32",
            serde_json::json!(cfg.max_staleness_days),
            "Auto-purge data older than N days (0 = disabled). Flows into archive.max_age_hours",
        ),
    );
    root.insert(
        "project_root".into(),
        key(
            "string?",
            serde_json::json!(null),
            "Explicit project root directory. Prevents accidental home-directory scans",
        ),
    );
    root.insert(
            "proxy_enabled".into(),
            key(
                "bool?",
                serde_json::json!(null),
                "Enable/disable the proxy layer. null = auto-detect, true = force on, false = force off",
            ),
        );
    root.insert(
        "proxy_port".into(),
        key(
            "u16?",
            serde_json::json!(null),
            "Custom proxy port (default: 4444). Useful for multi-user systems.",
        ),
    );
    root.insert(
        "proxy_timeout_ms".into(),
        key(
            "u64?",
            serde_json::json!(null),
            "Proxy reachability timeout in ms (default: 200).",
        ),
    );
    root.insert(
        "proxy_require_token".into(),
        key(
            "bool",
            serde_json::json!(cfg.proxy_require_token),
            "Require lean-ctx Bearer token authentication and disable provider API key fallback",
        ),
    );
    root.insert(
        "proxy_loopback_open".into(),
        key(
            "bool",
            serde_json::json!(cfg.proxy_loopback_open),
            "Skip ALL proxy authentication on loopback binds. MCP/HTTP clients work without tokens.",
        ),
    );
    root.insert(
        "response_verbosity".into(),
        key_enum(
            &["normal", "compact", "minimal"],
            "normal",
            "Controls how verbose tool responses are",
        ),
    );
    root.insert(
        "allow_auto_reroot".into(),
        key(
            "bool",
            serde_json::json!(false),
            "Allow automatic project-root re-rooting when absolute paths outside the jail are seen",
        ),
    );
    root.insert(
        "hook_binary".into(),
        key(
            "string?",
            serde_json::json!(null),
            "Verbatim binary path/expression for generated agent-hook commands (e.g. $HOME/.local/bin/lean-ctx) — for settings files synced across machines with different usernames. Shell-expanded by the hook host at run time; doctor accepts it as current. Empty = automatic absolute-path resolution",
        ),
    );
    root.insert(
        "path_jail".into(),
        key(
            "bool?",
            serde_json::json!(null),
            "Filesystem path jail. null/true = enforced (tools confined to the project root + allow_paths). false = the blanket \"any path\" opt-out — every tool path is allowed (for containers/sandboxes where the boundary is external). Compression and secret redaction are unaffected. Flip both planes at once with `lean-ctx yolo` / `lean-ctx secure`",
        ),
    );
    root.insert(
        "sandbox_level".into(),
        key(
            "u8",
            serde_json::json!(0),
            "Sandbox strictness level (0=default, 1=strict, 2=paranoid)",
        ),
    );
    root.insert(
        "reference_results".into(),
        key(
            "bool",
            serde_json::json!(false),
            "Store large tool outputs as references instead of inline content",
        ),
    );
    root.insert(
        "shell_security".into(),
        key(
            "string",
            serde_json::json!("enforce"),
            "Shell command gating: enforce (default, secure), warn (log only, never block) or off (skip the hard blocks; compression stays active)",
        ),
    );
    root.insert(
        "shell_timeout_secs".into(),
        key(
            "u64?",
            serde_json::json!(null),
            "Shell command timeout (seconds) for normal commands. null = built-in 2-minute default",
        ),
    );
    root.insert(
        "shell_heavy_timeout_secs".into(),
        key(
            "u64?",
            serde_json::json!(null),
            "Shell command timeout (seconds) for heavy commands (cargo build/test, make, docker build, git commit/push). null = built-in 10-minute ceiling",
        ),
    );

    sections.insert(
        "root".into(),
        SectionSchema {
            description: "Top-level configuration keys".into(),
            keys: root,
        },
    );

    sections.insert(
            "ide_paths".into(),
            SectionSchema {
                description: "Per-IDE allowed paths. Keys are agent names (cursor, codex, opencode, antigravity, etc.), values are arrays of paths to index for that agent".into(),
                keys: BTreeMap::new(),
            },
        );

    sections.insert(
            "model_context_windows".into(),
            SectionSchema {
                description: "Per-model context-window overrides in tokens. Keys are model names (case-insensitive), values override every registry layer — use for models the bundled/local registry does not know yet. Bracketed window markers in the model name itself (e.g. `claude-opus-4-8[1m]`) are parsed automatically and need no entry here. Example: `[model_context_windows]\\n\"my-custom-model\" = 500000`".into(),
                keys: BTreeMap::new(),
            },
        );

    let mut lsp_keys = BTreeMap::new();
    lsp_keys.insert(
        "rust".into(),
        key(
            "string?",
            serde_json::json!(null),
            "Custom path to rust-analyzer binary",
        ),
    );
    lsp_keys.insert(
        "typescript".into(),
        key(
            "string?",
            serde_json::json!(null),
            "Custom path to typescript-language-server binary",
        ),
    );
    lsp_keys.insert(
        "python".into(),
        key(
            "string?",
            serde_json::json!(null),
            "Custom path to pylsp binary",
        ),
    );
    lsp_keys.insert(
        "go".into(),
        key(
            "string?",
            serde_json::json!(null),
            "Custom path to gopls binary",
        ),
    );
    sections.insert(
        "lsp".into(),
        SectionSchema {
            description: "LSP server binary overrides. Map language name to custom binary path"
                .into(),
            keys: lsp_keys,
        },
    );

    let mut archive = BTreeMap::new();
    archive.insert(
        "enabled".into(),
        key(
            "bool",
            serde_json::json!(cfg.archive.enabled),
            "Enable zero-loss compression archive",
        ),
    );
    archive.insert(
        "threshold_chars".into(),
        key(
            "usize",
            serde_json::json!(cfg.archive.threshold_chars),
            "Minimum output size (chars) to trigger archiving",
        ),
    );
    archive.insert(
        "max_age_hours".into(),
        key(
            "u64",
            serde_json::json!(cfg.archive.max_age_hours),
            "Maximum age of archived entries before cleanup",
        ),
    );
    archive.insert(
        "max_disk_mb".into(),
        key(
            "u64",
            serde_json::json!(cfg.archive.max_disk_mb),
            "Maximum total disk usage for the archive",
        ),
    );
    archive.insert(
        "ephemeral".into(),
        key(
            "bool",
            serde_json::json!(cfg.archive.ephemeral),
            "Replace large results with summary+ref (ctx_expand to retrieve)",
        ),
    );
    archive.insert(
        "ephemeral_min_tokens".into(),
        key("usize", serde_json::json!(cfg.archive.ephemeral_min_tokens), "Minimum output tokens before the ephemeral firewall replaces inline body with summary+ref"),
    );
    archive.insert(
        "inline_max_bytes".into(),
        key(
            "usize",
            serde_json::json!(cfg.archive.inline_max_bytes),
            "Maximum ctx_shell(inline=true) output size in bytes before archive/firewall handling",
        ),
    );
    sections.insert(
        "archive".into(),
        SectionSchema {
            description:
                "Settings for the zero-loss compression archive (large tool outputs saved to disk)"
                    .into(),
            keys: archive,
        },
    );

    let mut search = BTreeMap::new();
    search.insert(
        "bm25_weight".into(),
        key(
            "f64",
            serde_json::json!(cfg.search.bm25_weight),
            "BM25 lexical search weight in RRF fusion",
        ),
    );
    search.insert(
        "dense_weight".into(),
        key(
            "f64",
            serde_json::json!(cfg.search.dense_weight),
            "Dense vector search weight in RRF fusion",
        ),
    );
    search.insert(
        "bm25_candidates".into(),
        key(
            "usize",
            serde_json::json!(cfg.search.bm25_candidates),
            "Number of BM25 candidates to retrieve before fusion",
        ),
    );
    search.insert(
        "dense_candidates".into(),
        key(
            "usize",
            serde_json::json!(cfg.search.dense_candidates),
            "Number of dense candidates to retrieve before fusion",
        ),
    );
    search.insert(
        "splade_weight".into(),
        key(
            "f64",
            serde_json::json!(cfg.search.splade_weight),
            "SPLADE expansion weight (0.0 to disable)",
        ),
    );
    search.insert(
        "dense_enabled".into(),
        key(
            "bool",
            serde_json::json!(cfg.search.dense_enabled),
            "Enable the dense (embedding) retrieval path. false → hybrid search ranks with BM25 + graph + rerank (+ SPLADE) only, skipping the embedding engine and the persistent embeddings.json (lighter footprint, no embed latency). An explicit mode=dense query still forces dense.",
        ),
    );
    sections.insert("search".into(), SectionSchema {
            description: "Hybrid search weights for ctx_semantic_search (BM25 + dense vector + SPLADE + graph proximity)".into(),
            keys: search,
        });

    let mut graph = BTreeMap::new();
    graph.insert(
        "traversal_edges".into(),
        key(
            "bool",
            serde_json::json!(cfg.graph.traversal_edges),
            "Learn co-access edges from real sessions (files surfaced together), surface them as decaying `co_access` graph edges, and boost recall by them. Set false for a purely static AST-only graph.",
        ),
    );
    sections.insert(
        "graph".into(),
        SectionSchema {
            description:
                "Code-graph settings, including traversal (co-access) edges learned from sessions"
                    .into(),
            keys: graph,
        },
    );

    let mut index = BTreeMap::new();
    index.insert(
        "respect_gitignore".into(),
        key(
            "bool",
            serde_json::json!(cfg.index.respect_gitignore),
            "Honor .gitignore / global gitignore / .git/info/exclude during index walks. false indexes ignored files too (the vendor-directory guard still applies). CLI override: --no-gitignore / --respect-gitignore.",
        ),
    );
    index.insert(
        "exclude".into(),
        key(
            "string[]",
            serde_json::json!(cfg.index.exclude),
            "Globs dropped from the index corpus (root-relative, forward slashes), e.g. [\"**/*.csv\", \"fixtures/**\"]. Wins over include. CLI --exclude appends per run. Excluded files produce no chunks, graph nodes, or embeddings.",
        ),
    );
    index.insert(
        "include".into(),
        key(
            "string[]",
            serde_json::json!(cfg.index.include),
            "When non-empty, ONLY matching files enter the index corpus, e.g. [\"**/*.rs\", \"**/*.ts\"]. Empty = no restriction. CLI --include replaces this set per run.",
        ),
    );
    sections.insert(
        "index".into(),
        SectionSchema {
            description:
                "Index-time file filters: declare the retrieval corpus explicitly (BM25 + graph + semantic + watch share one filter layer, #735)"
                    .into(),
            keys: index,
        },
    );

    let mut summaries = BTreeMap::new();
    summaries.insert(
        "enabled".into(),
        key(
            "bool",
            serde_json::json!(cfg.summaries.enabled),
            "Record periodic, semantically-recallable AI session summaries (what was done, files, decisions).",
        ),
    );
    summaries.insert(
        "every_n_turns".into(),
        key(
            "u32",
            serde_json::json!(cfg.summaries.every_n_turns),
            "Tool calls between automatic session summaries (gated by the auto-checkpoint cadence).",
        ),
    );
    summaries.insert(
        "max_kept".into(),
        key(
            "u32",
            serde_json::json!(cfg.summaries.max_kept),
            "Maximum session summaries kept per project (oldest pruned first).",
        ),
    );
    sections.insert(
        "summaries".into(),
        SectionSchema {
            description: "AI session summaries: periodic, semantically-recallable session digests"
                .into(),
            keys: summaries,
        },
    );

    let mut embedding = BTreeMap::new();
    embedding.insert(
            "model".into(),
            key(
            "string",
            serde_json::json!("minilm"),
            "Local ONNX embedding model for ctx_semantic_search. One of: minilm (all-MiniLM-L6-v2, 384d, default), nomic (768d) — or any HuggingFace repo with an ONNX export via hf:org/repo[@revision] (e.g. hf:jinaai/jina-embeddings-v2-base-code for code). Switching models re-indexes once on the next search.",
        ),
        );
    embedding.insert(
        "dimensions".into(),
        key(
            "integer",
            serde_json::json!(null),
            "Declared embedding width for hf: custom models (fallback only — the real width is probed from the ONNX graph at load time). Built-in models ignore this key.",
        ),
    );
    embedding.insert(
        "deterministic".into(),
        key(
            "bool",
            serde_json::json!(null),
            "Pin embedding inference to a single CPU thread with no GPU provider so vectors are bit-identical across machines (default: off, multi-threaded GPU-capable path). Extractive prose ranking is already deterministic via score quantization; enable this only for cross-machine reproducibility, at a throughput cost.",
        ),
    );
    sections.insert(
        "embedding".into(),
        SectionSchema {
            description:
                "Semantic-embedding engine settings (model selection for ctx_semantic_search)"
                    .into(),
            keys: embedding,
        },
    );
}
