//! Security hardening tests — validates all Critical and High fixes from the
//! bank-readiness audit (2026-05-08).

/// Reads the full `SobreServer` dispatch source across its split submodules
/// (`mod.rs`, `call_tool.rs`, `server_handler.rs`, plus the extracted
/// `post_process.rs` / `post_dispatch.rs` stages) so that the security-invariant
/// checks below stay robust to internal module structure.
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

// ---------------------------------------------------------------------------
// M2 — ReDoS guard in ctx_search
// ---------------------------------------------------------------------------
#[test]
fn ctx_search_has_pattern_length_limit() {
    let src = include_str!("../../src/tools/ctx_search.rs");

    assert!(
        src.contains("MAX_PATTERN_LEN"),
        "M2: ctx_search must limit pattern length"
    );
    assert!(
        src.contains(".size_limit("),
        "M2: ctx_search must set regex size_limit"
    );
    assert!(
        src.contains("RegexBuilder::new("),
        "M2: ctx_search must use RegexBuilder (not Regex::new) for size limits"
    );
}

// ---------------------------------------------------------------------------
// M3 — MCP stdio max_length
// ---------------------------------------------------------------------------
#[test]
fn mcp_stdio_has_bounded_max_length() {
    let src = include_str!("../../src/mcp_stdio.rs");

    assert!(
        !src.contains("max_length: usize::MAX"),
        "M3: MCP stdio must NOT use usize::MAX for max_length"
    );
    assert!(
        src.contains("32 * 1024 * 1024"),
        "M3: MCP stdio max_length must be 32 MiB"
    );
}

// ---------------------------------------------------------------------------
// raw=true bypasses ALL post-processing in call_tool
// ---------------------------------------------------------------------------
#[test]
fn raw_shell_skips_all_postprocessing() {
    let src = crate::suite::security_hardening::server_dispatch_src().replace("\r\n", "\n");
    assert!(
        src.contains("let is_raw_shell = name == \"ctx_shell\""),
        "call_tool must compute is_raw_shell flag"
    );
    assert!(
        src.contains("is_raw_shell")
            && src.contains(
                "post_process::final_token_count(&result_text, pre_terse_len, output_tokens)"
            )
            && src.contains("skip_terse"),
        "skip_terse must include is_raw_shell; the sent count must be taken after terse"
    );
    // Verification only runs on text terse changed, and terse never touches
    // raw shell output.
    assert!(
        src.contains("result_text != pre_compression") && src.contains("verify_output"),
        "output verification must only run on condensed text"
    );
}
