use sobre::core::compressor::aggressive_compress;
use sobre::core::config::CompressionLevel;
use sobre::core::terse;
use sobre::core::tokens::count_tokens;
use sobre::shell::compress::compress_if_beneficial_pub;

/// Tool output of several kinds, for the prose pipeline below.
const PROSE_PIPELINE_SAMPLES: &[&str] = &[
    "On branch main\nYour branch is up to date with 'origin/main'.\n\nChanges not staged for commit:\n  (use \"git add <file>...\" to update what will be committed)\n\tmodified:   src/config.rs\n\tmodified:   src/server.rs\n\nno changes added to commit (use \"git add\" and/or \"git commit -a\")",
    "   Compiling myapp v0.1.0 (/home/dev/myapp)\nwarning: unused variable `result`\n  --> src/config.rs:42:9\n   |\n42 |     let result = compute();\n   |         ^^^^^^ help: if this is intentional, prefix it with an underscore: `_result`\n   |\n   = note: `#[warn(unused_variables)]` on by default\n\nwarning: `myapp` (lib) generated 1 warning\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 12.34s",
    "npm warn deprecated inflight@1.0.6: This module is not supported, and leaks memory.\n\nadded 847 packages, and audited 848 packages in 12s\n\n142 packages are looking for funding\n  run `npm fund` for details\n\n3 moderate severity vulnerabilities\n\nTo address all issues, run:\n  npm audit fix\n\nRun `npm audit` for details.",
    "{\n  \"status\": \"success\",\n  \"data\": {\n    \"users\": [\n      {\n        \"id\": 1,\n        \"name\": \"Alice\",\n        \"role\": \"admin\"\n      },\n      {\n        \"id\": 2,\n        \"name\": \"Bob\",\n        \"role\": \"user\"\n      }\n    ]\n  }\n}",
    "# myapp\n\nA small web service.\n\n## Installation\n\nThe installation is described in the documentation. After the installation, the service automatically integrates with your configuration, and no manual configuration is needed.\n\n## Usage\n\nRun the service and open the dashboard. For more details, see the documentation.",
];

#[test]
fn prose_pipeline_passes_its_quality_gate_and_max_compresses_at_least_as_much() {
    let mut standard_total = 0.0f32;
    let mut max_total = 0.0f32;
    for sample in PROSE_PIPELINE_SAMPLES {
        let standard = terse::pipeline::compress(sample, &CompressionLevel::Standard, None);
        assert!(
            standard.quality_passed,
            "quality gate failed at Standard for:\n{sample}"
        );
        let max = terse::pipeline::compress(sample, &CompressionLevel::Max, None);
        standard_total += standard.savings_pct;
        max_total += max.savings_pct;
    }
    assert!(
        standard_total > 0.0,
        "Standard must compress some of these samples"
    );
    assert!(
        max_total >= standard_total,
        "Max must compress at least as much as Standard: {max_total:.1} < {standard_total:.1}"
    );
}

#[test]
fn markdown_aggressive_keeps_headings_and_saves_tokens() {
    let mut doc =
        String::from("# Context Engine\n\nIntro text with stable overview.\n\n## Setup\n\n");
    for i in 0..60 {
        doc.push_str(&format!(
            "Repeated explanatory prose line {i} about background concepts and examples.\n"
        ));
    }
    doc.push_str("- `sobre_read` keeps exact source references for agent tasks.\n");
    doc.push_str("## Safety\n\nMUST preserve diagnostics, commands, and recovery notes.\n");
    for i in 0..60 {
        doc.push_str(&format!(
            "Additional explanatory prose line {i} about operational background.\n"
        ));
    }

    let compressed = aggressive_compress(&doc, Some("md"));
    assert!(count_tokens(&compressed) < count_tokens(&doc));
    assert!(compressed.contains("# Context Engine"));
    assert!(compressed.contains("## Setup"));
    assert!(compressed.contains("## Safety"));
    assert!(compressed.contains("sobre_read"));
    assert!(compressed.contains("MUST preserve"));
}

#[test]
fn cargo_warning_output_folds_progress_and_preserves_diagnostics() {
    let mut output = String::new();
    for i in 0..80 {
        output.push_str(&format!(
            "   Compiling crate_{i:03} v0.1.0 (/tmp/ws/crate_{i:03})\n"
        ));
    }
    output.push_str("warning: unused variable: `tmp`\n");
    output.push_str("  --> crates/demo/src/lib.rs:42:9\n");
    output.push_str("   |\n42 |     let tmp = 1;\n   |         ^^^\n");
    output.push_str("warning: `demo` generated 1 warning\n");
    output.push_str("    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.23s\n");

    let compressed = compress_if_beneficial_pub("cargo build", &output);
    assert!(count_tokens(&compressed) < count_tokens(&output));
    assert!(compressed.contains("cargo compile/check lines folded"));
    assert!(compressed.contains("warning: unused variable"));
    assert!(compressed.contains("crates/demo/src/lib.rs:42:9"));
    assert!(compressed.contains("generated 1 warning"));
}

#[test]
fn pytest_success_output_folds_passed_lines_and_preserves_summary() {
    let mut output = String::from(
        "============================= test session starts =============================\n",
    );
    for i in 0..80 {
        output.push_str(&format!(
            "tests/test_mod_{:02}.py::test_case_{i:03} PASSED [ {:02}%]\n",
            i / 10,
            i % 100
        ));
    }
    output.push_str(
        "======================= 80 passed, 0 warnings in 2.34s =======================\n",
    );

    let compressed = compress_if_beneficial_pub("pytest", &output);
    assert!(count_tokens(&compressed) < count_tokens(&output));
    assert!(compressed.contains("pytest PASSED lines folded"));
    assert!(compressed.contains("80 passed"));
}

#[test]
fn markdown_compaction_is_deterministic_across_calls() {
    // Aggressive compaction must be a pure function of the input bytes.
    // Mixed token frequencies engineer near-tied line scores on purpose — with
    // unordered per-line token sets the f64 summation order could flip the
    // selected lines between calls.
    let mut doc = String::from("# Stability\n\nIntro line for the document.\n");
    for section in 0..4 {
        doc.push_str(&format!("\n## Section {section}\n\n"));
        for i in 0..30 {
            doc.push_str(&format!(
                "candidate_{i} shared_token_{} another_token_{} overlapping detail item.\n",
                i % 3,
                i % 7,
            ));
        }
    }

    let first = aggressive_compress(&doc, Some("md"));
    for _ in 0..16 {
        assert_eq!(
            first,
            aggressive_compress(&doc, Some("md")),
            "markdown compaction must be byte-stable across calls"
        );
    }
}

#[test]
fn verbatim_token_cap_survives_progress_folding() {
    // Folding progress noise must compose with — not replace — the verbatim
    // token cap: a build log whose *diagnostics alone* exceed the budget is
    // still head/tail-truncated with safety-needle preservation.
    let mut output = String::new();
    for i in 0..200 {
        output.push_str(&format!(
            "   Compiling crate_{i:03} v0.1.0 (/tmp/ws/crate_{i:03})\n"
        ));
    }
    output.push_str("error[E0308]: mismatched types\n");
    output.push_str("  --> crates/demo/src/lib.rs:7:5\n");
    // A diagnostic body far above MAX_VERBATIM_TOKENS (8000) that is neither
    // foldable progress nor low-signal, so only the cap can bound it.
    for i in 0..4000 {
        output.push_str(&format!(
            "note: required because of the expansion trace frame {i} in `deeply::nested::module_{i}`\n"
        ));
    }
    output.push_str("error: aborting due to 1 previous error\n");

    let compressed = compress_if_beneficial_pub("cargo build", &output);
    assert!(
        compressed.contains("cargo compile/check lines folded"),
        "progress noise must still fold"
    );
    assert!(
        compressed.contains("lines omitted"),
        "oversized diagnostics must still hit the verbatim cap"
    );
    assert!(
        compressed.contains("error[E0308]: mismatched types"),
        "the head diagnostic must survive"
    );
    assert!(
        compressed.contains("aborting due to 1 previous error"),
        "the tail diagnostic must survive"
    );
    assert!(
        count_tokens(&compressed) < count_tokens(&output) / 4,
        "capped output must be a fraction of the raw log"
    );
}

#[test]
fn plain_txt_without_headings_is_not_structurally_compacted() {
    // A `.txt` with hyphen lists but no ATX heading is ordinary prose — the
    // lossy structural compactor must not touch it (no omitted-lines markers).
    let mut doc = String::from("Shopping notes\n\n");
    for i in 0..40 {
        doc.push_str(&format!("- item number {i} with a longer description\n"));
    }

    let compressed = aggressive_compress(&doc, Some("txt"));
    assert!(
        !compressed.contains("[sobre: omitted"),
        "txt without headings must not be structurally compacted: {compressed}"
    );

    // The same content as `.md` (heading added) opts in.
    let md = format!("# Notes\n\n{doc}");
    let compacted = aggressive_compress(&md, Some("md"));
    assert!(compacted.contains("[sobre: omitted"));
}

#[test]
fn markdown_compaction_never_splits_code_fences() {
    let mut doc = String::from("# Guide\n\nIntro line.\n\n## Usage\n\n");
    for i in 0..40 {
        doc.push_str(&format!("Filler prose sentence number {i} for volume.\n"));
    }
    doc.push_str("```bash\nsobre read src/lib.rs\n\nsobre search \"sobre_read\" src/\n```\n");
    for i in 0..40 {
        doc.push_str(&format!("More filler prose sentence number {i}.\n"));
    }

    let compressed = aggressive_compress(&doc, Some("md"));
    assert_eq!(
        compressed.matches("```").count() % 2,
        0,
        "code fences must stay balanced: {compressed}"
    );
    let mut in_fence = false;
    for line in compressed.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if in_fence {
            assert!(
                !line.starts_with("... [sobre:"),
                "omission marker inside a fenced block: {compressed}"
            );
        }
    }
}
