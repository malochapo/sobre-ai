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
            "Commands whose output is passed through uncompressed (the command still runs normally). An entry matches a command that is the entry or starts with it and a space, case-insensitively: `mytool` matches `mytool --watch`. Adds to the built-in list below",
        ),
    );
    root.insert(
        "compression_level".into(),
        key(
            "f64",
            serde_json::json!(cfg.compression_level),
            "Compression intensity 0.0 (none) – 1.0 (densest), default 0.3. Picks the prose, shell-output and CRP style by band: <0.2 off, <0.4 lite, <0.6 standard, <0.8 max, else raw. File reads choose their mode per file",
        ),
    );
    root.insert(
        "search_roots".into(),
        key(
            "string[]",
            serde_json::json!(cfg.search_roots),
            "Extra directories searched alongside the project root, as the other roots of a multi-root workspace",
        ),
    );
    root.insert(
        "ignore".into(),
        key(
            "string[]",
            serde_json::json!(cfg.ignore),
            "Globs kept out of the search and graph indexes (BM25, dense, graph), on top of .gitignore (always respected). sobre_glob, sobre_tree and regex search do not read it",
        ),
    );
    root.insert(
        "debug_log".into(),
        key(
            "bool",
            serde_json::json!(false),
            "Opt-in (default off): write a human-readable debug log of MCP tool calls and of the native tool calls hooks changed (with the reason) to <state_dir>/logs/debug.log; `sobre doctor` prints the path",
        ),
    );
    root.insert(
        "hook_binary".into(),
        key(
            "string?",
            serde_json::json!(null),
            "Verbatim binary path/expression for generated agent-hook commands (e.g. $HOME/.local/bin/sobre) — for settings files synced across machines with different usernames. Shell-expanded by the hook host at run time; doctor accepts it as current. Empty = automatic absolute-path resolution",
        ),
    );
    root.insert(
        "embedding_model".into(),
        key(
            "string",
            serde_json::json!("minilm"),
            "Embedding model for semantic search (dense builds): minilm (sentence-transformers/all-MiniLM-L6-v2), nomic (nomic-ai/nomic-embed-text-v1.5), each pinned to a commit, or any HuggingFace repo with an ONNX export and a tokenizer.json, as org/repo[@revision]. Pooling and query/document prompts come from the repo's sentence-transformers metadata. Changing it re-indexes on the next semantic search",
        ),
    );
    root.insert(
        "shell_timeout_secs".into(),
        key(
            "u64",
            serde_json::json!(120),
            "sobre_shell stops a command after this many seconds without new output (0 stops it at once; builds and tests get at least 600; a call's timeout_ms wins). A foreground command still running after about 110 s moves to the background and returns a job id",
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
