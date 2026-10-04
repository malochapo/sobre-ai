//! Core config sections.
//! Split out of `schema/mod.rs`; `use super::*` re-imports the key-builder
//! helpers and `SectionSchema`.

#[allow(clippy::wildcard_imports)]
use super::*;
use std::collections::BTreeMap;

pub(super) fn build(sections: &mut BTreeMap<String, SectionSchema>) {
    let cfg = crate::core::config::Config::default();
    let mut root = BTreeMap::new();
    root.insert(
        "passthrough_commands".into(),
        key(
            "string[]",
            serde_json::json!(cfg.passthrough_commands),
            "Commands whose output is passed through uncompressed (the command still runs normally)",
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
        "extra_ignore_patterns".into(),
        key(
            "string[]",
            serde_json::json!(cfg.extra_ignore_patterns),
            "Extra glob patterns to ignore in graph/overview/preload",
        ),
    );
    root.insert(
        "compression_level".into(),
        key(
            "f64",
            serde_json::json!(cfg.compression_level),
            "Compression intensity 0.0 (none) – 1.0 (densest). Tunes read-mode compression continuously and picks the prose style by band: <0.2 off, <0.4 lite, <0.6 standard, <0.8 max, else raw. Empty = adaptive defaults with the lite prose style",
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
}
