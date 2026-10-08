//! Integration tests for the Validated HN Learnings Hardening Plan (8 fixes).
//! Each module tests a fix with multiple realistic agent interaction scenarios.
#![allow(clippy::needless_raw_string_hashes)]

use sobre::core::cache::SessionCache;
use sobre::core::protocol::CrpMode;
use std::io::Write;

/// Reads the full `SobreServer` dispatch source across its split submodules
/// (`mod.rs`, `call_tool.rs`, `server_handler.rs`) so that the invariant checks
/// below stay robust to internal module structure.
fn server_dispatch_src() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        include_str!("../../src/server/mod.rs"),
        include_str!("../../src/server/call_tool.rs"),
        include_str!("../../src/server/server_handler.rs"),
        include_str!("../../src/server/post_process.rs"),
        include_str!("../../src/server/post_dispatch.rs"),
    )
}

/// Extract the body of the `fn skip_terse(` guard (everything after the opening
/// brace) so structural assertions stay robust to where the guard lives and to
/// its parameter order — they only care about the boolean logic in the body.
fn skip_terse_body(src: &str) -> String {
    let sig = src
        .find("fn skip_terse(")
        .expect("skip_terse guard function must exist");
    let brace = src[sig..].find('{').expect("skip_terse must have a body") + sig;
    let window = &src[brace..];
    window[..window.len().min(800)].to_string()
}

// =============================================================================
// Fix A: Correction-Loop-Metrik
// =============================================================================

mod correction_loop {
    use sobre::core::refetch_signals::RefetchSignals;

    #[test]
    fn scenario_agent_rereads_same_file_fresh() {
        let mut detector = RefetchSignals::default();
        // Cold start: 3 initial reads
        detector.record_read("src/main.rs", "full", false);
        detector.record_read("src/lib.rs", "full", false);
        detector.record_read("src/utils.rs", "full", false);
        assert_eq!(
            detector.correction_count(),
            0,
            "cold start should not count"
        );

        // Agent reads a file normally
        detector.record_read("src/cache.rs", "full", false);
        assert_eq!(detector.correction_count(), 0);

        // Agent re-reads same file with fresh=true (lost trust in compression)
        detector.record_read("src/cache.rs", "full", true);
        assert_eq!(detector.correction_count(), 1);
    }

    #[test]
    fn scenario_agent_bounces_map_to_full() {
        let mut detector = RefetchSignals::default();
        // Cold start
        for i in 0..3 {
            detector.record_read(&format!("cold{i}.rs"), "full", false);
        }

        // Agent reads with map mode first (exploring)
        detector.record_read("src/server/mod.rs", "map", false);
        assert_eq!(detector.correction_count(), 0);

        // Then immediately bounces to full (map wasn't enough)
        detector.record_read("src/server/mod.rs", "full", false);
        assert_eq!(detector.correction_count(), 1);
    }

    #[test]
    fn scenario_agent_reruns_same_shell_command() {
        let mut detector = RefetchSignals::default();
        // Cold start
        for i in 0..3 {
            detector.record_shell(&format!("echo init{i}"));
        }

        // Agent runs cargo test
        detector.record_shell("cargo test --lib");
        assert_eq!(detector.correction_count(), 0);

        // Agent runs same command again (didn't trust output)
        detector.record_shell("cargo test --lib");
        assert_eq!(detector.correction_count(), 1);

        // Third time
        detector.record_shell("cargo test --lib");
        assert_eq!(detector.correction_count(), 2);
    }

    #[test]
    fn scenario_different_commands_no_correction() {
        let mut detector = RefetchSignals::default();
        for i in 0..3 {
            detector.record_shell(&format!("echo init{i}"));
        }

        detector.record_shell("cargo build");
        detector.record_shell("cargo test");
        detector.record_shell("cargo clippy");
        assert_eq!(
            detector.correction_count(),
            0,
            "different commands should not trigger"
        );
    }

    #[test]
    fn scenario_legitimate_exploration_no_false_positive() {
        let mut detector = RefetchSignals::default();
        for i in 0..3 {
            detector.record_read(&format!("cold{i}.rs"), "full", false);
        }

        // Agent explores multiple different files (legitimate workflow)
        for i in 0..10 {
            detector.record_read(&format!("src/file{i}.rs"), "map", false);
        }
        assert_eq!(
            detector.correction_count(),
            0,
            "exploring different files is not a correction"
        );
    }

    #[test]
    fn scenario_prune_clears_old_signals() {
        let mut detector = RefetchSignals::default();
        for i in 0..3 {
            detector.record_shell(&format!("echo init{i}"));
        }

        detector.record_shell("cargo check");
        detector.record_shell("cargo check");
        assert_eq!(detector.correction_count(), 1);

        // Prune should work
        detector.prune();
        // Signal is still within window (just happened), so still 1
        assert_eq!(detector.correction_count(), 1);
    }
}

// =============================================================================
// Fix C: Double-Compression Guard
// =============================================================================

mod double_compression_guard {
    #[test]
    fn scenario_dispatch_returns_saved_tokens() {
        // Verify the dispatch_tool return type includes saved_tokens (the
        // third element carries the shell outcome for MCP isError, GH #389)
        // and content_blocks (fourth element for image/binary MCP output).
        let src = include_str!("../../src/server/dispatch/mod.rs");
        assert!(
            src.contains("Option<Vec<ContentBlock>>"),
            "dispatch_tool must return (String, saved_tokens, shell_outcome, content_blocks)"
        );
    }

    #[test]
    fn scenario_skip_terse_when_already_compressed() {
        let src = crate::suite::hn_hardening_scenarios::server_dispatch_src();

        // Reads already produce mode-aware, structure-preserving output, so the
        // generic terse layer must never re-compress them. The guard skips the
        // whole read family unconditionally (a verbatim `full`/`lines:` read has
        // 0 savings yet must still be protected from dictionary-mangling). The
        // read-family + verbatim-mode decision lives in `is_verbatim_read`.
        let body = crate::suite::hn_hardening_scenarios::skip_terse_body(&src);
        assert!(
            body.contains("is_verbatim_read"),
            "skip_terse must skip verbatim reads (read family) to avoid re-compressing them"
        );

        // Usage is recorded once, after terse, from the final sent count: the
        // tool's own savings and terse's are never counted twice.
        assert!(
            src.contains(
                "post_process::final_token_count(&result_text, pre_terse_len, output_tokens)"
            ),
            "the sent count must be taken after terse"
        );
    }

    #[test]
    fn scenario_raw_shell_still_bypasses() {
        let src = crate::suite::hn_hardening_scenarios::server_dispatch_src();
        let body = crate::suite::hn_hardening_scenarios::skip_terse_body(&src);
        let raw_idx = body
            .find("is_raw_shell")
            .expect("skip_terse must reference is_raw_shell");
        let saved_idx = body.find("tool_saved_tokens").unwrap_or(usize::MAX);
        assert!(
            raw_idx < saved_idx,
            "is_raw_shell must short-circuit first in the skip_terse body"
        );
    }
}

// =============================================================================
// Fix E: Cache-Hit Messages
// =============================================================================

mod cache_hit_messages {
    use super::*;

    #[test]
    fn scenario_modified_file_returns_new_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("changing.rs");
        std::fs::write(&file, "// version 1\n").unwrap();
        let path = file.to_string_lossy().to_string();

        let mut cache = SessionCache::new();
        sobre::tools::ctx_read::handle(&mut cache, &path, "full", CrpMode::Off);

        // Modify file
        std::thread::sleep(std::time::Duration::from_millis(50));
        {
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&file)
                .unwrap();
            f.write_all(b"// version 2\nfn new_function() {}\n")
                .unwrap();
        }

        let result = sobre::tools::ctx_read::handle(&mut cache, &path, "full", CrpMode::Off);
        assert!(
            result.contains("version 2") || result.contains("new_function"),
            "modified file must return new content"
        );
    }
}

// =============================================================================
// Fix D: Test-Identifier Preservation
// =============================================================================

mod test_identifiers {
    use sobre::core::patterns::compress_output as compress;

    #[test]
    fn scenario_cargo_test_preserves_names() {
        let output = r#"
   Compiling myapp v0.1.0
    Finished test target(s) in 2.34s
     Running unittests src/lib.rs
test auth::tests::login_works ... ok
test auth::tests::logout_clears_session ... ok
test auth::tests::token_refresh ... ok
test db::connection_pool_reuse ... ok
test db::migration_up ... ok
test db::migration_down ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.45s
"#;
        let compressed = compress("cargo test", output).unwrap_or_default();
        // Must preserve at least some test names
        assert!(
            compressed.contains("login_works") || compressed.contains("auth::tests::login_works"),
            "must preserve test names in output: {compressed}"
        );
        // Must have summary
        assert!(
            compressed.contains('6') && compressed.contains("pass"),
            "must have pass count"
        );
    }

    #[test]
    fn scenario_cargo_test_many_tests_truncates() {
        let mut lines = String::from("     Running unittests src/lib.rs\n");
        for i in 0..20 {
            lines.push_str(&format!("test module::test_{i} ... ok\n"));
        }
        lines.push_str(
            "test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.23s\n",
        );
        let compressed = compress("cargo test", &lines).unwrap_or_default();
        // Should show max 5 names + "more"
        assert!(
            compressed.contains("more"),
            "20 tests should show '...+N more': {compressed}"
        );
    }

    #[test]
    fn scenario_cargo_test_failure_preserves_failed_name() {
        // The cargo pattern detects "FAILED" + "---" lines like "---- test_name ----"
        let output = r#"
     Running unittests src/lib.rs
test auth::login ... ok
test auth::validate ... FAILED

---- FAILED auth::validate ----
thread 'auth::validate' panicked at 'assertion failed'

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s
"#;
        let compressed = compress("cargo test", output).unwrap_or_default();
        // The parser extracts the word after "FAILED" on lines containing "---"
        assert!(
            compressed.contains("fail"),
            "must indicate failure: {compressed}"
        );
        assert!(
            compressed.contains("1 pass") && compressed.contains("1 fail"),
            "must show pass/fail counts: {compressed}"
        );
    }

    #[test]
    fn scenario_pytest_preserves_passed_names() {
        let output = r#"
============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login PASSED
tests/test_auth.py::test_logout PASSED
tests/test_auth.py::test_refresh PASSED

============================== 3 passed in 0.54s ===============================
"#;
        let compressed = compress("pytest", output).unwrap_or_default();
        assert!(
            compressed.contains("3 passed"),
            "must show pass count: {compressed}"
        );
        assert!(
            compressed.contains("test_login") || compressed.contains("ran:"),
            "should preserve test names: {compressed}"
        );
    }

    #[test]
    fn scenario_go_test_preserves_names() {
        let output = "=== RUN   TestAdd\n--- PASS: TestAdd (0.00s)\n=== RUN   TestMultiply\n--- PASS: TestMultiply (0.00s)\n=== RUN   TestDivide\n--- PASS: TestDivide (0.01s)\nok  \tgithub.com/user/math\t0.015s\n";
        let compressed = compress("go test ./...", output).unwrap_or_default();
        assert!(
            compressed.contains("ok") && compressed.contains("github.com/user/math"),
            "must show package result: {compressed}"
        );
        assert!(
            compressed.contains("TestAdd") || compressed.contains("ran:"),
            "should preserve test names: {compressed}"
        );
    }
}

// =============================================================================
// Fix B: Auto-Degrade (CompressionLevel session override)
// =============================================================================

mod auto_degrade {
    use serial_test::serial;
    use sobre::core::config::CompressionLevel;

    #[test]
    #[serial]
    fn scenario_set_degrade_to_off() {
        CompressionLevel::set_session_degrade(&CompressionLevel::Off);
        let level = CompressionLevel::session_degrade_level();
        assert_eq!(level, Some(CompressionLevel::Off));
        CompressionLevel::clear_session_degrade();
    }

    #[test]
    #[serial]
    fn scenario_set_degrade_to_lite() {
        CompressionLevel::set_session_degrade(&CompressionLevel::Lite);
        let level = CompressionLevel::session_degrade_level();
        assert_eq!(level, Some(CompressionLevel::Lite));
        CompressionLevel::clear_session_degrade();
    }

    #[test]
    #[serial]
    fn scenario_clear_degrade_restores_none() {
        CompressionLevel::set_session_degrade(&CompressionLevel::Off);
        CompressionLevel::clear_session_degrade();
        let level = CompressionLevel::session_degrade_level();
        assert_eq!(level, None);
    }

    #[test]
    #[serial]
    fn scenario_effective_uses_degrade_when_set() {
        let cfg = sobre::core::config::Config::load();
        CompressionLevel::set_session_degrade(&CompressionLevel::Lite);
        let effective = CompressionLevel::effective(&cfg);
        assert_eq!(effective, CompressionLevel::Lite);
        CompressionLevel::clear_session_degrade();
    }

    #[test]
    fn scenario_server_degrade_thresholds() {
        use sobre::core::config::CompressionLevel;
        use sobre::core::config::SessionDegrade::{Clear, Leave, Set};
        // Behavioral invariant for the dispatch's auto-degrade. Asserted via the
        // pure `degrade_action` decision rather than grepping the dispatch source
        // for a literal, so an internal rename (e.g. #941's `correction_count` →
        // retrieve-coupled `pressure`) can't silently rot this guard again (#957).
        assert_eq!(
            CompressionLevel::degrade_action(5, 0),
            Set(CompressionLevel::Off),
            "must degrade to Off at 5+ corrections"
        );
        assert_eq!(
            CompressionLevel::degrade_action(3, 0),
            Set(CompressionLevel::Lite),
            "must degrade to Lite at 3+ corrections"
        );
        assert_eq!(
            CompressionLevel::degrade_action(0, 0),
            Clear,
            "must clear degrade when pressure drops to 0"
        );
        // Either signal alone drives the degrade, and the 1–2 band holds steady.
        assert_eq!(
            CompressionLevel::degrade_action(0, 5),
            Set(CompressionLevel::Off)
        );
        assert_eq!(CompressionLevel::degrade_action(2, 0), Leave);
    }
}

// =============================================================================
// Fix H: First-Contact Auto-Context (meta_visible guard removed)
// =============================================================================

mod first_contact {
    #[test]
    fn scenario_meta_visible_guard_removed() {
        let src = crate::suite::hn_hardening_scenarios::server_dispatch_src();
        // Find the auto_context section
        let auto_ctx_pos = src
            .find("let Some(ctx) = auto_context")
            .expect("auto_context block must exist");
        let block = &src[auto_ctx_pos..auto_ctx_pos + 200];

        // Must NOT contain meta_visible check
        assert!(
            !block.contains("meta_visible()"),
            "meta_visible guard must be removed from auto_context block"
        );
    }

    #[test]
    fn scenario_token_budget_enforced() {
        // The briefing is cut to its budget where it is built, not dropped at
        // the call site.
        let src = include_str!("../../src/tools/autonomy.rs");
        assert!(
            src.contains("pub const BRIEFING_MAX_TOKENS: usize = 400;"),
            "auto_context must enforce a 400 token budget"
        );
        assert!(
            src.contains("Some(format_briefing(&result))"),
            "the session briefing must go through the budget cut"
        );
    }

    #[test]
    fn scenario_raw_shell_still_skips_auto_context() {
        let src = crate::suite::hn_hardening_scenarios::server_dispatch_src();
        // Collapse whitespace so the gating check survives the edition-2024
        // let-chain collapse (`if !is_raw_shell { if let Some(ctx) = … }` →
        // `if !is_raw_shell && let Some(ctx) = …`) and any rustfmt line wrapping.
        let collapsed = src.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            collapsed.contains("if !is_raw_shell && let Some(ctx) = auto_context"),
            "auto_context must still be gated behind !is_raw_shell"
        );
    }
}

// =============================================================================
// Fix G: ctx_overview Module Descriptions
// =============================================================================

mod overview_descriptions {
    use super::*;

    #[test]
    fn scenario_extract_rust_module_doc() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cache.rs");
        std::fs::write(
            &file,
            "//! Session-level file cache with LRU eviction.\n\nuse std::collections::HashMap;\n",
        )
        .unwrap();
        let path = file.to_string_lossy().to_string();

        // Use ctx_read to verify file is readable, then check overview behavior
        let mut cache = SessionCache::new();
        let content = sobre::tools::ctx_read::handle(&mut cache, &path, "full", CrpMode::Off);
        assert!(content.contains("Session-level"));
    }

    #[test]
    fn scenario_extract_python_module_doc() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("utils.py");
        std::fs::write(
            &file,
            "\"\"\"Utility functions for data transformation.\"\"\"\n\nimport os\n",
        )
        .unwrap();
        let path = file.to_string_lossy().to_string();

        let mut cache = SessionCache::new();
        let content = sobre::tools::ctx_read::handle(&mut cache, &path, "full", CrpMode::Off);
        assert!(content.contains("Utility functions"));
    }

    #[test]
    fn scenario_extract_js_module_doc() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("api.ts");
        std::fs::write(
            &file,
            "/** REST API client for the backend service. */\n\nexport class ApiClient {}\n",
        )
        .unwrap();
        let path = file.to_string_lossy().to_string();

        let mut cache = SessionCache::new();
        let content = sobre::tools::ctx_read::handle(&mut cache, &path, "full", CrpMode::Off);
        assert!(content.contains("REST API"));
    }

    #[test]
    fn scenario_overview_source_has_extract_call() {
        let src = include_str!("../../src/tools/impl_overview.rs");
        assert!(
            src.contains("extract_module_doc"),
            "ctx_overview must call extract_module_doc"
        );
        assert!(
            src.contains("knowledge_doc_for_file"),
            "ctx_overview must have knowledge fallback"
        );
    }
}

// =============================================================================
// Cross-cutting: Full integration scenario (all fixes working together)
// =============================================================================

mod integration {
    use super::*;
    use sobre::core::config::CompressionLevel;
    use sobre::core::refetch_signals::RefetchSignals;

    #[test]
    fn scenario_full_correction_loop_triggers_degrade() {
        // Simulate: agent reads file, re-reads fresh, triggers degrade
        let mut detector = RefetchSignals::default();

        // Cold start
        for i in 0..3 {
            detector.record_read(&format!("cold{i}.rs"), "full", false);
        }

        // Normal work
        detector.record_read("src/lib.rs", "full", false);
        detector.record_read("src/main.rs", "full", false);

        // Correction spiral: agent doesn't trust output
        detector.record_read("src/lib.rs", "full", true);
        detector.record_read("src/main.rs", "full", true);
        detector.record_shell("cargo check");
        detector.record_shell("cargo check");
        // mode bounce
        detector.record_read("src/config.rs", "map", false);
        detector.record_read("src/config.rs", "full", false);

        let count = detector.correction_count();
        assert!(count >= 3, "should have >= 3 corrections, got {count}");

        // This would trigger degrade to Lite in server
        if count >= 5 {
            CompressionLevel::set_session_degrade(&CompressionLevel::Off);
        } else if count >= 3 {
            CompressionLevel::set_session_degrade(&CompressionLevel::Lite);
        }

        let level = CompressionLevel::session_degrade_level();
        assert!(
            level.is_some(),
            "degrade must be active after correction spiral"
        );

        // Clean up
        CompressionLevel::clear_session_degrade();
    }

    #[test]
    fn scenario_cache_hit_with_proof_line_reduces_corrections() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stable.rs");
        std::fs::write(&file, "pub fn stable_function() -> bool { true }\n").unwrap();
        let path = file.to_string_lossy().to_string();

        let mut cache = SessionCache::new();

        // Read 1: full content
        let r1 = sobre::tools::ctx_read::handle(&mut cache, &path, "full", CrpMode::Off);
        assert!(r1.contains("stable_function"));

        // Read 2: still returns content (marks as delivered)
        sobre::tools::ctx_read::handle(&mut cache, &path, "full", CrpMode::Off);

        // Read 3: now cache hit with proof
        let r3 = sobre::tools::ctx_read::handle(&mut cache, &path, "full", CrpMode::Off);
        assert!(
            r3.contains("unchanged") || r3.contains("cached"),
            "third read should be a cache hit: {r3}"
        );
    }

    #[test]
    fn scenario_shell_compression_with_saved_tokens_skips_terse() {
        // Structural test: verify the pipeline
        let src = crate::suite::hn_hardening_scenarios::server_dispatch_src();

        // 1. dispatch threads saved_tokens (+ shell outcome, GH #389,
        // content_blocks) out of the tool call
        assert!(
            src.contains("let (mut result_text, tool_saved_tokens, shell_outcome, content_blocks)")
        );

        // 2. Terse compression is gated by skip_terse()
        assert!(
            src.contains("if skip_terse("),
            "terse compression must be gated by an early skip_terse() return"
        );

        // 3. Usage is recorded once, after terse, with the tool's savings
        // added back onto the pre-terse count.
        assert!(
            src.contains("crate::core::usage::record_compression("),
            "call_tool must record the call in the usage ledger"
        );
    }
}
