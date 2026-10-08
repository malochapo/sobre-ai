//! `lean-ctx verify-cache` — a first-class, one-command proof that the session
//! cache is engaged: it reads a file twice through the real `SessionCache` and
//! asserts the second (unchanged) read collapses to a ~13-token `[unchanged …]`
//! stub instead of re-sending the whole file.
//!
//! Born out of the independent tokbench benchmark (GH #361), where the reviewer
//! had to verify the ~13-token re-read *by hand*. This makes that check
//! reproducible for any user or reviewer, with a machine-checkable exit code.

use crate::core::cache::SessionCache;
use crate::tools::CrpMode;
use crate::tools::ctx_read;

/// Tokens below which a re-read counts as a cache "stub". The marker itself is
/// ~13 tokens; the ceiling is generous to absorb long paths / file-ref labels
/// while staying far under any real file's full payload.
const STUB_MAX_TOKENS: usize = 64;

/// Exit codes: 0 = cache proven, 1 = expected stub but got full content.
pub(crate) fn cmd_verify_cache(args: &[String]) -> i32 {
    let json = args.iter().any(|a| a == "--json");

    let (path, cleanup) = match resolve_target(args) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("verify-cache: {e}");
            return 1;
        }
    };

    let outcome = run_probe(&path);

    if let Some(probe) = cleanup {
        let _ = std::fs::remove_file(&probe);
    }

    if json {
        print_json(&path, &outcome);
    } else {
        print_human(&path, &outcome);
    }
    outcome.exit_code()
}

/// What the two probe reads revealed about the cache.
struct ProbeOutcome {
    /// Tokens emitted by the first (full) read.
    first_tokens: usize,
    /// Tokens emitted by the second (re-)read — the stub on a working cache.
    second_tokens: usize,
    /// True when the second read returned an `[unchanged …]` / cached stub.
    is_stub: bool,
    /// Cache hits / total reads observed during this run.
    run_hits: u64,
    run_reads: u64,
}

impl ProbeOutcome {
    fn passed(&self) -> bool {
        self.is_stub
            && self.second_tokens <= STUB_MAX_TOKENS
            && self.second_tokens < self.first_tokens
    }

    fn exit_code(&self) -> i32 {
        i32::from(!self.passed())
    }

    fn saved_pct(&self) -> f64 {
        if self.first_tokens > 0 {
            (1.0 - (self.second_tokens as f64 / self.first_tokens as f64)) * 100.0
        } else {
            0.0
        }
    }
}

/// Run the real two-read probe against `path` using an in-process cache — the
/// same `SessionCache` + `ctx_read` path the MCP server serves.
fn run_probe(path: &str) -> ProbeOutcome {
    let crp = CrpMode::effective();
    let mut cache = SessionCache::new();

    let first = ctx_read::handle_with_task_resolved(&mut cache, path, "full", crp, None);
    let second = ctx_read::handle_with_task_resolved(&mut cache, path, "full", crp, None);

    let is_stub = second.content.contains("[unchanged") || second.content.contains(" cached ");

    let stats = cache.get_stats();
    let run_reads = stats.total_reads();
    let run_hits = stats.cache_hits();

    ProbeOutcome {
        first_tokens: first.output_tokens,
        second_tokens: second.output_tokens,
        is_stub,
        run_hits,
        run_reads,
    }
}

/// Resolve the file to probe: an explicit path argument if given and readable,
/// otherwise a synthetic probe file written to the temp dir (cleaned up after).
/// Returns `(path, Some(probe_to_delete))` when a synthetic file was created.
fn resolve_target(args: &[String]) -> Result<(String, Option<String>), String> {
    if let Some(explicit) = args.iter().find(|a| !a.starts_with('-')) {
        let p = std::path::Path::new(explicit);
        if p.is_file() {
            return Ok((explicit.clone(), None));
        }
        return Err(format!("'{explicit}' is not a readable file"));
    }

    let file_name = format!(".lean-ctx-verify-cache-{}.txt", std::process::id());
    let content = synthetic_probe_source();

    // Prefer the current directory (inside the project root) so the read does not
    // trip the defense-in-depth path-escape guard; fall back to the temp dir.
    let candidates = [
        std::env::current_dir().ok().map(|d| d.join(&file_name)),
        Some(std::env::temp_dir().join(&file_name)),
    ];
    for cand in candidates.into_iter().flatten() {
        if std::fs::write(&cand, &content).is_ok() {
            let path = cand.to_string_lossy().into_owned();
            return Ok((path.clone(), Some(path)));
        }
    }
    Err("could not create a probe file in the current or temp directory".to_string())
}

/// Deterministic, clearly-larger-than-a-stub source used when no path is given.
fn synthetic_probe_source() -> String {
    let mut s = String::from(
        "// lean-ctx verify-cache synthetic probe — safe to delete.\n\
         use std::collections::HashMap;\n\n",
    );
    for i in 0..24 {
        s.push_str(&format!(
            "/// Probe function number {i}, present so the full read is unambiguous.\n\
             let mut counts: HashMap<String, usize> = HashMap::new();\n    \
             for token in input.split_whitespace() {{\n        \
             *counts.entry(token.to_string()).or_insert(0) += factor + {i};\n    }}\n    \
             counts\n}}\n\n"
        ));
    }
    s
}

fn print_human(path: &str, o: &ProbeOutcome) {
    println!("lean-ctx verify-cache\n");
    println!("  Target:        {path}");

    println!("  Read #1 (full):     {} tokens", o.first_tokens);
    println!(
        "  Read #2 (re-read):  {} tokens  {}",
        o.second_tokens,
        if o.is_stub {
            "[unchanged stub]"
        } else {
            "[full — NOT cached]"
        }
    );
    println!("  Re-read savings:    {:.0}%", o.saved_pct());
    println!("  Cache hits (run):   {}/{}", o.run_hits, o.run_reads);

    if o.passed() {
        println!(
            "\n  PASS — session cache engaged: the unchanged re-read cost {} tokens \
             (≈13-token stub).",
            o.second_tokens
        );
    } else {
        println!(
            "\n  FAIL — expected a ~13-token stub on re-read, got {} tokens. The session \
             cache is not collapsing unchanged re-reads.",
            o.second_tokens
        );
    }
}

fn print_json(path: &str, o: &ProbeOutcome) {
    let status = if o.passed() { "pass" } else { "fail" };
    let json = serde_json::json!({
        "status": status,
        "target": path,
        "first_read_tokens": o.first_tokens,
        "second_read_tokens": o.second_tokens,
        "second_read_is_stub": o.is_stub,
        "stub_threshold_tokens": STUB_MAX_TOKENS,
        "reread_saved_pct": (o.saved_pct() * 10.0).round() / 10.0,
        "run_cache_hits": o.run_hits,
        "run_cache_reads": o.run_reads,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&json).unwrap_or_default()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(first: usize, second: usize, is_stub: bool) -> ProbeOutcome {
        ProbeOutcome {
            first_tokens: first,
            second_tokens: second,
            is_stub,
            run_hits: 1,
            run_reads: 2,
        }
    }

    #[test]
    fn pass_when_second_read_is_small_stub() {
        let o = outcome(2000, 13, true);
        assert!(o.passed());
        assert_eq!(o.exit_code(), 0);
    }

    #[test]
    fn fail_when_second_read_not_stubbed() {
        let o = outcome(2000, 1900, false);
        assert!(!o.passed());
        assert_eq!(o.exit_code(), 1);
    }

    #[test]
    fn fail_when_marker_present_but_above_stub_ceiling() {
        // Stub marker present, but payload above the ceiling ⇒ not a real stub.
        let o = outcome(2000, 200, true);
        assert!(!o.passed());
        assert_eq!(o.exit_code(), 1);
    }

    #[test]
    fn saved_pct_reflects_collapse() {
        assert!(outcome(1000, 10, true).saved_pct() > 98.0);
        assert_eq!(outcome(0, 0, true).saved_pct(), 0.0);
    }
}
