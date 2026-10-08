//! Tests for `sobre_read`. Extracted from `sobre_read/mod.rs`;
//! `super::*` resolves to the `sobre_read` module.

use super::*;

#[test]
fn test_header_toon_format_no_brackets() {
    let content = "use std::io;\nfn main() {}\n";
    let header = build_header("main.rs", "rs", content, 2, false);
    assert!(!header.contains('['));
    assert!(!header.contains(']'));
    assert!(header.contains("main.rs 2L"));
}

#[test]
fn test_header_toon_deps_indented() {
    let content = "use crate::core::cache;\nuse crate::tools;\npub fn main() {}\n";
    let header = build_header("main.rs", "rs", content, 3, true);
    if header.contains("deps") {
        assert!(
            header.contains("\n deps "),
            "deps should use indented TOON format"
        );
        assert!(
            !header.contains("deps:["),
            "deps should not use bracket format"
        );
    }
}

#[test]
fn test_header_toon_saves_tokens() {
    let content = "use crate::foo;\nuse crate::bar;\npub fn baz() {}\npub fn qux() {}\n";
    let old_header = "F1=main.rs [4L +] deps:[foo,bar] exports:[baz,qux]".to_string();
    let new_header = build_header("main.rs", "rs", content, 4, true);
    let old_tokens = count_tokens(&old_header);
    let new_tokens = count_tokens(&new_header);
    assert!(
        new_tokens <= old_tokens,
        "TOON header ({new_tokens} tok) should be <= old format ({old_tokens} tok)"
    );
}

#[test]
fn test_tdd_symbols_are_compact() {
    let symbols = [
        "⊕", "⊖", "∆", "→", "⇒", "✓", "✗", "⚠", "λ", "§", "∂", "τ", "ε",
    ];
    for sym in &symbols {
        let tok = count_tokens(sym);
        assert!(tok <= 2, "Symbol {sym} should be 1-2 tokens, got {tok}");
    }
}

#[test]
fn test_task_mode_filters_content() {
    // #840: file must exceed TASK_MIN_LINES (250) to engage filtering.
    let content = (0..300)
        .map(|i| {
            if i % 20 == 0 {
                format!("fn validate_token(token: &str) -> bool {{ /* line {i} */ }}")
            } else {
                format!("fn unrelated_helper_{i}(x: i32) -> i32 {{ x + {i} }}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let full_tokens = count_tokens(&content);
    let task = Some("fix bug in validate_token");
    let (result, result_tokens) = process_mode(
        &content,
        "task",
        "test.rs",
        "rs",
        full_tokens,
        CrpMode::Off,
        "test.rs",
        task,
    );
    assert!(
        result_tokens < full_tokens,
        "task mode ({result_tokens} tok) should be less than full ({full_tokens} tok)"
    );
    assert!(
        result.contains("task-filtered"),
        "output should contain task-filtered marker"
    );
}

#[test]
fn test_task_mode_without_task_returns_full() {
    let content = "fn main() {}\nfn helper() {}\n";
    let tokens = count_tokens(content);
    let (result, _sent) = process_mode(
        content,
        "task",
        "test.rs",
        "rs",
        tokens,
        CrpMode::Off,
        "test.rs",
        None,
    );
    assert!(
        result.contains("no task set"),
        "should indicate no task: {result}"
    );
}

#[test]
fn test_reference_mode_one_line() {
    let content = "fn main() {}\nfn helper() {}\nfn other() {}\n";
    let tokens = count_tokens(content);
    let (result, _sent) = process_mode(
        content,
        "reference",
        "test.rs",
        "rs",
        tokens,
        CrpMode::Off,
        "test.rs",
        None,
    );
    let lines: Vec<&str> = result.lines().collect();
    assert!(
        lines.len() <= 3,
        "reference mode should be very compact, got {} lines",
        lines.len()
    );
    assert!(result.contains("lines"), "should contain line count");
    assert!(result.contains("tok"), "should contain token count");
}

#[test]
fn cached_lines_mode_invalidates_on_mtime_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("file.txt");
    let p = path.to_string_lossy().to_string();

    std::fs::write(&path, "one\nsecond\n").unwrap();
    let mut cache = SessionCache::new();

    let r1 = handle_with_task_resolved(&mut cache, &p, "lines:1-1", CrpMode::Off, None);
    let l1: Vec<&str> = r1.content.lines().collect();
    let got1 = l1.get(1).copied().unwrap_or_default().trim();
    let got1 = got1.split_once('|').map_or(got1, |(_, s)| s.trim());
    assert_eq!(got1, "one");

    crate::test_env::write_newer(&path, "two\nsecond\n");

    let r2 = handle_with_task_resolved(&mut cache, &p, "lines:1-1", CrpMode::Off, None);
    let l2: Vec<&str> = r2.content.lines().collect();
    let got2 = l2.get(1).copied().unwrap_or_default().trim();
    let got2 = got2.split_once('|').map_or(got2, |(_, s)| s.trim());
    assert_eq!(got2, "two");
}

#[test]
fn try_stub_hit_readonly_none_for_uncached_and_stale() {
    let _lock = crate::core::data_dir::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hot.rs");
    let p = path.to_string_lossy().to_string();
    std::fs::write(&path, "fn main() {}\n").unwrap();

    let mut cache = SessionCache::new();

    // Never read → no entry → the read-locked path declines (caller falls back).
    assert!(try_stub_hit_readonly(&cache, &p).is_none());

    // Populate the cache via a real full read.
    let _ = handle_with_task_resolved(&mut cache, &p, "full", CrpMode::Off, None);

    // Modify the file so the cached entry is stale → must NOT serve a stub,
    // independent of cache policy (the write path re-reads instead).
    crate::test_env::write_newer(&path, "fn main() { changed(); }\n");
    assert!(
        try_stub_hit_readonly(&cache, &p).is_none(),
        "stale file must never be served from the read-locked stub path"
    );
}

#[test]
#[cfg_attr(tarpaulin, ignore)]
fn benchmark_task_conditioned_compression() {
    // Keep this reasonably small so CI coverage instrumentation stays fast.
    let content = generate_benchmark_code(300);
    let full_tokens = count_tokens(&content);
    let task = Some("fix authentication in validate_token");

    let (_full_output, full_tok) = process_mode(
        &content,
        "full",
        "server.rs",
        "rs",
        full_tokens,
        CrpMode::Off,
        "server.rs",
        task,
    );
    let (_task_output, task_tok) = process_mode(
        &content,
        "task",
        "server.rs",
        "rs",
        full_tokens,
        CrpMode::Off,
        "server.rs",
        task,
    );
    let (_sig_output, sig_tok) = process_mode(
        &content,
        "signatures",
        "server.rs",
        "rs",
        full_tokens,
        CrpMode::Off,
        "server.rs",
        task,
    );
    let (_ref_output, ref_tok) = process_mode(
        &content,
        "reference",
        "server.rs",
        "rs",
        full_tokens,
        CrpMode::Off,
        "server.rs",
        task,
    );

    eprintln!("\n=== Task-Conditioned Compression Benchmark ===");
    eprintln!("Source: 200-line Rust file, task='fix authentication in validate_token'");
    eprintln!("  full:       {full_tok:>6} tokens (baseline)");
    eprintln!(
        "  task:       {task_tok:>6} tokens ({:.0}% savings)",
        (1.0 - task_tok as f64 / full_tok as f64) * 100.0
    );
    eprintln!(
        "  signatures: {sig_tok:>6} tokens ({:.0}% savings)",
        (1.0 - sig_tok as f64 / full_tok as f64) * 100.0
    );
    eprintln!(
        "  reference:  {ref_tok:>6} tokens ({:.0}% savings)",
        (1.0 - ref_tok as f64 / full_tok as f64) * 100.0
    );
    eprintln!("================================================\n");

    assert!(task_tok < full_tok, "task mode should save tokens");
    assert!(sig_tok < full_tok, "signatures should save tokens");
    assert!(ref_tok < sig_tok, "reference should be most compact");
}

fn generate_benchmark_code(lines: usize) -> String {
    let mut code = Vec::with_capacity(lines);
    code.push("use std::collections::HashMap;".to_string());
    code.push("use crate::core::auth;".to_string());
    code.push(String::new());
    code.push("pub struct Server {".to_string());
    code.push("    config: Config,".to_string());
    code.push("    cache: HashMap<String, String>,".to_string());
    code.push("}".to_string());
    code.push(String::new());
    code.push("impl Server {".to_string());
    code.push(
        "    pub fn validate_token(&self, token: &str) -> Result<Claims, AuthError> {".to_string(),
    );
    code.push("        let decoded = auth::decode_jwt(token)?;".to_string());
    code.push("        if decoded.exp < chrono::Utc::now().timestamp() {".to_string());
    code.push("            return Err(AuthError::Expired);".to_string());
    code.push("        }".to_string());
    code.push("        Ok(decoded.claims)".to_string());
    code.push("    }".to_string());
    code.push(String::new());

    let remaining = lines.saturating_sub(code.len());
    for i in 0..remaining {
        if i % 30 == 0 {
            code.push(format!(
                "    pub fn handler_{i}(&self, req: Request) -> Response {{"
            ));
        } else if i % 30 == 29 {
            code.push("    }".to_string());
        } else {
            code.push(format!("        let val_{i} = self.cache.get(\"key_{i}\").unwrap_or(&\"default\".to_string());"));
        }
    }
    code.push("}".to_string());
    code.join("\n")
}

#[test]
fn map_mode_inlines_task_relevant_body() {
    let content = "pub fn alpha() {\n    let a = 1;\n}\n\npub fn validate_token(t: &str) -> bool {\n    let ok = check(t);\n    ok\n}\n";
    let tokens = count_tokens(content);
    let (with_task, _) = process_mode(
        content,
        "map",
        "test.rs",
        "rs",
        tokens,
        CrpMode::Off,
        "test.rs",
        Some("fix bug in validate_token"),
    );
    assert!(
        with_task.contains("▸ body") && with_task.contains("validate_token"),
        "map with task should inline the matching body: {with_task}"
    );
    let (no_task, _) = process_mode(
        content,
        "map",
        "test.rs",
        "rs",
        tokens,
        CrpMode::Off,
        "test.rs",
        None,
    );
    assert!(
        !no_task.contains("▸ body"),
        "map without a task must not inline a body: {no_task}"
    );
}

// `lines:5,10-12` is multi-select (comma-separated lines/ranges), not a range.
// Callers who meant a span get stray lines back — the output must say what
// the comma did so the mistake is visible (limitations audit 2026-07-03, #7).
#[test]
fn lines_comma_multiselect_gets_hint() {
    let content = (1..=30)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let tokens = count_tokens(&content);
    let (out, _) = process_mode(
        &content,
        "lines:5,10-12",
        "t.txt",
        "txt",
        tokens,
        CrpMode::Off,
        "t.txt",
        None,
    );
    assert!(out.contains("line 5") && out.contains("line 10"), "{out}");
    assert!(
        out.contains("multi-select"),
        "comma form must carry a hint: {out}"
    );
}

// A map/signatures read of a language with no extractor must say so instead of
// returning a bare header the caller mistakes for "file has no API"
// (limitations audit 2026-07-03, #4 residual).
#[test]
fn map_on_unsupported_language_carries_marker() {
    let content = "some plain text\nwith no code\nstructure at all\n";
    let tokens = count_tokens(content);
    let (out, _) = process_mode(
        content,
        "map",
        "notes.txt",
        "txt",
        tokens,
        CrpMode::Off,
        "notes.txt",
        None,
    );
    assert!(
        out.contains("no extractable structure"),
        "empty map must carry a marker: {out}"
    );
}

#[test]
fn signatures_on_unsupported_language_carries_marker() {
    let content = "some plain text\nwith no code\nstructure at all\n";
    let tokens = count_tokens(content);
    let (out, _) = process_mode(
        content,
        "signatures",
        "notes.txt",
        "txt",
        tokens,
        CrpMode::Off,
        "notes.txt",
        None,
    );
    assert!(
        out.contains("no extractable structure"),
        "empty signatures must carry a marker: {out}"
    );
}

// UTF-8 BOM must not leak into the first line of sobre_read output
// (limitations doc #11).
#[test]
fn read_file_lossy_strips_utf8_bom() {
    let p = std::env::temp_dir().join("sobre_bom_test.txt");
    std::fs::write(&p, b"\xEF\xBB\xBFhello\n").unwrap();
    let s = read_file_lossy(p.to_str().unwrap()).unwrap();
    let _ = std::fs::remove_file(&p);
    assert!(
        !s.starts_with('\u{feff}'),
        "BOM must be stripped from read content"
    );
    assert!(
        s.starts_with("hello"),
        "content after BOM must survive: {s}"
    );
}

#[test]
fn compressed_cache_key_distinguishes_task() {
    let no_task = compressed_cache_key("map", CrpMode::Off, None, None, &[]);
    let tdd_no_task = compressed_cache_key("map", CrpMode::Tdd, None, None, &[]);
    let with_task = compressed_cache_key("map", CrpMode::Off, Some("fix login"), None, &[]);
    let other_task = compressed_cache_key("map", CrpMode::Off, Some("refactor db"), None, &[]);
    assert_eq!(no_task, "map");
    assert_eq!(tdd_no_task, "map:tdd");
    assert_ne!(with_task, no_task);
    assert_ne!(with_task, other_task);
}

#[test]
fn compressed_cache_key_distinguishes_aggressiveness() {
    // None → byte-identical to today's keys (#714 must not shift existing cache).
    let base = compressed_cache_key("map", CrpMode::Off, None, None, &[]);
    assert_eq!(base, "map");
    // Same aggressiveness → same key (determinism, #498).
    let a = compressed_cache_key("map", CrpMode::Off, None, Some(0.7), &[]);
    assert_eq!(
        a,
        compressed_cache_key("map", CrpMode::Off, None, Some(0.7), &[])
    );
    // Distinct buckets → distinct keys; jitter inside a 0.05 bucket collapses.
    assert_ne!(a, base);
    assert_ne!(
        a,
        compressed_cache_key("map", CrpMode::Off, None, Some(0.2), &[])
    );
    assert_eq!(
        a,
        compressed_cache_key("map", CrpMode::Off, None, Some(0.701), &[])
    );
}

#[test]
fn compressed_cache_key_distinguishes_protect() {
    // Empty protect → byte-identical to today's keys (#720 must not shift cache).
    let base = compressed_cache_key("entropy", CrpMode::Off, None, None, &[]);
    assert_eq!(base, "entropy");
    // A non-empty protect list changes the key (lossy output differs, #498)…
    let p = compressed_cache_key("entropy", CrpMode::Off, None, None, &["TODO".to_string()]);
    assert_ne!(p, base);
    // …deterministically, and independent of token order / duplicates.
    assert_eq!(
        p,
        compressed_cache_key("entropy", CrpMode::Off, None, None, &["TODO".to_string()])
    );
    let multi_a = compressed_cache_key(
        "entropy",
        CrpMode::Off,
        None,
        None,
        &["a".to_string(), "b".to_string()],
    );
    let multi_b = compressed_cache_key(
        "entropy",
        CrpMode::Off,
        None,
        None,
        &["b".to_string(), "a".to_string(), "a".to_string()],
    );
    assert_eq!(multi_a, multi_b);
    assert_ne!(multi_a, p);
}

#[test]
fn aggressiveness_is_deterministic_and_monotonic() {
    let _iso = crate::core::data_dir::isolated_data_dir();
    // The savings footer is off (it would carry session-cumulative counters),
    // so we compare the pure compressed body.
    // Prose-y fixture with redundant low-information lines the density pass can
    // shed; enough lines that compression is meaningful.
    let mut content = String::new();
    for i in 0..60 {
        content.push_str(&format!(
            "line {i}: the quick brown fox jumps over the lazy dog\n"
        ));
    }
    let render_at = |a: f64| -> String {
        // Bare `density:` exercises the aggressiveness-target fallback (#714).
        let (out, _) = process_mode_tuned(
            &content,
            "density:",
            "f.txt",
            "txt",
            count_tokens(&content),
            CrpMode::Off,
            "/tmp/f.txt",
            None,
            ReadTuning {
                aggressiveness: Some(a),
                protect: &[],
            },
        );
        out
    };
    // Determinism (#498): same aggressiveness → byte-identical output. Guards the
    // canonical-order entropy summation fix in `token_entropy_from_ids`.
    assert_eq!(render_at(0.7), render_at(0.7));
    // Monotonic: more aggressive keeps no more tokens than less aggressive.
    let low = count_tokens(&render_at(0.2));
    let high = count_tokens(&render_at(0.9));
    assert!(
        high <= low,
        "aggressiveness 0.9 ({high} tok) must not exceed 0.2 ({low} tok)"
    );
}

#[test]
fn aggressive_json_uses_lossless_crush_core() {
    let _iso = crate::core::data_dir::isolated_data_dir();

    // A redundant array-of-objects JSON file: aggressive mode compacts it through
    // the shared json_crush core (#936) instead of generic text pruning, which
    // would mangle the structure. Constant columns + many rows so it halves.
    let items: Vec<String> = (0..40)
        .map(|i| {
            format!(r#"{{"status":"active","region":"eu-central-1","tier":"standard","id":{i}}}"#)
        })
        .collect();
    let content = format!("[{}]", items.join(","));
    let original = count_tokens(&content);

    let (out, sent) = process_mode_tuned(
        &content,
        "aggressive",
        "data.json",
        "json",
        original,
        CrpMode::Off,
        "/tmp/data.json",
        None,
        ReadTuning {
            aggressiveness: None,
            protect: &[],
        },
    );

    assert!(
        out.contains("_lc_crush"),
        "aggressive json must compact via the crush core: {out}"
    );
    assert!(
        sent < original,
        "crush must reduce tokens ({sent} >= {original})"
    );
}

#[test]
fn map_mode_includes_signature_line_ranges() {
    // Map formatting is rendered by `process_mode`; assert it directly so the
    // structure check stays independent of the handle-level #361 cap, which
    // legitimately collapses this tiny fixture to raw.
    let content = "pub struct Config {}\n\npub fn build() -> Config { Config {} }\n";
    let (result, _) = process_mode(
        content,
        "map",
        "lib.rs",
        "rs",
        count_tokens(content),
        CrpMode::Off,
        "/tmp/lib.rs",
        None,
    );

    assert!(
        result.contains("API:"),
        "map output should include API: {result}"
    );
    assert!(
        result.contains("struct pub Config @L1"),
        "struct signature should include line suffix: {result}"
    );
    assert!(
        result.contains("fn pub build() → Config @L3"),
        "function signature should include line suffix: {result}"
    );
}

#[test]
fn map_mode_omits_exports_already_in_api() {
    // #361 follow-up: the `exports:` line duplicated symbols the API section
    // already lists with full signatures + line ranges. Map must not repeat
    // exports that the API already covers (pure redundant tokens). Rendered by
    // `process_mode`; assert it directly (handle would cap this tiny fixture).
    let content = "pub struct Config {}\n\npub fn build() -> Config { Config {} }\n";
    let (result, _) = process_mode(
        content,
        "map",
        "lib.rs",
        "rs",
        count_tokens(content),
        CrpMode::Off,
        "/tmp/lib.rs",
        None,
    );

    // Both exported symbols stay discoverable via the API section …
    assert!(
        result.contains("struct pub Config") && result.contains("fn pub build"),
        "API section must still list exported symbols: {result}"
    );
    // … and the redundant `exports:` line is gone (both are in the API).
    assert!(
        !result.contains("exports:"),
        "map must not repeat exports already shown in API: {result}"
    );
}

#[test]
fn tdd_map_output_carries_symbol_legend() {
    // GL #580: symbol notation must be self-describing for vanilla agents.
    // Rendered by `process_mode`; assert it directly (handle caps this fixture).
    let content = "pub struct Config {}\n\npub fn build() -> Config { Config {} }\n";
    let (result, _) = process_mode(
        content,
        "map",
        "lib.rs",
        "rs",
        count_tokens(content),
        CrpMode::Tdd,
        "/tmp/lib.rs",
        None,
    );
    assert!(
        result.contains("[λ=fn §=class +=pub]"),
        "TDD map output must carry the symbol legend: {result}"
    );

    let (sigs, _) = process_mode(
        content,
        "signatures",
        "lib.rs",
        "rs",
        count_tokens(content),
        CrpMode::Tdd,
        "/tmp/lib.rs",
        None,
    );
    assert!(
        sigs.contains("[λ=fn §=class +=pub]"),
        "TDD signatures output must carry the symbol legend: {sigs}"
    );
}

#[test]
fn instruction_file_detection() {
    assert!(is_instruction_file(
        "/home/user/.pi/agent/skills/committing-changes/SKILL.md"
    ));
    assert!(is_instruction_file("/workspace/.cursor/rules/team.mdc"));
    assert!(is_instruction_file("/project/AGENTS.md"));
    assert!(is_instruction_file("/project/.cursorrules"));
    assert!(is_instruction_file("/home/user/.claude/rules/my-rule.md"));
    assert!(is_instruction_file("/skills/some-skill/README.md"));

    assert!(!is_instruction_file("/project/src/main.rs"));
    assert!(!is_instruction_file("/project/config.json"));
    assert!(!is_instruction_file("/project/data/report.csv"));
}

#[test]
fn resolve_auto_mode_returns_full_for_instruction_files() {
    let mode = resolve_auto_mode(
        None,
        "/home/user/.pi/agent/skills/committing-changes/SKILL.md",
        5000,
        Some("read"),
    );
    assert_eq!(mode, "full", "SKILL.md must always be read in full");

    let mode = resolve_auto_mode(None, "/workspace/AGENTS.md", 3000, Some("read"));
    assert_eq!(mode, "full", "AGENTS.md must always be read in full");

    let mode = resolve_auto_mode(None, "/workspace/.cursorrules", 2000, None);
    assert_eq!(mode, "full", ".cursorrules must always be read in full");
}

/// Phase 1a (epic #1008): `mode=anchored` returns each source line as a
/// `N:hh|content` anchor the model can edit against via `sobre_patch`, plus a
/// self-describing legend. End-to-end through the real read pipeline.
#[test]
fn anchored_mode_emits_line_hash_anchors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("anc.rs");
    let p = path.to_string_lossy().to_string();
    let content = "fn main() {\n    let x = 1;\n}\n";
    std::fs::write(&path, content).unwrap();

    let mut cache = SessionCache::new();
    let r = handle_with_task_resolved(&mut cache, &p, "anchored", CrpMode::Off, None);
    assert_eq!(r.resolved_mode, "anchored");
    assert!(
        r.content.contains("[anchored:"),
        "anchored output must carry the self-describing legend: {}",
        r.content
    );

    // Every source line appears as `N:hh|<line>` with the SSOT anchor hash.
    for (i, line) in content.lines().enumerate() {
        let n = i + 1;
        let expected = format!("{n}:{}|{line}", crate::core::anchor::line_hash(line));
        assert!(
            r.content.contains(&expected),
            "missing anchor for line {n}: expected `{expected}` in:\n{}",
            r.content
        );
    }
}

/// Anchored mode is lossless, so the #361 raw cap must never strip the anchors
/// on a small file (it opts out of the cap) — the agent always gets editable
/// anchors back.
#[test]
fn anchored_mode_is_not_capped_to_raw_on_small_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tiny.rs");
    let p = path.to_string_lossy().to_string();
    std::fs::write(&path, "a\n").unwrap();

    let mut cache = SessionCache::new();
    let r = handle_with_task_resolved(&mut cache, &p, "anchored", CrpMode::Off, None);
    assert!(
        r.content.contains("|a"),
        "anchored output must keep anchors even on a tiny file: {}",
        r.content
    );
    assert!(r.content.contains("[anchored:"), "legend must survive");
}

#[test]
fn raw_mode_returns_exact_file_content() {
    let _lock = crate::core::data_dir::test_env_lock();
    let content = "fn main() {\n    println!(\"hello\");\n}\n";
    let (output, _sent) = render::process_mode(
        content,
        "raw",
        "main.rs",
        "rs",
        100,
        CrpMode::Off,
        "/tmp/main.rs",
        None,
    );
    assert_eq!(
        output, content,
        "raw mode must return exact file content with zero overhead"
    );
    assert!(
        !output.contains("main.rs"),
        "raw mode must not contain filename header"
    );
    assert!(!output.contains("deps"), "raw mode must not contain deps");
}

/// Regression for GH #628: the verbatim views (`full`, `raw`, `lines:N-M`) must
/// reproduce every source line — including decorative separator comments
/// (`// ————`, `// ----`) — so the content the model edits never diverges from
/// disk. The original report saw these silently stripped, which then broke
/// `sobre_edit` on a whitespace mismatch.
#[test]
fn verbatim_modes_preserve_decorative_comment_lines() {
    let _lock = crate::core::data_dir::test_env_lock();
    let sep_em = "// ————————————————————————————————————————";
    let sep_dash = "// ------------------------------------------";
    let content = format!(
        "import {{ describe, it, expect }} from \"vitest\";\n\
         \n\
         {sep_em}\n\
         // Section: arithmetic\n\
         {sep_em}\n\
         describe(\"add\", () => {{\n  it(\"adds\", () => expect(1 + 1).toBe(2));\n}});\n\
         \n\
         {sep_dash}\n\
         // Section: strings\n\
         {sep_dash}\n"
    );

    for mode in ["full", "raw"] {
        let (output, _) = render::process_mode(
            &content,
            mode,
            "math.test.ts",
            "ts",
            count_tokens(&content),
            CrpMode::Off,
            "/tmp/math.test.ts",
            None,
        );
        assert!(
            output.contains(sep_em) && output.contains(sep_dash),
            "{mode} mode must keep every separator comment verbatim:\n{output}"
        );
        // Every source line is present (modes may add a header/footer, never drop).
        for line in content.lines().filter(|l| !l.is_empty()) {
            assert!(
                output.contains(line),
                "{mode} mode dropped a source line: {line:?}"
            );
        }
    }

    // A `lines:` window must keep separators too, with original line numbering.
    let window = render::extract_line_range(&content, "1-5");
    assert!(
        window.contains(sep_em),
        "lines: window dropped the separator comment:\n{window}"
    );
    assert!(
        window.contains("   3| ") && window.contains("   5| "),
        "lines: window must number the separator lines (3 and 5):\n{window}"
    );
}

/// Determinism contract (#498): tool output must be a pure function of
/// (content, mode, crp_mode, task). Timestamps, counters or random hints in
/// the body would make otherwise-identical outputs unique and defeat
/// provider-side prompt caching.
#[test]
fn process_mode_output_is_byte_stable_across_calls() {
    // Fresh, empty data dir: session state left by parallel tests must not
    // leak into the output.
    let _iso = crate::core::data_dir::isolated_data_dir();
    let content: String = (0..120)
        .map(|i| format!("pub fn handler_{i}(x: u32) -> u32 {{ x * {i} }}"))
        .collect::<Vec<_>>()
        .join("\n");
    let tokens = count_tokens(&content);

    for mode in [
        "map",
        "signatures",
        "reference",
        "aggressive",
        "entropy",
        "raw",
        "lines:5-20",
        "anchored",
    ] {
        let run = || {
            render::process_mode(
                &content,
                mode,
                "stable.rs",
                "rs",
                tokens,
                CrpMode::Off,
                "/tmp/stable.rs",
                None,
            )
            .0
        };
        let first = run();
        let second = run();
        assert_eq!(
            first, second,
            "mode '{mode}' produced non-deterministic output"
        );
    }
}

/// The reactive recovery footer (#premium-recovery): present on compressed views,
/// leading with the MCP-free native path; absent from verbatim views; and
/// byte-stable across calls (#498).
#[test]
fn recovery_footer_is_compressed_only() {
    // `isolated_data_dir()` already holds `test_env_lock` for its lifetime; taking
    // the lock again here would self-deadlock (the mutex is non-reentrant).
    let _iso = crate::core::data_dir::isolated_data_dir();
    let content: String = (0..120)
        .map(|i| format!("pub fn handler_{i}(x: u32) -> u32 {{ x * {i} }}"))
        .collect::<Vec<_>>()
        .join("\n");
    let tokens = count_tokens(&content);
    let run = |mode: &str| {
        render::process_mode(
            &content,
            mode,
            "rec.rs",
            "rs",
            tokens,
            CrpMode::Off,
            "/tmp/rec.rs",
            None,
        )
        .0
    };

    // Minimal tier: a compressed view leads its footer with the native,
    // MCP-free path so an agent needing the full source never reads line-by-line.
    let sigs = run("signatures");
    assert!(
        sigs.contains("full source in \"/tmp/rec.rs\" (read it directly)"),
        "compressed view must surface the MCP-free recovery path: {sigs}"
    );
    // Determinism (#498): byte-stable across calls.
    assert_eq!(sigs, run("signatures"), "footer must be byte-stable");

    // The verbatim escape hatch itself carries no footer (nothing to recover).
    assert!(
        !run("raw").contains("(read it directly)"),
        "raw view needs no recovery footer"
    );
}

// ---------------------------------------------------------------------------
// #361 anti-inflation invariant: a `sobre_read` must never cost more tokens than
// the raw file. The framing header only earns its keep on large files and
// cached re-reads; on a cold read of a small file it is pure overhead, so the
// guard ships bare content (break-even, never a loss). The guard now applies to
// every mode — auto-resolved AND explicitly requested — so no view can ever
// cost more tokens than reading the file raw.
// ---------------------------------------------------------------------------

#[test]
fn cap_to_raw_falls_back_when_framing_inflates() {
    let raw = "pub fn a() {}\n";
    let framed = format!("F1=x.rs 1L\n deps foo,bar\n{raw}");
    let raw_tokens = count_tokens(raw);
    let framed_tokens = count_tokens(&framed);
    assert!(
        framed_tokens > raw_tokens,
        "fixture must inflate to exercise the guard"
    );
    assert_eq!(
        cap_to_raw(framed, framed_tokens, raw, raw_tokens, "full"),
        raw,
        "framing larger than raw must fall back to bare content"
    );
}

#[test]
fn cap_to_raw_keeps_framing_when_not_larger() {
    let raw = "a long original body that compresses well";
    let framed = "sig summary".to_string();
    let framed_tokens = count_tokens(&framed);
    assert_eq!(
        cap_to_raw(framed.clone(), framed_tokens, raw, 100, "map"),
        framed,
        "output at or below raw must be returned untouched"
    );
}

#[test]
fn cap_to_raw_keeps_framing_for_empty_file() {
    // An empty file has zero content tokens; keep the framing so the reader
    // still gets an "empty / 0L" signal rather than a blank payload.
    let framed = "F1=empty.rs 0L".to_string();
    let framed_tokens = count_tokens(&framed);
    assert_eq!(
        cap_to_raw(framed.clone(), framed_tokens, "", 0, "map"),
        framed,
        "empty files keep their framing signal"
    );
}

#[test]
fn auto_read_never_inflates_small_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.rs");
    let p = path.to_string_lossy().to_string();
    let content =
        "use std::io;\n\npub fn greet(name: &str) -> String {\n    format!(\"hi {name}\")\n}\n";
    std::fs::write(&path, content).unwrap();

    let mut cache = SessionCache::new();
    let out = handle_with_task_resolved(&mut cache, &p, "auto", CrpMode::Off, None);
    assert!(
        out.output_tokens <= count_tokens(content),
        "auto cold read inflated a small file: {} output tok > {} raw tok\n{}",
        out.output_tokens,
        count_tokens(content),
        out.content
    );
}

#[test]
fn full_read_never_inflates_tiny_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tiny.rs");
    let p = path.to_string_lossy().to_string();
    let content = "pub fn a() {}\n";
    std::fs::write(&path, content).unwrap();

    let mut cache = SessionCache::new();
    let out = handle_with_task_resolved(&mut cache, &p, "full", CrpMode::Off, None);
    assert!(
        out.output_tokens <= count_tokens(content),
        "full cold read inflated a tiny file: {} > {}\n{}",
        out.output_tokens,
        count_tokens(content),
        out.content
    );
}

#[test]
fn auto_read_still_compresses_large_file() {
    // Isolate learning state so the resolver falls through to the size
    // heuristic (large code file → map), proving the guard never blocks a
    // genuine compression win.
    let _iso = crate::core::data_dir::isolated_data_dir();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.rs");
    let p = path.to_string_lossy().to_string();
    let mut content = String::new();
    for i in 0..400 {
        content.push_str(&format!(
            "pub fn function_number_{i}(x: i32, y: i32) -> i32 {{\n    let z = x + y + {i};\n    z * 2\n}}\n\n"
        ));
    }
    std::fs::write(&path, &content).unwrap();

    let mut cache = SessionCache::new();
    let out = handle_with_task_resolved(&mut cache, &p, "auto", CrpMode::Off, None);
    assert!(
        out.output_tokens < count_tokens(&content),
        "auto read of a large file must still compress: {} >= {} (mode={})",
        out.output_tokens,
        count_tokens(&content),
        out.resolved_mode
    );
}

#[test]
fn explicit_compressed_mode_capped_on_tiny_file() {
    // #361 applies to explicit modes too: `signatures` of a tiny file costs
    // more than the file, so the file is sent instead, labelled as such.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lib.rs");
    let p = path.to_string_lossy().to_string();
    let content = "pub fn alpha() {}\npub fn beta() {}\n";
    std::fs::write(&path, content).unwrap();

    let mut cache = SessionCache::new();
    let out = handle_with_task_resolved(&mut cache, &p, "signatures", CrpMode::Off, None);
    // The file itself, with the one line that says the view was replaced.
    assert_eq!(
        out.content,
        format!("[signatures view is larger than the file: whole file below]\n{content}")
    );
}

#[test]
fn explicit_signatures_still_compresses_large_file() {
    // Capping explicit modes must not break legitimate compression: signatures
    // of a large file are far smaller than raw, so the cap is a no-op and the
    // compressed view (not raw) is returned.
    let _iso = crate::core::data_dir::isolated_data_dir();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.rs");
    let p = path.to_string_lossy().to_string();
    let mut content = String::new();
    for i in 0..400 {
        content.push_str(&format!(
            "pub fn function_number_{i}(x: i32, y: i32) -> i32 {{\n    let z = x + y + {i};\n    z * 2\n}}\n\n"
        ));
    }
    std::fs::write(&path, &content).unwrap();

    let mut cache = SessionCache::new();
    let out = handle_with_task_resolved(&mut cache, &p, "signatures", CrpMode::Off, None);
    assert!(
        out.output_tokens < count_tokens(&content),
        "explicit signatures of a large file must compress: {} >= {}",
        out.output_tokens,
        count_tokens(&content)
    );
}

#[test]
fn cache_hit_stub_is_byte_stable_across_rereads() {
    // #498 determinism: re-reading an unchanged file must yield byte-identical
    // output (no read-count note, no rotating proof line) so provider prompt
    // caching applies to the repeated stub.
    let _iso = crate::core::data_dir::isolated_data_dir();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stable.rs");
    let p = path.to_string_lossy().to_string();
    std::fs::write(&path, "pub fn alpha() {}\npub fn beta() {}\n").unwrap();

    let mut cache = SessionCache::new();
    // Prime: the first full read marks full content as delivered.
    let _ = handle_with_task_resolved(&mut cache, &p, "full", CrpMode::Off, None);
    let r2 = handle_with_task_resolved(&mut cache, &p, "full", CrpMode::Off, None);
    let r3 = handle_with_task_resolved(&mut cache, &p, "full", CrpMode::Off, None);
    let r4 = handle_with_task_resolved(&mut cache, &p, "full", CrpMode::Off, None);
    assert_eq!(
        r2.content, r3.content,
        "re-read drifted between reads 2 and 3"
    );
    assert_eq!(
        r3.content, r4.content,
        "re-read drifted between reads 3 and 4"
    );
    assert!(
        !r2.content.contains("(read"),
        "read-count note must not appear in the cache-hit body: {}",
        r2.content
    );
}

// ---------------------------------------------------------------------------

// -- Regression: GitHub Issue #775 --
// After a full-file read is cached, a subsequent ranged read with `lines:N-M`
// must return only the requested window — not the full file content again.

/// Helper: create a test file with `n` numbered lines ("line 1\nline 2\n…").
fn write_numbered_file(dir: &std::path::Path, name: &str, n: usize) -> String {
    let path = dir.join(name);
    let content: String = (1..=n)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, &content).unwrap();
    path.to_string_lossy().to_string()
}

#[test]
fn gh775_full_then_ranged_returns_only_window() {
    let _iso = crate::core::data_dir::isolated_data_dir();

    let dir = tempfile::tempdir().unwrap();
    let p = write_numbered_file(dir.path(), "big.ts", 2000);

    let mut cache = SessionCache::new();

    // 1. Full read — delivers all 2000 lines, marks as fully delivered.
    let full = handle_with_task_resolved(&mut cache, &p, "full", CrpMode::Off, None);
    assert!(
        full.content.contains("line 1"),
        "full read must include first line"
    );
    assert!(
        full.content.contains("line 2000"),
        "full read must include last line"
    );

    // 2. Ranged read — must return ONLY lines 1480–1489.
    let ranged =
        handle_fresh_with_task_resolved(&mut cache, &p, "lines:1480-1489", CrpMode::Off, None);
    assert!(
        ranged.content.contains("line 1480"),
        "ranged read must contain the first requested line:\n{}",
        &ranged.content[..ranged.content.len().min(300)]
    );
    assert!(
        ranged.content.contains("line 1489"),
        "ranged read must contain the last requested line"
    );
    assert!(
        !ranged.content.contains("line 1\n") && !ranged.content.contains("line 2000"),
        "ranged read must NOT contain lines outside the window:\n{}",
        &ranged.content[..ranged.content.len().min(500)]
    );
}

#[test]
fn gh775_ranged_response_starts_at_requested_line() {
    let _iso = crate::core::data_dir::isolated_data_dir();

    let dir = tempfile::tempdir().unwrap();
    let p = write_numbered_file(dir.path(), "big3.ts", 2000);

    let mut cache = SessionCache::new();

    // Full read to warm cache.
    handle_with_task_resolved(&mut cache, &p, "full", CrpMode::Off, None);

    // Ranged read.
    let ranged =
        handle_fresh_with_task_resolved(&mut cache, &p, "lines:500-504", CrpMode::Off, None);

    // The first numbered output line must be line 500.
    // extract_line_range formats as " 500| line 500".
    let body_lines: Vec<&str> = ranged
        .content
        .lines()
        .filter(|l| l.contains("| line "))
        .collect();
    assert!(
        !body_lines.is_empty(),
        "ranged read must contain numbered output lines"
    );
    assert!(
        body_lines[0].contains("500| line 500"),
        "first body line must be line 500, got: {}",
        body_lines[0]
    );
    assert_eq!(
        body_lines.len(),
        5,
        "lines:500-504 must return exactly 5 lines, got {}",
        body_lines.len()
    );
}

#[test]
fn gh775_cold_ranged_read_returns_only_window() {
    let _iso = crate::core::data_dir::isolated_data_dir();

    let dir = tempfile::tempdir().unwrap();
    let p = write_numbered_file(dir.path(), "cold.ts", 2000);

    let mut cache = SessionCache::new();

    // No prior full read — cold ranged read must still return only the window.
    let ranged = handle_with_task_resolved(&mut cache, &p, "lines:100-109", CrpMode::Off, None);
    assert!(
        ranged.content.contains("line 100"),
        "cold ranged read must contain start line"
    );
    assert!(
        ranged.content.contains("line 109"),
        "cold ranged read must contain end line"
    );
    assert!(
        !ranged.content.contains("line 2000"),
        "cold ranged read must NOT contain lines outside window"
    );
}

// ---------------------------------------------------------------------------
// #811: the `anchored:N-M` disk-streaming short-circuit. A fresh, explicitly
// windowed anchored read must never materialize the whole file — these tests
// exercise `read_line_window`/`parse_disk_anchor_range` directly, and the
// end-to-end case that motivated it: a file bigger than the read size limit
// must still serve a small anchored window instead of erroring "file too
// large" (the bug that `read_file_lossy`'s own error message told the caller
// to route around, but couldn't actually satisfy before this fix).
// ---------------------------------------------------------------------------

#[test]
fn parse_disk_anchor_range_accepts_dash_form_only() {
    assert_eq!(parse_disk_anchor_range("5-10"), Some((5, 10)));
    assert_eq!(parse_disk_anchor_range("1-999999"), Some((1, 999_999)));
    // Bare "N" (meaning "to EOF") needs a known total to resolve — the
    // streaming path doesn't have one up front, so it declines rather than
    // guess, and the caller falls back to the full-read path.
    assert_eq!(parse_disk_anchor_range("5"), None);
    assert_eq!(parse_disk_anchor_range("not-a-range"), None);
}

#[test]
fn read_line_window_streams_only_the_requested_span() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("window.txt");
    let body = (1..=50)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, format!("{body}\n")).unwrap();
    let p = path.to_string_lossy().to_string();

    let window = read_line_window(&p, 10, 12).expect("streamed read must succeed");
    assert_eq!(window.total_lines, 50);
    assert_eq!(window.start, 10);
    assert_eq!(window.end, 12);
    assert_eq!(window.body, "line 10\nline 11\nline 12");
}
