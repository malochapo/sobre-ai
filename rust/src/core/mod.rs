// ---------------------------------------------------------------------------
// Domain: Compression
// ---------------------------------------------------------------------------
pub mod aggressiveness;
pub mod artifacts_exec;
pub mod auto_findings;
pub mod codebook;
#[cfg(target_os = "macos")]
pub mod codesign;
pub mod compressor;
pub mod entropy;
pub mod eval_harness;
// Without `embeddings` the engine still compiles these modules — their
// entry points are internally cfg-gated and fall back — but their helpers
// go uncalled. That is honest unused code, not dead code, so it is silenced
// only in that configuration. TODO: proper cfg hygiene across the 21 call
// sites listed in docs/removals.md would let this attribute go away.
#[cfg_attr(not(feature = "embeddings"), allow(dead_code))]
pub mod extractive;
pub mod json_crush;
pub mod markdown_compact;
pub mod pop_pruning;
pub mod preservation;
pub mod protect;
pub mod rabin_karp;
pub mod relevance_tracker;
pub mod runtime_flags;
pub mod structured_read;
pub mod tabular_crush;
pub mod yaml_crush;

/// Convenience re-export: all compression-related modules.
pub mod compression {
    pub use super::codebook;
    pub use super::compressor;
    pub use super::entropy;
    pub use super::json_crush;
    pub use super::preservation;
    pub use super::rabin_karp;
}

// ---------------------------------------------------------------------------
// Domain: Memory
// ---------------------------------------------------------------------------
pub mod episodic_memory;
pub mod interrupt;
pub mod memory_archive;
pub mod memory_boundary;
pub mod memory_capacity;
pub mod memory_guard;
pub mod memory_lifecycle;
pub mod memory_policy;
pub mod memory_salience;
pub mod procedural_memory;
pub mod prospective_memory;

/// Convenience re-export: all memory-related modules.
pub mod memory {
    pub use super::episodic_memory;
    pub use super::memory_boundary;
    pub use super::memory_lifecycle;
    pub use super::memory_policy;
    pub use super::procedural_memory;
    pub use super::prospective_memory;
}

// ---------------------------------------------------------------------------
// Domain: Graph
// ---------------------------------------------------------------------------
pub mod call_graph;
pub mod community;
pub mod graph_analysis;
pub mod graph_context;
pub mod graph_enricher;
pub mod graph_export;
pub mod graph_index;
pub mod graph_parity;
pub mod graph_provider;
pub mod pagerank;
pub mod property_graph;
pub mod repomap;

/// Convenience re-export: all graph-related modules.
pub mod graph {
    pub use super::call_graph;
    pub use super::community;
    pub use super::graph_context;
    pub use super::graph_enricher;
    pub use super::graph_export;
    pub use super::graph_index;
    pub use super::graph_provider;
    pub use super::pagerank;
    pub use super::property_graph;
}

// ---------------------------------------------------------------------------
// Domain: Context
// ---------------------------------------------------------------------------
pub mod context_field;
pub mod context_handles;
pub mod context_ir;
pub mod context_ledger;
pub mod context_lint;
pub mod context_overhead;
pub mod context_radar;
pub mod context_snapshot;
pub mod cross_source_hints;

/// Convenience re-export: all context-related modules.
pub mod context {
    pub use super::context_field;
    pub use super::context_handles;
    pub use super::context_ir;
    pub use super::context_ledger;
}

// ---------------------------------------------------------------------------
// Domain: Knowledge
// ---------------------------------------------------------------------------
pub mod cognition_loop;
pub mod cognition_scheduler;
pub mod knowledge;
pub mod knowledge_bridge;
// Without `embeddings` the engine still compiles these modules — their
// entry points are internally cfg-gated and fall back — but their helpers
// go uncalled. That is honest unused code, not dead code, so it is silenced
// only in that configuration. TODO: proper cfg hygiene across the 21 call
// sites listed in docs/removals.md would let this attribute go away.
#[cfg_attr(not(feature = "embeddings"), allow(dead_code))]
pub mod knowledge_embedding;
pub mod knowledge_relations;

/// Convenience re-export: all knowledge-related modules.
pub mod knowledge_domain {
    pub use super::cognition_loop;
    pub use super::knowledge;
    pub use super::knowledge_bridge;
    pub use super::knowledge_embedding;
    pub use super::knowledge_relations;
}

// ---------------------------------------------------------------------------
// Domain: Search & Retrieval
// ---------------------------------------------------------------------------
pub mod bm25_cache;
pub mod bm25_index;
pub mod content_cache;
pub mod content_chunk;
pub mod context_packing;
pub mod cooccurrence;
pub mod dense_backend;
pub mod embedding_index;
pub mod embedding_quant;
// Without `embeddings` the engine still compiles these modules — their
// entry points are internally cfg-gated and fall back — but their helpers
// go uncalled. That is honest unused code, not dead code, so it is silenced
// only in that configuration. TODO: proper cfg hygiene across the 21 call
// sites listed in docs/removals.md would let this attribute go away.
#[cfg_attr(not(feature = "embeddings"), allow(dead_code))]
pub mod embeddings;
pub mod energy;
pub mod hybrid_search;
pub mod search_reranking;
pub mod splade_retrieval;
pub mod spreading_activation;

/// Convenience re-export: all search-related modules.
pub mod search {
    pub use super::bm25_index;
    pub use super::content_chunk;
    pub use super::dense_backend;
    pub use super::embedding_index;
    pub use super::embeddings;
    pub use super::hybrid_search;
    pub use super::search_reranking;
    pub use super::splade_retrieval;
}

// ---------------------------------------------------------------------------
// Domain: Session & Handoff
// ---------------------------------------------------------------------------
pub mod ccp_session_bundle;
pub mod session;
pub mod session_diff;

/// Convenience re-export: all session-related modules.
pub mod session_domain {
    pub use super::ccp_session_bundle;
    pub use super::session;
    pub use super::session_diff;
}

// ---------------------------------------------------------------------------
// Domain: Neural / ML
// ---------------------------------------------------------------------------
pub mod neural;
// ORT runtime glue links against the `ort` crate, which is only pulled in by the
// `embeddings` or `neural` features. On platforms ORT does not support (e.g.
// FreeBSD, see #586) these features are disabled, so the modules must be gated
// to keep the build clean without them.
#[cfg(any(feature = "embeddings", feature = "neural"))]
pub mod ort_environment;
#[cfg(any(feature = "embeddings", feature = "neural"))]
pub mod ort_execution_providers;

// ---------------------------------------------------------------------------
// Domain: Patterns & Shell
// ---------------------------------------------------------------------------
pub mod patterns;

// ---------------------------------------------------------------------------
// Domain: Agents & A2A
// ---------------------------------------------------------------------------
pub mod a2a;
pub mod agent_identity;
pub mod agent_runtime_env;
pub mod agents;

// ---------------------------------------------------------------------------
// Domain: Adaptive & Scoring
// ---------------------------------------------------------------------------
pub mod adaptive;
pub mod adaptive_thresholds;
pub mod auto_mode_resolver;
pub mod model_registry;
pub mod task_relevance;

// ---------------------------------------------------------------------------
// Domain: Diagnostics & Quality
// ---------------------------------------------------------------------------
pub mod benchmark;
/// Commercial-plane billing substrate (`billing-plane-v1`): plans, entitlements,
/// and usage metering derived from the signed savings ledger. Never gates local.
pub mod code_health;
pub mod conformance;
pub mod contracts;
pub mod cyclomatic;
pub mod output_verification;
pub mod quality;
pub mod refetch_signals;
pub mod safety_needles;
pub mod scorecard;
pub mod setup_report;
pub mod slow_log;
pub mod smells;
pub mod surprise;

// ---------------------------------------------------------------------------
// Domain: Config & Infrastructure
// ---------------------------------------------------------------------------
pub mod anchor;
pub mod ann_cache;
pub mod atomic_fs;
pub mod audit_trail;
pub mod binary_detect;
pub mod bounce_tracker;
pub mod budgets;
pub mod cache;
pub mod cache_telemetry;
pub mod cli_cache;
pub mod client_capabilities;
pub mod client_constraints;
pub mod cloud_files;
pub mod config;
pub mod consolidation;
pub mod consolidation_engine;
pub mod conversation;
pub mod crash_log;
pub mod data_dir;
pub mod debug_log;
pub mod diagnostics_store;
pub mod error;
pub mod events;
pub mod eviction_orchestrator;
pub mod extension_registry;
pub mod extractors;
pub mod gain;
pub mod git;
pub mod git_cache;
pub mod git_signals;
pub mod git_util;
pub mod godot;
pub mod gotcha_tracker;
pub mod handle;
pub mod hasher;
pub mod heatmap;
pub mod hebbian_cache;
pub mod hnsw;
pub mod home;
pub mod homeostasis;
pub mod quality_benchmark;

pub mod edit_quality;
pub mod graph_cache;
pub mod http_client;
pub mod import_resolver;
pub mod index_admission;
pub mod index_filter;
pub mod index_namespace;
pub mod index_orchestrator;
pub mod index_paths;
pub mod index_progress;
pub mod ingestion;
pub mod integrity;
pub mod intent_engine;
pub(crate) mod intent_lang;
pub mod intent_router;
pub mod introspect;
pub mod io_health;
pub mod jsonc;
pub mod language_capabilities;
#[cfg(target_os = "macos")]
pub mod launchd;
pub mod levenshtein;
pub mod limits;
pub mod logging;
pub mod mcp_manifest;
pub mod nc_compress;
pub mod output_echo;
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
pub mod reference_docs;
pub mod sanitize;
pub mod savings_ledger;
pub mod search_index;
pub mod secret_detection;
pub mod sensitivity;
pub mod session_token;
pub mod startup_guard;
pub mod stats;
pub mod syntax_validate;
pub mod task_briefing;
/// macOS Seatbelt self-sandbox (#356): wraps launchd-owned daemon/proxy/updater
/// in a `sandbox-exec` profile that denies `~/Documents`/`~/Desktop`/
/// `~/Downloads`, so the TCC privacy prompt can never appear.
#[cfg(target_os = "macos")]
pub mod tcc_guard_sandbox;
pub mod tdd_schema;
pub mod terse;
pub mod text_read;
pub mod text_similarity;
pub mod theme;
pub mod tokens;
pub mod tool_health;
pub mod tool_lifecycle;
pub mod transcript_compact;
pub mod visualizer;
pub mod walk_filter;
pub mod web;
pub mod wrapped;

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
