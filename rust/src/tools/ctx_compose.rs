//! `ctx_compose` — task composer (Phase 2 of the efficiency epic).
//!
//! The biggest agent win is a single "rich per call" tool that returns ranked
//! files *with* inline bodies, replacing the typical search → read → outline →
//! read chain (3-5 calls) with one.
//!
//! lean-ctx already has the building blocks as separate tools; this composes
//! them into one response for a natural-language task:
//!   1. extracted keywords,
//!   2. semantically ranked files (BM25 / hybrid),
//!   3. exact match locations (index-backed `ctx_search`),
//!   4. the body of the most relevant symbol, inline.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use crate::core::graph_provider;
use crate::core::tokens::count_tokens;
use crate::tools::CrpMode;

/// Wall-time budget for the semantic-ranking stage. The exact-match and symbol
/// stages are index-backed and cheap; only semantic ranking can hit a cold
/// `O(corpus)` BM25 build. We never let that block the agent loop: past the
/// budget (4s, tuned for cold-start coverage #902) we return what we have and let the detached worker finish warming the
/// resident cache for the next call.
const DEFAULT_SEMANTIC_BUDGET_MS: u64 = 4000;

fn semantic_budget() -> Duration {
    Duration::from_millis(DEFAULT_SEMANTIC_BUDGET_MS)
}

/// Token budget for the inlined symbol bodies. Submodular selection fills it
/// with the most coverage-effective, non-redundant set of symbols.
const DEFAULT_SYMBOL_BUDGET_TOKENS: usize = 600;

fn symbol_budget_tokens() -> usize {
    DEFAULT_SYMBOL_BUDGET_TOKENS
}

/// Wall-time budget for the associative (graph spreading-activation) stage.
/// Opening/building the graph index is `O(corpus)` on a cold repo, so — like
/// semantic ranking — we bound it and skip the (purely additive) section on
/// overrun while the detached worker warms the index.
const DEFAULT_GRAPH_BUDGET_MS: u64 = 1500;

fn graph_budget() -> Duration {
    Duration::from_millis(DEFAULT_GRAPH_BUDGET_MS)
}

/// Per-hop activation decay and hop count for spreading activation. Small decay
/// keeps activation local (structurally near the seeds); 3 hops covers
/// import→callee→sibling chains without diffusing across the whole graph.
const SPREAD_DECAY: f64 = 0.6;
const SPREAD_HOPS: usize = 3;
/// How many associative neighbours to surface.
const SPREAD_TOP_K: usize = 8;

/// Build the associative-relevance block: spreading activation seeded at the
/// files the task keywords resolve to, propagated over the union of the static
/// import/call graph and the *learned* Hebbian co-access graph. Returns an empty
/// string when no graph/seeds are available. Runs entirely in the worker thread
/// so [`associative_block_budgeted`] can bound it.
fn build_associative_block(project_root: &str, keywords: &[String]) -> String {
    let Some(open) = graph_provider::open_or_build(project_root) else {
        return String::new();
    };
    let gp = &open;

    // Seeds: distinct files the keywords resolve to via symbol lookup.
    let mut seed_files: Vec<String> = Vec::new();
    for kw in keywords {
        for sym in gp.find_symbols(kw, None, None) {
            if !seed_files.contains(&sym.file) {
                seed_files.push(sym.file);
            }
        }
    }
    if seed_files.is_empty() {
        return String::new();
    }

    // Hebbian update: files relevant to the same task "fire together", so record
    // their co-access (strengthens future associative recall). Persisted.
    crate::core::cooccurrence::record_access(project_root, &seed_files);

    // Adjacency = static structural edges ∪ learned co-access edges. Edges are
    // made bidirectional so activation spreads both up and down the graph.
    let mut adjacency: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    let mut add_edge = |a: &str, b: &str, w: f64| {
        adjacency
            .entry(a.to_string())
            .or_default()
            .push((b.to_string(), w));
        adjacency
            .entry(b.to_string())
            .or_default()
            .push((a.to_string(), w));
    };
    for e in gp.edges() {
        add_edge(&e.from, &e.to, if e.weight > 0.0 { e.weight } else { 1.0 });
    }
    let coaccess = crate::core::cooccurrence::load(project_root);
    for sf in &seed_files {
        for (nbr, w) in coaccess.related(sf, 16) {
            add_edge(sf, &nbr, w);
        }
    }

    let seeds: HashMap<String, f64> = seed_files.iter().map(|f| (f.clone(), 1.0)).collect();
    let ranked = crate::core::spreading_activation::related_ranked(
        &seeds,
        &adjacency,
        SPREAD_DECAY,
        SPREAD_HOPS,
        SPREAD_TOP_K,
    );
    if ranked.is_empty() {
        return String::new();
    }

    let mut s = String::from("\n## Related (associative: import/call graph + learned co-access)\n");
    for (file, activation) in ranked {
        // Forward-slash normalize so Windows backslash paths are never escape-
        // mangled by client render layers (issue #324).
        let file = crate::core::protocol::display_path(&file);
        s.push_str(&format!("- {file} (activation {activation:.2})\n"));
    }
    s
}

/// Run [`build_associative_block`] under [`graph_budget`]. The Hebbian record is
/// a side effect of the worker, so it persists even when we time out and drop
/// the (optional) section.
fn associative_block_budgeted(project_root: &str, keywords: &[String]) -> String {
    if keywords.is_empty() {
        return String::new();
    }
    let (tx, rx) = mpsc::channel::<String>();
    let root = project_root.to_string();
    let kws = keywords.to_vec();
    std::thread::spawn(move || {
        let block = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_associative_block(&root, &kws)
        }))
        .unwrap_or_else(|_| {
            tracing::warn!("[ctx_compose: associative block panicked; omitting section]");
            String::new()
        });
        let _ = tx.send(block);
    });
    rx.recv_timeout(graph_budget()).unwrap_or_default()
}

/// Words that carry no retrieval signal — dropped from keyword extraction.
const STOPWORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "that",
    "this",
    "from",
    "into",
    "how",
    "where",
    "what",
    "does",
    "are",
    "was",
    "use",
    "used",
    "uses",
    "add",
    "all",
    "any",
    "can",
    "get",
    "set",
    "via",
    "out",
    "its",
    "his",
    "her",
    "you",
    "your",
    "our",
    "find",
    "show",
    "list",
    "make",
    "when",
    "then",
    "has",
    "have",
    "had",
    "not",
    "but",
    "see",
    "function",
    "method",
    "class",
    "code",
    "file",
    "files",
    "implement",
    "implementation",
];

/// Extract up to `max` distinct identifier-ish keywords from a task, preserving
/// original case (symbol lookups are case-sensitive) and first-seen order.
fn extract_keywords(task: &str, max: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for raw in task.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if raw.len() < 3 {
            continue;
        }
        if STOPWORDS.contains(&raw.to_ascii_lowercase().as_str()) {
            continue;
        }
        if seen.insert(raw.to_string()) {
            out.push(raw.to_string());
            if out.len() >= max {
                break;
            }
        }
    }
    out
}

/// Order `keywords` from most to least specific using the resident BM25 index's
/// per-token document frequency (how many chunks contain the token). Rarer =
/// more specific = better as the "exact matches" seed. A token absent from the
/// corpus (df 0) sinks to the end — grepping it yields nothing useful.
///
/// Non-blocking and best-effort: if the resident index isn't warm yet we return
/// the keywords in their original first-seen order (the previous behaviour), so
/// this can only improve the seed, never stall the call to build an index.
fn order_by_specificity(keywords: &[String], project_root: &str) -> Vec<String> {
    let Some(index) = resident_index(project_root) else {
        return keywords.to_vec();
    };
    rank_by_doc_freq(keywords, &index.doc_freqs)
}

/// Pure ranking core: choose the exact-match seed keyword.
///
/// The seed feeds a case-sensitive regex grep, so raw rarity is the wrong sort:
/// the rarest task token is often a lowercase prose word (`measurand`) that the
/// index counts case-insensitively but the grep then misses against `Measurand`
/// — 0 hits, worse than before. Instead prefer *code identifiers* (camelCase or
/// snake_case: `GetMaxCurrent`, `CurrentGetter`), which grep straight to code,
/// over acronyms (`OCPP`) and prose (`current`) that also match READMEs. Within
/// each class, rarer (lower document frequency) wins; absent tokens (df 0) sink.
/// Keys are lowercased in `doc_freqs`; a stable sort keeps first-seen order on
/// ties, so a task with no identifiers degrades to the previous rarity order.
fn rank_by_doc_freq(
    keywords: &[String],
    doc_freqs: &std::collections::HashMap<String, usize>,
) -> Vec<String> {
    let df = |kw: &String| match doc_freqs.get(&kw.to_ascii_lowercase()) {
        Some(&n) if n > 0 => n,
        _ => usize::MAX,
    };
    // Class 0 = code identifier (grep-friendly), class 1 = acronym/prose.
    let rank_key = |kw: &String| (u8::from(!is_code_identifier(kw)), df(kw));
    let mut ranked = keywords.to_vec();
    ranked.sort_by_key(rank_key);
    ranked
}

/// True for tokens that read as code identifiers — snake_case (`get_max_current`)
/// or camelCase/PascalCase with an internal capital (`GetMaxCurrent`). A leading
/// capital alone (`Current`) or an all-caps acronym (`OCPP`) does not qualify:
/// those match prose and file boilerplate as readily as code.
fn is_code_identifier(kw: &str) -> bool {
    if kw.contains('_') {
        return true;
    }
    let has_lower = kw.chars().any(|c| c.is_ascii_lowercase());
    let internal_upper = kw.chars().skip(1).any(|c| c.is_ascii_uppercase());
    has_lower && internal_upper
}

/// Fetch the already-resident BM25 index for `project_root` without triggering a
/// build. Returns `None` when nothing is cached yet (cold start).
fn resident_index(
    project_root: &str,
) -> Option<std::sync::Arc<crate::core::bm25_index::BM25Index>> {
    let cache = crate::tools::impl_semantic_search::get_thread_cache()?;
    crate::core::bm25_cache::get_or_background(&cache, std::path::Path::new(project_root))
}

/// Run the semantic ranking stage under a wall-time budget. Returns the ranked
/// block on time, or a short "deferred" note if the (cold) build overruns —
/// in which case the detached worker keeps running to warm the resident cache.
fn ranked_files_budgeted(task: &str, project_root: &str, crp_mode: CrpMode) -> String {
    let shared_cache = crate::tools::impl_semantic_search::get_thread_cache();
    let (tx, rx) = mpsc::channel::<String>();
    let task_owned = task.to_string();
    let root_owned = project_root.to_string();

    std::thread::spawn(move || {
        if let Some(cache) = shared_cache {
            crate::tools::impl_semantic_search::set_thread_cache(cache);
        }
        let ranked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::tools::impl_semantic_search::handle(
                &task_owned,
                &root_owned,
                8,
                crp_mode,
                None,
                None,
                None,
                Some(false),
            )
        }))
        .unwrap_or_else(|_| {
            tracing::warn!("[ctx_compose: semantic ranking panicked; omitting section]");
            String::new()
        });
        // Receiver may be gone (we timed out); dropping the result is fine —
        // the cache warming already happened as a side effect of the build.
        let _ = tx.send(ranked);
    });

    match rx.recv_timeout(semantic_budget()) {
        Ok(ranked) => ranked.trim().to_string(),
        Err(_) => deferred_ranking_note(project_root),
    }
}

/// Honest, state-aware note when semantic ranking overruns its wall-time budget.
///
/// The old message always promised ranking would be "instant on the next call".
/// That is a lie when the index build *failed* or the index is too large to
/// persist — in those cases every call rebuilds and the promise never comes
/// true (issue #249: "keeps saying it's warming up … but it never happens").
/// We now read the real orchestrator state and tell the agent exactly what is
/// happening and what to do about it.
fn deferred_ranking_note(project_root: &str) -> String {
    let exact = "the exact matches below are authoritative for this call";
    let s = crate::core::index_orchestrator::bm25_summary(project_root);
    match s.state {
        "failed" => {
            let why = s
                .last_error
                .or(s.note)
                .unwrap_or_else(|| "unknown error".to_string());
            format!(
                "(semantic ranking unavailable — index build FAILED: {why}. {exact}. \
                 Inspect with `lean-ctx index status` / `lean-ctx doctor`, then `lean-ctx index build`)"
            )
        }
        "building" => {
            let secs = s.elapsed_ms.map_or(0, |ms| ms / 1000);
            format!(
                "(deferred — semantic index is building ({secs}s elapsed); {exact}, \
                 and ranking becomes available once the build finishes)"
            )
        }
        // ready/idle: this call's cold build just overran the budget. If the
        // index could not be persisted (too large), surface that — otherwise it
        // silently rebuilds on every cold start and never gets faster.
        _ => match s.note {
            Some(note) if note.contains("NOT persisted") => {
                format!("(semantic ranking deferred — {note} {exact}.)")
            }
            _ => format!(
                "(deferred — semantic index is warming; {exact}, \
                 and ranking will be fast on the next call once the index is cached)"
            ),
        },
    }
}

/// Compose a single rich response for `task`.
pub fn handle(task: &str, project_root: &str, crp_mode: CrpMode) -> (String, usize) {
    let task = task.trim();
    if task.is_empty() {
        return ("ERROR: task is required".to_string(), 0);
    }

    let keywords = extract_keywords(task, 6);

    let mut out = String::new();
    out.push_str(&format!("TASK: {task}\n"));
    if keywords.is_empty() {
        out.push_str("KEYWORDS: (none extracted — using full task for ranking)\n");
    } else {
        out.push_str(&format!("KEYWORDS: {}\n", keywords.join(", ")));
    }

    // 1. Semantically ranked files for the whole task — budgeted so a cold
    //    BM25 build can never stall the agent loop (hardening H1). The worker
    //    inherits the resident cache, so a build that overruns the budget still
    //    warms the cache for the next call rather than being wasted.
    out.push_str("\n## Ranked files (semantic)\n");
    out.push_str(&ranked_files_budgeted(task, project_root, crp_mode));
    out.push('\n');

    // 2. Exact match locations for the most specific identifier-shaped keyword.
    // Broad prose words and acronyms create repository-wide README/Dockerfile
    // noise. Within identifiers, the resident index ranks the rarest one first.
    let ranked_keywords = order_by_specificity(&keywords, project_root);
    if let Some(primary) = ranked_keywords
        .iter()
        .find(|keyword| is_code_identifier(keyword))
    {
        let grep = crate::tools::ctx_search::handle_filtered(
            primary,
            project_root,
            None,
            10,
            crp_mode,
            true,
            false,
            None,
            None,
        )
        .text;
        out.push_str(&format!("\n## Exact matches: '{primary}'\n"));
        out.push_str(grep.trim());
        out.push('\n');
    }

    // 3. Inline the symbol bodies that best cover the task keywords. Rather
    //    than just the first match, select the non-redundant *set* of symbols
    //    with maximal keyword coverage under a token budget via submodular
    //    greedy (1−1/e optimal). Two keywords resolving to the same symbol, or
    //    a symbol whose body adds no new keyword, are naturally pruned.
    use crate::core::context_packing::{CoverageItem, greedy_max_coverage};
    let mut snippets: Vec<String> = Vec::new();
    let mut items: Vec<CoverageItem> = Vec::new();
    for kw in &keywords {
        if let Some((rendered, toks)) =
            crate::tools::impl_symbol::best_symbol_snippet_for_task(kw, task, project_root)
        {
            // The snippet always covers its triggering keyword, plus any other
            // task keyword its body textually surfaces (a more central symbol).
            let mut terms: std::collections::HashSet<String> =
                std::collections::HashSet::from([kw.clone()]);
            for other in &keywords {
                if other != kw && rendered.contains(other.as_str()) {
                    terms.insert(other.clone());
                }
            }
            items.push(CoverageItem {
                terms,
                cost: toks.max(1),
            });
            snippets.push(rendered);
        }
    }
    if !items.is_empty() {
        let chosen = greedy_max_coverage(&items, symbol_budget_tokens(), |_| 1.0);
        let mut seen = std::collections::HashSet::new();
        let mut header_written = false;
        for idx in chosen {
            let rendered = snippets[idx].trim();
            if rendered.is_empty() || !seen.insert(rendered.to_string()) {
                continue;
            }
            if !header_written {
                out.push_str("\n## Top symbols (bodies)\n");
                header_written = true;
            }
            out.push_str(rendered);
            out.push('\n');
        }
    }

    // 4. Associative neighbours via spreading activation over the import/call
    //    graph unified with the learned Hebbian co-access graph (budgeted,
    //    additive — surfaces structurally-close files lexical search misses).
    out.push_str(&associative_block_budgeted(project_root, &keywords));

    let sent = count_tokens(&out);
    (out, sent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_by_doc_freq_puts_rare_identifier_first() {
        // The evcc/#993 shape: an "OCPP … GetMaxCurrent" task. `Current` and
        // `OCPP` are common tokens; `GetMaxCurrent` is rare. The rare one must
        // seed the exact-match grep so it lands on code, not README/Dockerfile.
        let keywords = vec![
            "OCPP".to_string(),
            "GetMaxCurrent".to_string(),
            "Current".to_string(),
        ];
        let doc_freqs = std::collections::HashMap::from([
            ("ocpp".to_string(), 120),
            ("current".to_string(), 400),
            ("getmaxcurrent".to_string(), 3),
        ]);
        let ranked = rank_by_doc_freq(&keywords, &doc_freqs);
        assert_eq!(ranked.first().unwrap(), "GetMaxCurrent");
        assert_eq!(ranked.last().unwrap(), "Current");
    }

    #[test]
    fn rank_by_doc_freq_sinks_absent_tokens_and_is_stable() {
        // A token absent from the corpus (df 0) is useless as a grep seed and
        // must sort last; equal-df tokens keep their original order.
        let keywords = vec![
            "absent".to_string(),
            "alpha".to_string(),
            "beta".to_string(),
        ];
        let doc_freqs =
            std::collections::HashMap::from([("alpha".to_string(), 5), ("beta".to_string(), 5)]);
        let ranked = rank_by_doc_freq(&keywords, &doc_freqs);
        assert_eq!(ranked, vec!["alpha", "beta", "absent"]);
    }

    #[test]
    fn rank_prefers_code_identifier_over_rarer_prose_word() {
        // The regression the case-sensitive grep exposed: a rarer lowercase prose
        // token (`measurand`, df 4) must NOT beat a camelCase identifier
        // (`GetMaxCurrent`, df 30) as the seed — the identifier greps to code,
        // the prose word whiffs against `Measurand`.
        let keywords = vec!["measurand".to_string(), "GetMaxCurrent".to_string()];
        let doc_freqs = std::collections::HashMap::from([
            ("measurand".to_string(), 4),
            ("getmaxcurrent".to_string(), 30),
        ]);
        let ranked = rank_by_doc_freq(&keywords, &doc_freqs);
        assert_eq!(ranked.first().unwrap(), "GetMaxCurrent");
    }

    #[test]
    fn is_code_identifier_classifies_camel_snake_vs_prose_and_acronym() {
        assert!(is_code_identifier("GetMaxCurrent"));
        assert!(is_code_identifier("CurrentGetter"));
        assert!(is_code_identifier("get_max_current"));
        // Leading-cap word and all-caps acronym are not code identifiers.
        assert!(!is_code_identifier("Current"));
        assert!(!is_code_identifier("OCPP"));
        assert!(!is_code_identifier("charger"));
    }

    #[test]
    fn extract_keywords_drops_stopwords_and_short_tokens() {
        let kw = extract_keywords("How does the BM25Index cache work for ctx_search?", 6);
        assert!(kw.contains(&"BM25Index".to_string()));
        assert!(kw.contains(&"cache".to_string()));
        assert!(kw.contains(&"ctx_search".to_string()));
        assert!(!kw.iter().any(|k| k == "the" || k == "How" || k == "for"));
    }

    #[test]
    fn extract_keywords_dedups_and_caps() {
        let kw = extract_keywords("alpha alpha beta gamma delta epsilon zeta eta", 3);
        assert_eq!(kw.len(), 3);
        assert_eq!(kw[0], "alpha");
    }

    #[test]
    fn exact_matches_choose_specific_identifier_not_first_broad_keyword() {
        let keywords = extract_keywords(
            "OCPP charger GetMaxCurrent Current.Offered measurand CurrentGetter",
            6,
        );
        assert!(keywords.iter().any(|keyword| keyword == "GetMaxCurrent"));
        assert!(keywords.iter().any(|keyword| is_code_identifier(keyword)));
        assert!(!is_code_identifier("OCPP"));

        let prose = extract_keywords("Fix semantic ranking exact matches", 6);
        assert!(prose.iter().all(|keyword| !is_code_identifier(keyword)));
    }

    #[test]
    fn empty_task_is_rejected() {
        let (out, tok) = handle("   ", "/tmp", CrpMode::Off);
        assert!(out.starts_with("ERROR"));
        assert_eq!(tok, 0);
    }

    #[test]
    fn handle_with_a_task_produces_a_composition() {
        let (output, tokens) = handle("find authentication bugs", "/tmp/nonexistent", CrpMode::Tdd);
        // The kernel may or may not produce output for a nonexistent project,
        // but handle() must not panic.
        assert!(tokens > 0);
        assert!(output.contains("TASK:"));
    }

    #[test]
    fn deferred_note_for_idle_index_is_optimistic_but_honest() {
        // Unknown project → orchestrator state is idle. The note must NOT promise
        // "instant on the next call" (the dishonest wording from #249); it should
        // explain the index is warming and will be fast once cached.
        let tmp = tempfile::tempdir().unwrap();
        let note = deferred_ranking_note(tmp.path().to_string_lossy().as_ref());
        assert!(
            note.contains("warming") || note.contains("building"),
            "note: {note}"
        );
        assert!(
            note.contains("authoritative"),
            "note must reassure that exact matches are authoritative: {note}"
        );
        assert!(
            !note.contains("instant on the next call"),
            "must not repeat the dishonest 'instant next call' promise: {note}"
        );
    }
}
