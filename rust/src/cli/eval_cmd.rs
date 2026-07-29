//! `lean-ctx eval` — deterministic with/without output-quality eval CLI (#238).
//!
//! Subcommands:
//! * `eval init <dir>`     — scaffold a runnable starter suite (one QA + one code task).
//! * `eval ab --suite P`   — run the A/B comparison and render the report.

use std::path::{Path, PathBuf};

use crate::core::eval_ab::footprint::{
    Footprint, FootprintConfig, FootprintReport, run_footprint_ab,
};
use crate::core::eval_ab::model::{ModelRunner, OpenAiRunner, RecordedRunner, RecordingRunner};
use crate::core::eval_ab::report::ReportConfig;
use crate::core::eval_ab::suite::EvalSuite;
use crate::core::eval_ab::{AbRunConfig, run_ab};

/// Entry point dispatched from `cli::dispatch`.
pub fn cmd_eval(args: &[String]) {
    // `eval --delta [opts]` is sugar for the footprint subcommand.
    if args.iter().any(|a| a == "--delta") {
        let rest: Vec<String> = args.iter().filter(|a| *a != "--delta").cloned().collect();
        return cmd_footprint(&rest);
    }
    match args.first().map(String::as_str) {
        Some("ab") => cmd_ab(&args[1..]),
        Some("footprint" | "delta") => cmd_footprint(&args[1..]),
        Some("init") => cmd_init(&args[1..]),
        Some("-h" | "--help") | None => print_help(),
        Some(other) => {
            eprintln!("eval: unknown subcommand '{other}'\n");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "lean-ctx eval — deterministic with/without output-quality proof\n\n\
USAGE:\n\
  lean-ctx eval init <dir>                 Scaffold a runnable starter suite\n\
  lean-ctx eval ab --suite <file> [opts]   Run the A/B quality comparison\n\
  lean-ctx eval footprint --suite <f> [o]  Ablate the tool's OWN injected context\n\n\
ab OPTIONS:\n\
  --suite <file>     NDJSON suite (required)\n\
  --budget <n>       Token budget per condition (default 4000)\n\
  --margin <f>       Non-inferiority margin for the gate (default 0.0)\n\
  --out <file>       Write the JSON report here (optional)\n\
  --replay <file>    Replay a recording instead of calling a live model (deterministic CI)\n\
  --record <file>    Call the live model and save responses to a recording\n\
  --gate             Exit non-zero if the verdict is a regression\n\n\
footprint OPTIONS (also: `eval --delta`):\n\
  --suite <file>     Footprint-sensitive NDJSON suite (required)\n\
  --margin <f>       Non-inferiority margin for the per-element gate (default 0.0)\n\
  --floor <n>        Min marginal tokens before flagging an element to prune (default 50)\n\
  --replay <file>    Replay a recording (deterministic); --record to capture live\n\
  --json             Emit the full JSON report instead of the side-by-side table\n\
  --gate             Exit non-zero if any injected element is actively harmful\n\n\
LIVE MODEL (when not replaying) is read from the environment:\n\
  LEAN_CTX_EVAL_MODEL_URL   OpenAI-compatible base URL (e.g. https://api.openai.com/v1)\n\
  LEAN_CTX_EVAL_MODEL       Model id (e.g. gpt-4o-mini)\n\
  LEAN_CTX_EVAL_MODEL_KEY   API key (optional for local servers)\n\
  LEAN_CTX_EVAL_SEED        Decoding seed (default 7)"
    );
}

/// Returns the value following `flag` in `args`, if present.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn cmd_ab(args: &[String]) {
    let Some(suite_path) = flag_value(args, "--suite") else {
        eprintln!("eval ab: --suite <file> is required");
        std::process::exit(2);
    };
    let suite_path = PathBuf::from(suite_path);
    let suite = match EvalSuite::load(&suite_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("eval ab: {e:#}");
            std::process::exit(1);
        }
    };
    let suite_name = suite_path
        .file_name()
        .map_or_else(|| "suite".to_string(), |s| s.to_string_lossy().into_owned());

    let mut cfg = AbRunConfig::default();
    if let Some(b) = flag_value(args, "--budget").and_then(|v| v.parse().ok()) {
        cfg.budget_tokens = b;
    }
    cfg.report = ReportConfig {
        noninferiority_margin: flag_value(args, "--margin")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        ..ReportConfig::default()
    };

    // Runner selection: replay (deterministic) > live + record > live.
    let report = if let Some(replay) = flag_value(args, "--replay") {
        let runner = match RecordedRunner::from_file(Path::new(replay)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("eval ab: {e:#}");
                std::process::exit(1);
            }
        };
        run_or_exit(&suite, &suite_name, &runner, &cfg)
    } else {
        let live = match OpenAiRunner::from_env() {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "eval ab: no live model configured: {e:#}\n(use --replay <file> for an offline run)"
                );
                std::process::exit(1);
            }
        };
        if let Some(record_path) = flag_value(args, "--record") {
            let recorder = RecordingRunner::new(live);
            let report = run_or_exit(&suite, &suite_name, &recorder, &cfg);
            if let Err(e) = recorder.into_recording().save(Path::new(record_path)) {
                eprintln!("eval ab: failed to save recording: {e:#}");
                std::process::exit(1);
            }
            println!("Recording saved → {record_path}");
            report
        } else {
            run_or_exit(&suite, &suite_name, &live, &cfg)
        }
    };

    // sobre: no ed25519 attestation. The report is the deliverable; the CI
    // gate reads the verdict directly instead of a signed artifact.
    if let Some(path) = flag_value(args, "--out") {
        match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    eprintln!("eval ab: writing {path}: {e}");
                    std::process::exit(1);
                }
                println!("report written to {path}");
            }
            Err(e) => {
                eprintln!("eval ab: serializing report: {e}");
                std::process::exit(1);
            }
        }
    }

    println!("{}", report.render());

    if has_flag(args, "--gate") && !report.verdict.gate_passes() {
        eprintln!("
quality gate FAILED: {}", report.verdict.label());
        std::process::exit(1);
    }
}

fn run_or_exit(
    suite: &EvalSuite,
    suite_name: &str,
    runner: &dyn crate::core::eval_ab::model::ModelRunner,
    cfg: &AbRunConfig,
) -> crate::core::eval_ab::report::AbReport {
    match run_ab(suite, suite_name, runner, cfg) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("eval ab: run failed: {e:#}");
            std::process::exit(1);
        }
    }
}

/// `eval footprint` (alias `eval --delta`): ablate each element of lean-ctx's own
/// injected context (rules / tool schemas / wakeup) and report per-element
/// pass-rate Δ + token Δ with a prune recommendation (#959).
fn cmd_footprint(args: &[String]) {
    let Some(suite_path) = flag_value(args, "--suite") else {
        eprintln!("eval footprint: --suite <file> is required");
        std::process::exit(2);
    };
    let suite_path = PathBuf::from(suite_path);
    let suite = match EvalSuite::load(&suite_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("eval footprint: {e:#}");
            std::process::exit(1);
        }
    };
    let suite_name = suite_path.file_name().map_or_else(
        || "footprint".to_string(),
        |s| s.to_string_lossy().into_owned(),
    );

    let margin = flag_value(args, "--margin")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let token_floor = flag_value(args, "--floor")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| FootprintConfig::default().token_floor);
    let cfg = FootprintConfig {
        report: ReportConfig {
            noninferiority_margin: margin,
            ..ReportConfig::default()
        },
        token_floor,
    };

    // The footprint under test is what this install actually injects.
    let project_root = std::env::current_dir()
        .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().into_owned());
    let footprint = Footprint::live(&project_root);

    let mut report = if let Some(replay) = flag_value(args, "--replay") {
        let runner = match RecordedRunner::from_file(Path::new(replay)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("eval footprint: {e:#}");
                std::process::exit(1);
            }
        };
        run_footprint_or_exit(&suite, &suite_name, &footprint, &runner, &cfg)
    } else {
        let live = match OpenAiRunner::from_env() {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "eval footprint: no live model configured: {e:#}\n(use --replay <file> for an offline run)"
                );
                std::process::exit(1);
            }
        };
        if let Some(record_path) = flag_value(args, "--record") {
            let recorder = RecordingRunner::new(live);
            let report = run_footprint_or_exit(&suite, &suite_name, &footprint, &recorder, &cfg);
            if let Err(e) = recorder.into_recording().save(Path::new(record_path)) {
                eprintln!("eval footprint: failed to save recording: {e:#}");
                std::process::exit(1);
            }
            println!("Recording saved → {record_path}");
            report
        } else {
            run_footprint_or_exit(&suite, &suite_name, &footprint, &live, &cfg)
        }
    };

    let agent_id = crate::core::agent_identity::current_agent_id().to_string();
    if let Err(e) = report.sign(&agent_id) {
        eprintln!("eval footprint: signing failed: {e}");
        std::process::exit(1);
    }

    let out = match flag_value(args, "--out") {
        Some(p) => PathBuf::from(p),
        None => match default_footprint_path() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("eval footprint: {e}");
                std::process::exit(1);
            }
        },
    };
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&out, report.to_json()) {
        eprintln!("eval footprint: write {}: {e}", out.display());
        std::process::exit(1);
    }

    if has_flag(args, "--json") {
        println!("{}", report.to_json());
    } else {
        println!("{}", report.render());
        println!("artifact:           {}", out.display());
    }

    if has_flag(args, "--gate") && !report.gate_passes() {
        eprintln!("\nfootprint gate FAILED: a harmful injected element is present");
        std::process::exit(1);
    }
}

fn run_footprint_or_exit(
    suite: &EvalSuite,
    suite_name: &str,
    footprint: &Footprint,
    runner: &dyn ModelRunner,
    cfg: &FootprintConfig,
) -> FootprintReport {
    match run_footprint_ab(suite, suite_name, footprint, runner, cfg) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("eval footprint: run failed: {e:#}");
            std::process::exit(1);
        }
    }
}

/// Default footprint artifact location: `<data_dir>/eval/footprint-report-v1_<utc>.json`.
fn default_footprint_path() -> Result<PathBuf, String> {
    let dir = crate::core::data_dir::lean_ctx_data_dir()?.join("eval");
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir eval: {e}"))?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    Ok(dir.join(format!("footprint-report-v1_{stamp}.json")))
}

fn cmd_init(args: &[String]) {
    let dir = PathBuf::from(args.first().map_or("eval-suite", |s| s.as_str()));
    match write_starter_suite(&dir) {
        Ok(suite) => {
            println!("Starter suite written to {}", dir.display());
            println!("Suite file: {}", suite.display());
            println!("\nNext:");
            println!("  # 1) record real model answers once (needs a live model in env)");
            println!(
                "  lean-ctx eval ab --suite {} --record {}/recording.json",
                suite.display(),
                dir.display()
            );
            println!("  # 2) replay deterministically anywhere (CI)");
            println!(
                "  lean-ctx eval ab --suite {} --replay {}/recording.json --gate",
                suite.display(),
                dir.display()
            );
        }
        Err(e) => {
            eprintln!("eval init: {e:#}");
            std::process::exit(1);
        }
    }
}

/// Materializes a small, runnable starter suite: one RAG/QA task whose answer lives in the
/// corpus, and one POSIX-shell code task with a failing stub + unit test.
fn write_starter_suite(dir: &Path) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let corpus = dir.join("corpus");
    let code = dir.join("code");
    std::fs::create_dir_all(&corpus).context("creating corpus dir")?;
    std::fs::create_dir_all(&code).context("creating code dir")?;

    std::fs::write(
        corpus.join("architecture.md"),
        "# Consolidation pipeline\n\n\
Provider data flows through one consolidation pipeline. Artifacts are persisted to four \
stores: the BM25 index, the Graph index, ProjectKnowledge, and the Session cache. This is \
what lets semantic search, knowledge recall, and cross-source hints share one source of truth.\n",
    )
    .context("writing corpus/architecture.md")?;
    std::fs::write(
        corpus.join("overview.md"),
        "# Overview\n\nlean-ctx is a context runtime for AI agents. This file is general \
background and intentionally does not list the consolidation stores.\n",
    )
    .context("writing corpus/overview.md")?;

    std::fs::write(
        code.join("test.sh"),
        "#!/bin/sh\n. ./solution.sh\n[ \"$(add 2 3)\" = \"5\" ] || exit 1\n[ \"$(add 10 20)\" = \"30\" ] || exit 1\n",
    )
    .context("writing code/test.sh")?;
    std::fs::write(
        code.join("solution.sh"),
        "# TODO: implement add() so that `add a b` prints a+b\nadd() { echo 0; }\n",
    )
    .context("writing code/solution.sh")?;

    let suite = dir.join("suite.ndjson");
    let lines = [
        r#"{"id":"qa-consolidation-stores","domain":"qa","prompt":"Which four stores does the consolidation pipeline persist artifacts to?","workspace":"corpus","answers":["bm25 index, graph index, projectknowledge, session cache","bm25, graph, knowledge, session"]}"#,
        r#"{"id":"code-add","domain":"code","prompt":"Implement the POSIX shell function add in solution.sh so that `add a b` prints the sum a+b. Output only the file contents.","workspace":"code","target_file":"solution.sh","test_cmd":"sh test.sh"}"#,
    ];
    std::fs::write(
        &suite,
        format!("# lean-ctx eval starter suite\n{}\n", lines.join("\n")),
    )
    .context("writing suite.ndjson")?;
    Ok(suite)
}

#[cfg(test)]
mod recording_guard_tests {
    use super::*;

    /// The committed recording (`rust/eval/recording.json`) is what flips the CI
    /// quality-gate from "skipped" to "enforced" (#361 Phase 3). Guard it
    /// **in-process** so a suite/corpus/prompt change that invalidates the
    /// recording (a replay key miss) or a captured regression fails here in
    /// `cargo test` — i.e. during `dev-install` — not only in CI.
    #[test]
    fn committed_recording_replays_and_passes_gate() {
        let dir = tempfile::tempdir().unwrap();
        let suite_path = write_starter_suite(dir.path()).expect("scaffold starter suite");
        let suite = EvalSuite::load(&suite_path).expect("load starter suite");

        let rec_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/recording.json");
        assert!(
            rec_path.exists(),
            "committed recording missing at {} — CI quality-gate would silently skip",
            rec_path.display()
        );
        let runner = RecordedRunner::from_file(&rec_path).expect("load committed recording");

        // Every (task × condition) request must hit a recorded key, else the
        // recording drifted from the suite/corpus/prompt and must be re-captured.
        let report = run_ab(&suite, "suite.ndjson", &runner, &AbRunConfig::default())
            .expect("committed recording must cover every replay key");
        assert!(
            report.verdict.gate_passes(),
            "committed recording must not encode a regression, got: {}",
            report.verdict.label()
        );
    }
}
