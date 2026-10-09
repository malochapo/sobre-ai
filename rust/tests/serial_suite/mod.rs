//! Integration tests that change process-global state — environment
//! variables, the data directory, process-wide caches. They share one binary
//! (tests/serial.rs) that runs them one at a time; see that file for how the
//! single thread is enforced.

mod autonomy_tests;
mod claude_config_dir;
mod claude_mcp_selfheal;
mod codex_proxy;
mod dual_process_rehydrate;
mod index_scoping_scenarios;
mod intensive_benchmarks;
mod memory_benchmark_suite;
mod multi_read_cap_scenarios;
mod output_contracts_knowledge;
mod parallel_knowledge;
mod power_user_worksession;
mod proxy_stale_env_scenarios;
mod savings_verification;
mod sobre_compose_scenarios;
mod subagent_cache_hit_scenarios;
mod terse_agent_tests;
mod ticket_fixes_scenarios;
