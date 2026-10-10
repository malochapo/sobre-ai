//! `sobre_search` latency on a synthetic corpus, through the directory walk
//! and through the resident trigram index.
//!
//! A plain `main` rather than criterion: it prints one markdown table per path
//! (p50/p95/p99 per query, and the size of the response in tokens) and checks
//! that both paths return the same matches. It measures time, not savings.
//!
//! Run:    cargo bench --bench efficiency
//! Tune:   `BENCH_FILES=5000` cargo bench --bench efficiency

use std::path::Path;
use std::time::{Duration, Instant};

use sobre::core::protocol::CrpMode;
use sobre::core::tokens::count_tokens;
use sobre::tools::sobre_search;

const ITERS: usize = 50;

fn percentile(sorted: &[Duration], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = (((sorted.len() - 1) as f64) * p).round() as usize;
    sorted[idx].as_secs_f64() * 1000.0
}

/// Deterministic synthetic corpus: `n_files` Rust files spread across 20 dirs.
/// Every file contains the common token `handler`; 1-in-500 contains a rare
/// camelCase token; none contain the negative query.
fn create_corpus(root: &Path, n_files: usize) {
    use std::io::Write;
    for i in 0..n_files {
        let dir = root.join(format!("mod_{}", i % 20));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = std::fs::File::create(dir.join(format!("file_{i}.rs"))).unwrap();
        writeln!(f, "// file {i}").unwrap();
        writeln!(f, "pub fn handler_{i}(x: usize) -> usize {{").unwrap();
        for j in 0..30 {
            writeln!(f, "    let v{j} = compute({j}); // handler work").unwrap();
        }
        writeln!(f, "    x").unwrap();
        writeln!(f, "}}").unwrap();
        if i % 500 == 0 {
            writeln!(f, "fn flushPassiveEffectsRare() {{}}").unwrap();
        }
    }
}

fn measure<F: FnMut() -> String>(mut run: F) -> (f64, f64, f64, usize) {
    let _ = run(); // warm
    let mut times = Vec::with_capacity(ITERS);
    let mut tokens = 0;
    for _ in 0..ITERS {
        let t = Instant::now();
        let out = run();
        times.push(t.elapsed());
        tokens = count_tokens(&out);
    }
    times.sort_unstable();
    (
        percentile(&times, 0.5),
        percentile(&times, 0.95),
        percentile(&times, 0.99),
        tokens,
    )
}

/// One search. `use_index` is the `respect_gitignore` flag: only a search
/// that honours `.gitignore` consults the resident index (and, when there is
/// none yet, starts building it in the background), so `false` always walks.
/// The synthetic corpus has no ignore file and no vendor directory, so both
/// settings search the same files.
fn search(corpus: &str, pattern: &str, max_results: usize, use_index: bool) -> String {
    sobre_search::handle_filtered(
        pattern,
        corpus,
        None,
        max_results,
        CrpMode::Off,
        use_index,
        false,
        None,
        None,
    )
    .text
}

fn bench_search(corpus: &str, pattern: &str, use_index: bool) -> (f64, f64, f64, usize) {
    measure(|| search(corpus, pattern, 20, use_index))
}

fn main() {
    let n_files: usize = std::env::var("BENCH_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let tmp = tempfile::tempdir().unwrap();
    let corpus = tmp.path();
    eprintln!("building synthetic corpus: {n_files} files ...");
    create_corpus(corpus, n_files);
    let corpus_str = corpus.to_string_lossy().to_string();

    let queries = [
        ("common (handler)", "handler"),
        ("rare (flushPassiveEffectsRare)", "flushPassiveEffectsRare"),
        ("negative (xyzzy_nonexistent)", "xyzzy_nonexistent"),
    ];

    println!("# sobre_search efficiency bench ({n_files} files, {ITERS} iters)\n");

    println!("## Walk path\n");
    println!("| query | p50 ms | p95 ms | p99 ms | resp tokens |");
    println!("|---|---|---|---|---|");
    let mut walk_out = Vec::new();
    for (label, pat) in queries {
        let (p50, p95, p99, tok) = bench_search(&corpus_str, pat, false);
        println!("| {label} | {p50:.3} | {p95:.3} | {p99:.3} | {tok} |");
        walk_out.push(match_lines(&corpus_str, pat, false));
    }

    // --- Resident index path: warm synchronously, then measure ---
    let warmed = sobre::core::search_index::warm_blocking(&corpus_str, true);
    println!("\n## Resident index path (warm={warmed})\n");
    println!("| query | p50 ms | p95 ms | p99 ms | resp tokens |");
    println!("|---|---|---|---|---|");
    for (idx, (label, pat)) in queries.iter().enumerate() {
        // The index only narrows the candidate files: its matches must be
        // exactly the walk's.
        let index_hits = match_lines(&corpus_str, pat, true);
        assert_eq!(
            index_hits, walk_out[idx],
            "the index and the walk disagree for query {label:?}"
        );
        let (p50, p95, p99, tok) = bench_search(&corpus_str, pat, true);
        println!("| {label} | {p50:.3} | {p95:.3} | {p99:.3} | {tok} |");
    }
}

/// The set of `file:line` match lines of one search, so the walk path and the
/// index path can be compared.
fn match_lines(corpus: &str, pattern: &str, use_index: bool) -> std::collections::BTreeSet<String> {
    search(corpus, pattern, 500, use_index)
        .lines()
        .filter(|l| l.contains(".rs:") || l.contains(".txt:"))
        .map(str::to_string)
        .collect()
}
