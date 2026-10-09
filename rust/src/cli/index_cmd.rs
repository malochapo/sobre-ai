use std::path::Path;
use std::time::Duration;

use crate::terminal_ui::ProgressIndicator;

pub(crate) fn cmd_index(args: &[String]) {
    let project_root = super::common::detect_project_root(args);

    match find_subcommand(args) {
        Some("status") => {
            if args.iter().any(|a| a == "--json") {
                println!(
                    "{}",
                    crate::core::index_orchestrator::status_json(&project_root)
                );
            } else {
                print_human_status(&project_root);
            }
        }
        Some("build") => build(&project_root, args.iter().any(|a| a == "--force")),
        other => {
            // A `\` line continuation would eat the indentation.
            eprintln!(
                "{}",
                [
                    "Usage: sobre index <build|status> [--root <path>]",
                    "  sobre index build            graph + BM25 indexes, and the dense index",
                    "                               in builds with local embeddings",
                    "  sobre index build --force    drop the indexes first, rebuild from scratch",
                    "  sobre index status [--json]  what is built",
                    "Leave files out of every index with `ignore` in config.toml.",
                ]
                .join("\n")
            );
            // `sobre index` alone asks for the usage; anything else is a mistake.
            if other.is_some() {
                std::process::exit(2);
            }
        }
    }
}

/// Build (or with `force`, rebuild from scratch) the project's indexes:
/// graph, then BM25, then the dense index in builds with local embeddings.
fn build(project_root: &str, force: bool) {
    // Activate the memory guardian for CLI builds so graph/BM25 abort
    // checks actually fire.
    crate::core::memory_guard::start_guard(std::sync::Arc::new(|level| {
        tracing::warn!("[index build] memory pressure: {level:?} — background tasks will throttle");
        if level >= crate::core::memory_guard::PressureLevel::Hard {
            crate::core::content_cache::clear();
        }
        crate::core::memory_guard::force_purge();
    }));

    if force {
        let root = Path::new(project_root);
        if let Some(path) = crate::core::bm25_index::BM25Index::index_file_path(root) {
            let _ = std::fs::remove_file(path);
        }
        // C4: purge the property graph (graph.db + wal/shm + meta) and
        // the call-graph cache so the rebuild starts clean.
        crate::core::graph_index::purge_index(project_root);
        if let Some(dir) = crate::core::index_namespace::vectors_dir(root) {
            let _ = std::fs::remove_file(dir.join("embeddings.bin"));
        }
    }

    crate::core::index_orchestrator::ensure_all_background(project_root);
    wait_graph_bm25_progress(project_root);

    // Surface the BM25 build outcome (chunk count + persisted size, or the
    // "too large to persist" remedy) so the operator knows the index state.
    let summary = crate::core::index_orchestrator::bm25_summary(project_root);
    if let Some(note) = summary.note {
        eprintln!("  BM25: {note}");
    }
    if let Some(err) = summary.last_error {
        eprintln!("  BM25 error: {err}");
    }

    #[cfg(feature = "dense")]
    build_dense(project_root);
}

/// The dense (embedding) index, on top of the fresh BM25 index. Uses the
/// model `sobre embeddings provision` installed; never downloads it.
#[cfg(feature = "dense")]
fn build_dense(project_root: &str) {
    crate::core::interrupt::install_ctrlc_handler();
    wait_semantic_progress(project_root);
    let sem = crate::core::index_orchestrator::semantic_summary(project_root);
    match sem.state {
        "ready" => eprintln!("  dense index ready"),
        "failed" => eprintln!(
            "  dense index failed: {}",
            sem.last_error.unwrap_or_else(|| String::from("unknown"))
        ),
        _ => {
            if let Some(note) = sem.note {
                eprintln!("  dense index: {note}");
            }
        }
    }
}

/// Wait for graph + BM25 with a shared progress indicator, using
/// [`progress_view`](crate::core::index_orchestrator::progress_view) (typed, no
/// JSON scrape). No timeout: the builders run on this process's threads, so
/// returning early would stop them when the process exits. Both phases always
/// end, a panic included, so the wait does too.
fn wait_graph_bm25_progress(project_root: &str) {
    let mut progress = ProgressIndicator::new("indexes");
    loop {
        let view = crate::core::index_orchestrator::progress_view(project_root);
        if !view.graph_bm25_active() {
            progress.finish("indexes (graph + BM25) done");
            return;
        }
        apply_graph_bm25_progress(&mut progress, &view);
        progress.tick();
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Build the dense index on a worker thread while showing progress, and wait
/// for it to finish: the worker stops when this process exits.
#[cfg(feature = "dense")]
fn wait_semantic_progress(project_root: &str) {
    let root = project_root.to_string();
    let handle = std::thread::spawn(move || {
        crate::core::index_orchestrator::build_semantic(&root);
    });
    let mut progress = ProgressIndicator::new("dense");
    while !handle.is_finished() {
        let view = crate::core::index_orchestrator::progress_view(project_root);
        apply_semantic_progress(&mut progress, &view);
        progress.tick();
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = handle.join();
    progress.finish("dense done");
}

fn apply_graph_bm25_progress(
    progress: &mut ProgressIndicator,
    view: &crate::core::index_orchestrator::ProgressView,
) {
    if !view.graph_run_done {
        progress.set_label("graph");
        progress.indeterminate();
        return;
    }
    // BM25 phase (or graph→BM25 handoff).
    progress.set_label("BM25");
    if view.bm25.total > 0 {
        progress.set(view.bm25.done, view.bm25.total);
    } else {
        progress.indeterminate();
    }
}

#[cfg(feature = "dense")]
fn apply_semantic_progress(
    progress: &mut ProgressIndicator,
    view: &crate::core::index_orchestrator::ProgressView,
) {
    if view.semantic.total > 0 {
        progress.set(view.semantic.done, view.semantic.total);
    } else {
        progress.indeterminate();
    }
}

/// First non-flag token, skipping the value of `--root`/`--project-root`, so
/// `index --root /x build` resolves the subcommand `build`, not the path.
fn find_subcommand(args: &[String]) -> Option<&str> {
    const VALUE_FLAGS: [&str; 2] = ["--root", "--project-root"];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if VALUE_FLAGS.contains(&a.as_str()) {
            let _ = it.next();
            continue;
        }
        if a.starts_with("--") {
            continue;
        }
        return Some(a.as_str());
    }
    None
}

fn print_human_status(project_root: &str) {
    let disk = crate::core::index_orchestrator::disk_status(project_root);

    println!("  Project:        {project_root}");
    println!(
        "  Graph Index:    {}",
        format_disk_line(&disk.graph_index, "files")
    );
    println!(
        "  BM25 Index:     {}",
        format_disk_line(&disk.bm25_index, "chunks")
    );
    println!(
        "  Code Graph:     {}",
        format_disk_line(&disk.code_graph, "nodes")
    );
    println!(
        "  Semantic Index: {}",
        format_disk_line(&disk.semantic_index, "vectors")
    );
    // Surface the `ignore` globs so a filtered index is always
    // recognizable. Absent when `ignore` is empty.
    if let Some(summary) = crate::core::index_filter::IndexFileFilter::effective().summary() {
        println!("  Index Filters:  {summary}");
    }
}

fn format_disk_line(ds: &crate::core::index_orchestrator::DiskStatus, count_label: &str) -> String {
    if !ds.exists {
        return "not built".to_string();
    }
    let mut parts = vec!["ready".to_string()];
    if let Some(count) = ds.file_count {
        parts.push(format!("{count} {count_label}"));
    }
    if let Some(bytes) = ds.size_bytes {
        parts.push(format_bytes(bytes));
    }
    if let Some(ref t) = ds.modified_at {
        parts.push(format!("built {t}"));
    }
    format!("({})", parts.join(", "))
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn subcommand_found_before_flags() {
        assert_eq!(
            find_subcommand(&args(&["build", "--root", "/x"])),
            Some("build")
        );
        assert_eq!(find_subcommand(&args(&["status"])), Some("status"));
    }

    #[test]
    fn subcommand_skips_root_value() {
        // The path must never be mistaken for the subcommand.
        assert_eq!(
            find_subcommand(&args(&["--root", "/x", "--force", "build"])),
            Some("build")
        );
    }

    #[test]
    fn subcommand_none_for_flag_only_args() {
        assert_eq!(find_subcommand(&args(&["--root", "/x"])), None);
        assert_eq!(find_subcommand(&[]), None);
    }
}
