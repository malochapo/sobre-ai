//! Integration tests that change process-global state — environment
//! variables, the data directory, process-wide caches. They share one binary
//! (tests/serial.rs) that runs them one at a time; see that file for how the
//! single thread is enforced.

mod archive_expand_tests;
mod autonomy_tests;
mod bazsi_reported_scenarios;
mod claude_config_dir;
mod codex_proxy_554;
mod ctx_compose_scenarios;
mod dropin_install_tests;
mod dual_process_rehydrate;
mod index_scoping_scenarios;
mod intensive_benchmarks;
mod intent_protocol_side_effects;
mod issue_244_ledger_reset;
mod issue_326_parallel_knowledge;
mod memory_benchmark_suite;
mod multi_read_cap_scenarios;
mod output_contracts_knowledge;
mod p4_docker_claude_mcp_selfheal_eval;
mod power_user_worksession;
mod proxy_stale_env_scenarios;
mod read_only_roots_e2e;
mod savings_verification;
mod terse_agent_tests;
mod ticket_fixes_scenarios;
mod wiring_memory_persistence;
