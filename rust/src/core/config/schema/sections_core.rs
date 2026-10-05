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
            "Paths the tools may read and write besides the project root; each is also searched as an extra root. [\"/\"] opens the path jail",
        ),
    );
    root.insert(
        "ignore".into(),
        key(
            "string[]",
            serde_json::json!(cfg.ignore),
            "Globs kept out of the index and project walks, on top of .gitignore (always respected)",
        ),
    );
    root.insert(
        "shell_hook".into(),
        key(
            "bool",
            serde_json::json!(cfg.shell_hook),
            "Install the shell hook that routes common commands through sobre",
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
        "debug_log".into(),
        key(
            "bool",
            serde_json::json!(false),
            "Opt-in (default off): write a human-readable debug log of intercepted MCP tool calls and hook routing decisions (lean-ctx vs native, with the reason) to <state_dir>/logs/debug.log. View with `lean-ctx debug-log`",
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
        "shell_timeout_secs".into(),
        key(
            "u64?",
            serde_json::json!(null),
            "Shell command timeout (seconds) for normal commands. null = built-in 2-minute default",
        ),
    );

    sections.insert(
        "root".into(),
        SectionSchema {
            description: "Top-level configuration keys".into(),
            keys: root,
        },
    );
}
