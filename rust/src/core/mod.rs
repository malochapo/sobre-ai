// ---------------------------------------------------------------------------
// Domain: Compression
// ---------------------------------------------------------------------------
pub mod aggressiveness;
pub mod artifacts_exec;
pub mod auto_findings;
pub mod compressor;
pub mod entropy;
pub mod json_crush;
pub mod markdown_compact;
pub mod preservation;
pub mod protect;
pub mod rabin_karp;
pub mod runtime_flags;
pub mod structured_read;
pub mod tabular_crush;
pub mod yaml_crush;

// ---------------------------------------------------------------------------
// Domain: Memory
// ---------------------------------------------------------------------------
pub mod interrupt;
pub mod memory_guard;

// ---------------------------------------------------------------------------
// Domain: Graph
// ---------------------------------------------------------------------------
pub mod call_graph;
pub mod graph_index;
pub mod graph_provider;
pub mod property_graph;

// ---------------------------------------------------------------------------
// Domain: Context
// ---------------------------------------------------------------------------
pub mod context_lint;
pub mod context_overhead;

// ---------------------------------------------------------------------------
// Domain: Search & Retrieval
// ---------------------------------------------------------------------------
pub mod bm25_cache;
pub mod bm25_index;
pub mod content_cache;
pub mod context_packing;
#[cfg(feature = "dense")]
pub mod dense_backend;
#[cfg(feature = "dense")]
pub mod embedding_index;
pub mod embedding_quant;
// Without `dense` the engine still compiles these modules — their
// entry points are internally cfg-gated and fall back — but their helpers
// go uncalled. That is honest unused code, not dead code, so it is silenced
// only in that configuration.
#[cfg_attr(not(feature = "dense"), allow(dead_code))]
pub mod embeddings;
pub mod hybrid_search;
#[cfg(feature = "dense")]
pub mod search_reranking;

// ---------------------------------------------------------------------------
// Domain: Session & Handoff
// ---------------------------------------------------------------------------
pub mod clock;
pub mod session;

// ---------------------------------------------------------------------------
// Domain: Neural / ML
// ---------------------------------------------------------------------------
// ORT runtime glue links against the `ort` crate, which is only pulled in by the
// `dense` feature. On platforms ORT does not support (e.g.
// FreeBSD) it is disabled, so the modules must be gated
// to keep the build clean without them.
#[cfg(feature = "dense")]
pub mod ort_environment;
#[cfg(feature = "dense")]
pub mod ort_execution_providers;

// ---------------------------------------------------------------------------
// Domain: Patterns & Shell
// ---------------------------------------------------------------------------
pub mod patterns;

// ---------------------------------------------------------------------------
// Domain: Agents
// ---------------------------------------------------------------------------
pub mod agent_runtime_env;

// ---------------------------------------------------------------------------
// Domain: Adaptive & Scoring
// ---------------------------------------------------------------------------
pub mod adaptive_thresholds;
pub mod auto_mode_resolver;
pub mod task_relevance;

// ---------------------------------------------------------------------------
// Domain: Diagnostics & Quality
// ---------------------------------------------------------------------------
pub mod code_health;
pub mod contracts;
pub mod output_verification;
pub mod quality;
pub mod refetch_signals;
pub mod safety_needles;
pub mod setup_report;
pub mod surprise;

// ---------------------------------------------------------------------------
// Domain: Config & Infrastructure
// ---------------------------------------------------------------------------
pub mod anchor;
#[cfg(feature = "dense")]
pub mod ann_cache;
pub mod atomic_fs;
pub mod binary_detect;
pub mod bounce_tracker;
pub mod budgets;
pub mod cache;
pub mod cloud_files;
pub mod config;
pub mod conversation;
pub mod crash_log;
pub mod data_dir;
pub mod debug_log;
pub mod diagnostics_store;
pub mod error;
pub mod eviction_orchestrator;
pub mod extractors;
pub mod git;
pub mod git_util;
pub mod godot;
pub mod handle;
pub mod hasher;
#[cfg(feature = "dense")]
pub mod hnsw;
pub mod home;
pub mod pricing;

pub mod edit_quality;
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod env_vars;
pub mod http_client;
pub mod import_resolver;
pub mod index_admission;
pub mod index_filter;
pub mod index_namespace;
pub mod index_orchestrator;
pub mod index_paths;
pub mod index_progress;
pub mod ingestion;
pub mod intent_engine;
pub(crate) mod intent_lang;
pub mod io_health;
pub mod jsonc;
pub mod language_capabilities;
#[cfg(target_os = "macos")]
pub mod launchd;
pub mod levenshtein;
pub mod limits;
pub mod logging;
#[cfg(any(test, feature = "test-support"))]
pub mod mcp_manifest;
pub mod path_locks;
pub mod path_resolve;
pub mod paths;
pub mod pathutil;
pub mod portable_binary;
pub mod project_hash;
pub mod protocol;
pub mod read_stub_index;
pub mod recovery;
pub mod redaction;
#[cfg(any(test, feature = "test-support"))]
pub mod reference_docs;
pub mod search_index;
pub mod secret_detection;
pub mod session_token;
pub mod startup_guard;
pub mod syntax_validate;
/// macOS Seatbelt self-sandbox: wraps the launchd-owned proxy in a
/// `sandbox-exec` profile that denies `~/Documents`/`~/Desktop`/`~/Downloads`,
/// so the TCC privacy prompt can never appear.
#[cfg(target_os = "macos")]
pub mod tcc_guard_sandbox;
pub mod terse;
pub mod theme;
pub mod tokens;
pub mod tool_lifecycle;
pub mod usage;
pub mod walk_filter;
pub mod web;

// ---------------------------------------------------------------------------
// Feature-gated modules
// ---------------------------------------------------------------------------
pub mod ast_walk;
#[cfg(feature = "tree-sitter")]
pub mod chunks_ts;
pub mod deep_queries;
pub mod deps;
pub mod editor_registry;
pub mod signatures;
#[cfg(feature = "tree-sitter")]
pub mod signatures_ts;
pub mod storage_maintenance;
pub mod structured_compact;
pub mod type_ref_edges;
