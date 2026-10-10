//! Integration tests that change process-global state — environment
//! variables, the data directory, process-wide caches. They share one binary
//! (tests/serial.rs) that runs them one at a time; see that file for how the
//! single thread is enforced.

mod auto_dedup;
mod claude_config_dir;
mod codex_proxy;
mod compression_level_config;
mod crash_loop_guard;
mod engine_tool_calls;
mod index_status;
mod intensive_benchmarks;
mod knowledge_archive_rehydrate;
mod knowledge_concurrent_writes;
mod knowledge_persistence;
mod knowledge_recall;
mod multi_read_cap_scenarios;
mod proxy_stale_env_scenarios;
mod read_cache_stub;
mod savings_verification;
mod sobre_compose_scenarios;
