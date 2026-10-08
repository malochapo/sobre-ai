//! Integration tests merged into one binary (tests/main.rs) so target/
//! holds one linked test artifact instead of ~100 (was 17-25 GB of disk).
//! Add new integration tests as modules here, NOT as new tests/*.rs files.
//! A test that mutates process env (set_var) or process-global state (Config
//! cache, stats buffer) goes in tests/serial_suite/ instead, which runs one
//! test at a time. Only a test that needs a process of its own — a OnceLock
//! that must start empty, a lock held for the whole run — gets its own
//! tests/*.rs binary.

mod adversarial_compression;
mod cli_anti_inflation;
mod cli_characterization;
mod compaction_cache_scenarios;
mod compaction_survival_tests;
mod compression_improvements;
mod config_path_parity_594;
mod contracts_frozen;
mod ctx_read_lmd_md_raw;
mod daviddatu_powershell_scenarios;
mod edit_reliability;
mod efficiency_ux_scenarios;
mod embeddings_shared_engine;
mod end_to_end_smoke;
mod engine_call;
mod entrypoints_wired;
mod hardening_ir_traits;
mod hn_hardening_scenarios;
mod home_dotdir_firewall;
mod http_server_streamable;
mod init_doctor_clean;
mod integration_tests;
mod io_health_scenarios;
mod issue_1088_batch_replace_unique;
mod issue_885_anchored_after_patch;
mod live_smoke_client_instructions;
mod lock_contention_hardening;
mod lock_ordering_check;
mod lsp_integration;
mod mcp_fast_initialize_669;
mod outline_machine_readable_e2e;
mod pattern_snapshots;
mod performance_stress;
mod project_data_dir_scenarios;
mod property_compression;
mod property_graph_reproducibility_contract;
mod proxy_smoke;
mod read_fidelity;
mod reference_docs_drift;
mod reranking_scenarios;
mod retrieval_eval;
mod ro_config_sandbox;
mod scientific_verification;
mod secret_scan_artifacts;
mod security_hardening;
mod security_resolve_path_guard;
mod sensitivity_floor;
mod session_lock_timeout_scenarios;
mod setup_ci_smoke;
mod setup_symlink_596;
mod shell_and_agent_tests;
mod shell_hook_595;
mod terse_eval;
mod tool_schema_strictness;
mod uninstall_help_safety_476;
mod utf8_safety_scenarios;
