//! Source-level guards on the server: bounded regex and message sizes, raw
//! shell output kept out of every post-processing stage, and tools that answer
//! a missing context handle with an error instead of a panic.

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
// ReDoS guard in sobre_search
// ---------------------------------------------------------------------------
#[test]
fn sobre_search_has_pattern_length_limit() {
    let src = include_str!("../../src/tools/sobre_search.rs");

    assert!(
        src.contains("MAX_PATTERN_LEN"),
        "sobre_search must limit pattern length"
    );
    assert!(
        src.contains(".size_limit("),
        "sobre_search must set regex size_limit"
    );
    assert!(
        src.contains("RegexBuilder::new("),
        "sobre_search must use RegexBuilder (not Regex::new) for size limits"
    );
}

// ---------------------------------------------------------------------------
// MCP stdio max_length
// ---------------------------------------------------------------------------
#[test]
fn mcp_stdio_has_bounded_max_length() {
    let src = include_str!("../../src/mcp_stdio.rs");

    assert!(
        !src.contains("max_length: usize::MAX"),
        "MCP stdio must NOT use usize::MAX for max_length"
    );
    assert!(
        src.contains("32 * 1024 * 1024"),
        "MCP stdio max_length must be 32 MiB"
    );
}

// ---------------------------------------------------------------------------
// raw=true bypasses ALL post-processing in call_tool
// ---------------------------------------------------------------------------
#[test]
fn raw_shell_skips_all_postprocessing() {
    let src = crate::suite::security_hardening::server_dispatch_src().replace("\r\n", "\n");
    // The dispatch-level terse stage never sees shell output: sobre_shell
    // condenses its own, and only outside the raw branch.
    assert!(
        src.contains("const TERSE_TOOLS: [&str; 2] = [\"sobre_knowledge\", \"sobre_session\"];"),
        "the dispatch terse stage must not reach sobre_shell"
    );
    let shell = include_str!("../../src/tools/registered/sobre_shell.rs").replace("\r\n", "\n");
    let raw_branch = shell
        .find("if raw {")
        .expect("sobre_shell must branch on raw");
    let condense = shell
        .find("condense_shell_output(")
        .expect("sobre_shell must condense its own output");
    let else_branch = shell[raw_branch..]
        .find("} else {")
        .map(|i| raw_branch + i)
        .expect("the raw branch must have an else");
    assert!(
        condense > else_branch,
        "condensing must happen outside the raw branch"
    );
    assert!(
        src.contains("post_process::final_token_count(&result_text, pre_terse_len, output_tokens)")
            && src.contains("skip_terse"),
        "the sent count must be taken after terse"
    );
    // Verification only runs on text terse changed, and terse never touches
    // raw shell output.
    assert!(
        src.contains("result_text != pre_compression") && src.contains("verify_output"),
        "output verification must only run on condensed text"
    );
}

// ---------------------------------------------------------------------------
// The optional handles of ToolContext are never unwrapped by a tool
// ---------------------------------------------------------------------------
#[test]
fn tools_never_unwrap_an_optional_context_handle() {
    let unwrap = regex::Regex::new(r"ctx\.\w+\.as_ref\(\)\.(unwrap|expect)\(").unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tools/registered");
    let mut scanned = 0;
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("src/tools/registered exists") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        scanned += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        for m in unwrap.find_iter(&text) {
            hits.push(format!("{}: {}", path.display(), m.as_str()));
        }
    }
    assert!(scanned > 0, "no tool sources found under {}", dir.display());
    assert!(
        hits.is_empty(),
        "a tool unwraps an optional ToolContext handle:\n{}",
        hits.join("\n")
    );
}
