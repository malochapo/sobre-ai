//! Integration tests merged into one binary (tests/main.rs), so target/
//! holds one linked test artifact instead of one per file.
//! Add new integration tests as modules here, NOT as new tests/*.rs files.
//! A test that mutates process env (set_var) or process-global state (Config
//! cache, stats buffer) goes in tests/serial_suite/ instead, which runs one
//! test at a time. Only a test that needs a process of its own — a OnceLock
//! that must start empty, a lock held for the whole run — gets its own
//! tests/*.rs binary.

mod adversarial_compression;
mod agent_config_files;
mod anchored_read_after_patch;
mod batch_replace_unique;
mod broad_project_roots;
mod cli_anti_inflation;
mod cli_characterization;
mod compaction_cache_scenarios;
mod compression_properties;
mod config_path_parity;
mod contracts_frozen;
mod edit_reliability;
mod end_to_end_smoke;
mod engine_call;
mod entrypoints_wired;
mod git_command_fidelity;
mod hardening_ir_traits;
mod home_dotdir_firewall;
mod init_doctor_clean;
mod init_smoke;
mod init_symlinked_configs;
mod init_uninstall_round_trip;
mod integration_tests;
mod io_health_scenarios;
mod live_smoke_client_instructions;
mod lock_contention_hardening;
mod lock_ordering_check;
mod ls_and_grep_patterns;
mod lsp_integration;
mod mcp_fast_initialize;
mod outline_machine_readable_e2e;
mod patch_dry_run_and_modes;
mod pattern_snapshots;
mod performance_stress;
mod property_compression;
mod property_graph_reproducibility_contract;
mod proxy_smoke;
mod read_fidelity;
mod reference_docs_drift;
mod refetch_signals;
#[cfg(feature = "dense")]
mod reranking_scenarios;
mod ro_config_sandbox;
mod scientific_verification;
mod secret_scan_artifacts;
mod security_hardening;
mod security_resolve_path_guard;
mod session_lock_timeout_scenarios;
mod shell_and_agent_tests;
mod shell_hook_wrapper;
mod test_runner_output;
mod tool_handler_scenarios;
mod tool_schema_strictness;
mod uninstall_help_safety;
