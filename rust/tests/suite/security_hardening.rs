//! Security hardening tests — validates all Critical and High fixes from the
//! bank-readiness audit (2026-05-08).

/// Reads the full `LeanCtxServer` dispatch source across its split submodules
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

#[test]
fn lineage_filters_by_workspace() {
    let src = include_str!("../../src/core/context_os/context_bus.rs");

    let lineage_fn = src.find("fn lineage(").expect("lineage fn missing");
    let lineage_sig = &src[lineage_fn..lineage_fn + 200];
    assert!(
        lineage_sig.contains("workspace_id: &str"),
        "C2: lineage() must take workspace_id parameter"
    );

    let lineage_body = &src[lineage_fn..lineage_fn + 800];
    assert!(
        lineage_body.contains("AND workspace_id = ?2"),
        "C2: lineage SQL must filter by workspace_id"
    );
}

// ---------------------------------------------------------------------------
// H1 — Shell CWD jail enforcement
// ---------------------------------------------------------------------------
#[test]
fn effective_cwd_calls_jail() {
    let src = include_str!("../../src/core/session/state.rs");

    // `effective_cwd` must not bypass the jail: it delegates to the checked
    // variant, which is the single enforcement point (#629/#633 refactor).
    let ecwd_fn = src
        .find("fn effective_cwd(")
        .expect("effective_cwd missing");
    let ecwd_body = &src[ecwd_fn..ecwd_fn + 500];
    assert!(
        ecwd_body.contains("effective_cwd_checked("),
        "H1: effective_cwd must delegate to effective_cwd_checked"
    );

    // The checked variant enforces the project-root jail for an explicit cwd.
    let checked_fn = src
        .find("fn effective_cwd_checked(")
        .expect("effective_cwd_checked missing");
    let checked_body = &src[checked_fn..checked_fn + 500];
    assert!(
        checked_body.contains("jail_cwd(cwd, root)"),
        "H1: effective_cwd_checked must call jail_cwd for explicit cwd"
    );
}

#[test]
fn update_shell_cwd_calls_jail_path() {
    let src = include_str!("../../src/core/session/state.rs");

    let uscwd_fn = src
        .find("fn update_shell_cwd(")
        .expect("update_shell_cwd missing");
    let uscwd_body = &src[uscwd_fn..uscwd_fn + 600];
    assert!(
        uscwd_body.contains("jail_path("),
        "H1: update_shell_cwd must jail_path check before storing"
    );
}

// ---------------------------------------------------------------------------
// H2 — MCP ctx_read path has secret check
// ---------------------------------------------------------------------------
#[test]
fn resolve_path_includes_secret_check() {
    let src = include_str!("../../src/tools/server_paths.rs");

    let resolve_fn = src.find("fn resolve_path(").expect("resolve_path missing");
    let resolve_body = &src[resolve_fn..];
    let end = resolve_body
        .find("fn resolve_path_or_passthrough")
        .unwrap_or(resolve_body.len());
    let resolve_body = &resolve_body[..end];
    assert!(
        resolve_body.contains("check_secret_path"),
        "H2: resolve_path must call check_secret_path"
    );
}

// ---------------------------------------------------------------------------
// H6 — Pipeline archive redaction
// ---------------------------------------------------------------------------
#[test]
fn pipeline_archive_uses_redacted_output() {
    let src = crate::suite::security_hardening::server_dispatch_src();

    assert!(
        src.contains("redact_text_if_enabled"),
        "H6: shell archive must use redact_text_if_enabled before storing"
    );
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
    // The archive guard reads either way round depending on whether the block
    // is an expression or a statement; pin the property, not the spelling.
    assert!(
        src.contains("if minimal || is_raw_shell {")
            || src.contains("if !(minimal || is_raw_shell)"),
        "archiving must be skipped for raw shell"
    );
    assert!(
        src.contains("is_raw_shell")
            && src.contains("result_text.len() != pre_terse_len && output_tokens > 0")
            && src.contains("skip_terse"),
        "skip_terse must include is_raw_shell; post-terse stats must guard on terse delta"
    );
    // The verify_output guard may AND extra predicates after !is_raw_shell
    // (e.g. !firewalled), so accept both the bare guard and the extended form
    // rather than pinning a brittle exact literal.
    assert!(
        (src.contains("if !is_raw_shell {") || src.contains("if !is_raw_shell &&"))
            && src.contains("verify_output"),
        "output verification must be skipped for raw shell"
    );
    assert!(
        src.contains("!is_raw_shell && name == \"ctx_shell\""),
        "the ctx_shell post-processing block must be skipped for raw shell"
    );
}
