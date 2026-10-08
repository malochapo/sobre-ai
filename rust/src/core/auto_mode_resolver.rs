use crate::core::cache::SessionCache;

pub struct AutoModeContext<'a> {
    pub path: &'a str,
    pub token_count: usize,
    pub task: Option<&'a str>,
    pub cache: Option<&'a SessionCache>,
}

pub struct ResolvedMode {
    pub mode: String,
    pub source: &'static str,
}

/// Single entry point for auto-mode resolution.
/// Merges Pipeline A (select_mode_with_task) and Pipeline B (resolve_auto_mode).
pub fn resolve(ctx: &AutoModeContext) -> ResolvedMode {
    // Quality loop (#1008): a sobre_patch anchor went stale — hand back fresh line
    // anchors (not `full`) so the agent retries by reference, one-shot. Checked
    // before the `full` escalation: it is the strictly better recovery for a
    // model already editing via anchors.
    if crate::core::edit_quality::take_pending_anchored_escalation(ctx.path) {
        return resolved("anchored", "anchored_edit_fail_escalation");
    }

    // Quality loop (#494), signal 1: an edit on this file just failed after a
    // compressed read — the agent needs the real body now, one-shot.
    if crate::core::edit_quality::take_pending_escalation(ctx.path) {
        return resolved("full", "edit_fail_escalation");
    }

    let r = resolve_inner(ctx);

    // Quality loop (#494), signal 2: this mode keeps producing edit failures
    // for this file type — compression here is a proven net loss, use full.
    if r.mode != "full" && crate::core::edit_quality::is_risky_mode(ctx.path, &r.mode) {
        return resolved("full", "edit_quality_penalty");
    }
    r
}

fn resolve_inner(ctx: &AutoModeContext) -> ResolvedMode {
    if crate::tools::sobre_read::is_instruction_file(ctx.path) {
        return resolved("full", "instruction_file");
    }

    if crate::core::binary_detect::is_binary_file(ctx.path) {
        return resolved("full", "binary");
    }

    if let Some(cache) = ctx.cache
        && let Some(cached) = cache.get(ctx.path)
    {
        if !file_unchanged(ctx.path, cached) {
            return resolved("diff", "cache_changed");
        }
        // Unchanged. Resolving to "full" is only a *cheap* stub hit when full
        // content was actually delivered before. If the first read was a
        // compressed mode (map/signatures), `full_content_delivered` is false,
        // so forcing "full" here re-delivers the entire file on the very next
        // read — a compression bounce that costs *more* tokens than the first
        // read and collapses the cache hit rate: the 2nd read of every file
        // blows up to full and stub hits only begin at the 3rd read (which
        // agents rarely reach). Only short-circuit once full was delivered;
        // otherwise fall through to the predictor, which deterministically
        // reproduces the cached compressed mode and serves it from the
        // compressed-output cache as a cheap, consistent hit.
        if cache.is_full_delivered(ctx.path) {
            return resolved("full", "cache_hit");
        }
    }

    if ctx.token_count <= 200 {
        return resolved("full", "small_file");
    }

    let ext = std::path::Path::new(ctx.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    if is_config_or_data(ext, ctx.path) {
        return resolved("full", "config_data");
    }

    // Active compiler error (#499): the agent reads this file to fix the
    // build — compressed modes would hide the error region.
    if crate::core::diagnostics_store::has_error(ctx.path) {
        return resolved("full", "active_diagnostic");
    }

    // Suspect file (#361 capability): the task explicitly names this file
    // (e.g. "fix the version sort in versioncmp.c"), so the agent is about to
    // inspect it for the defect. Keep the full body it needs to localize and
    // edit, ahead of any task-type intent default that might compress it.
    if task_names_file(ctx.task, ctx.path) {
        return resolved("full", "task_suspect_file");
    }

    if let Some(mode) = intent_recommended_mode(ctx.task) {
        return resolved(&mode, "intent");
    }

    // The capability guards above plus the deterministic heuristic below make
    // `auto` a pure function of (file, task) — byte-stable for provider prompt
    // caching (#498).

    // Deterministic cold-read fallback. Every read that reaches here missed the
    // session cache (a warm hit returns `full`/`diff` above). Structure-first is
    // how sobre reads cold code: a medium source file comes back as `map`, not
    // `full`, so the agent localizes before paying for the body. All capability
    // guards (diagnostic / edit-fail / intent) already ran above, and the
    // anti-inflation guarantee keeps `map` break-even at worst. Medium code
    // (up to 6000 tokens) is labelled `structure_first`; larger code maps on
    // size alone.
    let heuristic = heuristic_mode(ext, ctx.token_count);
    let source = if heuristic == "map" && ctx.token_count <= 6000 {
        "structure_first"
    } else {
        "heuristic"
    };
    resolved(&heuristic, source)
}

/// True when the task text explicitly names this file (basename match). A real
/// filename mention ("versioncmp.c") is a strong suspect signal for a bug-fix;
/// requiring an extension-bearing, non-trivial basename keeps it precise — the
/// bare stem in "improve the parser" must not match `parser.rs`. A rare false
/// positive only costs a little compression on a file the user literally named,
/// so the failure mode is capability-safe.
fn task_names_file(task: Option<&str>, path: &str) -> bool {
    let Some(task) = task else {
        return false;
    };
    let basename = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if basename.len() < 4 || !basename.contains('.') {
        return false;
    }
    task.to_ascii_lowercase()
        .contains(&basename.to_ascii_lowercase())
}

fn intent_recommended_mode(task: Option<&str>) -> Option<String> {
    let task_desc = task?;
    let classification = crate::core::intent_engine::classify(task_desc);
    if classification.confidence < 0.4 {
        return None;
    }
    let route = crate::core::intent_engine::route_intent(task_desc, &classification);
    let mode =
        crate::core::intent_router::read_mode_for_tier(route.model_tier, classification.task_type);
    if mode == "auto" {
        return None;
    }
    Some(mode)
}

fn heuristic_mode(ext: &str, token_count: usize) -> String {
    if token_count > 8000 {
        if is_code(ext) {
            return "map".to_string();
        }
        return "aggressive".to_string();
    }
    // Structure-first cold-read floor (#361): on a phase-isolated harness a cold
    // `full` read never amortizes, so code files default to `map` (deps +
    // exports + key signatures) — cheaper and a better localization surface.
    // `map` keeps far more than `signatures` (no empty bodies), so the
    // follow-up-read risk is low; the 500-token floor stays above the trivial
    // files where `full` is already best.
    if token_count > 500 && is_code(ext) {
        return "map".to_string();
    }
    "full".to_string()
}

/// Fast O(1) staleness check: if the file's mtime still matches what was
/// stored when the cache entry was created, the content is unchanged — no need
/// to read the file or compute any hash. Falls back to "changed" when metadata
/// is unavailable (e.g. file deleted) or when the cache entry predates mtime
/// tracking (legacy entries with `stored_mtime = None`).
///
/// mtime comparison is sufficient for correctness on all major filesystems:
/// every `write(2)` / `truncate(2)` updates mtime (POSIX guarantee).
fn file_unchanged(path: &str, cached: &crate::core::cache::CacheEntry) -> bool {
    let Some(stored_mtime) = cached.stored_mtime else {
        return false;
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(current_mtime) = meta.modified() else {
        return false;
    };
    current_mtime == stored_mtime
}

fn is_code(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "java"
            | "c"
            | "cpp"
            | "cc"
            | "h"
            | "hpp"
            | "rb"
            | "cs"
            | "kt"
            | "swift"
            | "php"
            | "zig"
            | "ex"
            | "exs"
            | "scala"
            | "sc"
            | "dart"
            | "sh"
            | "bash"
            | "svelte"
            | "vue"
    )
}

fn is_config_or_data(ext: &str, path: &str) -> bool {
    if matches!(ext, "xml" | "ini" | "cfg" | "env") {
        return true;
    }
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    matches!(
        name,
        "Cargo.toml"
            | "package.json"
            | "tsconfig.json"
            | "Makefile"
            | "Dockerfile"
            | "docker-compose.yml"
            | ".gitignore"
            | ".env"
            | "pyproject.toml"
            | "go.mod"
            | "build.gradle"
            | "pom.xml"
    )
}

fn resolved(mode: &str, source: &'static str) -> ResolvedMode {
    ResolvedMode {
        mode: mode.to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_file_always_full() {
        let ctx = AutoModeContext {
            path: "test.rs",
            token_count: 100,
            task: None,
            cache: None,
        };
        let result = resolve(&ctx);
        assert_eq!(result.mode, "full");
        assert_eq!(result.source, "small_file");
    }

    #[test]
    fn config_file_returns_full() {
        let ctx = AutoModeContext {
            path: "config.ini",
            token_count: 500,
            task: None,
            cache: None,
        };
        let result = resolve(&ctx);
        assert_eq!(result.mode, "full");
        assert_eq!(result.source, "config_data");
    }

    #[test]
    fn cached_compressed_only_file_does_not_escalate_to_full() {
        // Cache regression: a file first read in a compressed mode has
        // `full_content_delivered=false`. Resolving its re-read to "full" would
        // re-deliver the entire file on the 2nd read — a compression bounce that
        // costs more tokens than the first read and defeats the cache. The
        // resolver must fall through so the cached compressed mode is reused.
        //
        // Sized > 6000 tokens so the deterministic default heuristic itself
        // picks a compressed mode (`map` for large code) — making the
        // compressed-only premise real even with learning off (#683); the
        // invariant under test is that the `cache_hit` shortcut never fires for
        // an entry whose full body was not delivered.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("large.rs");
        let body = "fn placeholder() { let _ = 1; }\n".repeat(900);
        std::fs::write(&file, &body).unwrap();
        let path = file.to_str().unwrap();

        let mut cache = SessionCache::new();
        cache.store(path, &body);
        // A compressed first read does NOT mark full content as delivered.

        let ctx = AutoModeContext {
            path,
            token_count: 7000,
            task: None,
            cache: Some(&cache),
        };
        let result = resolve(&ctx);
        assert_ne!(
            result.mode, "full",
            "compressed-only cached file must not escalate to full on re-read"
        );
        assert_ne!(result.source, "cache_hit");
    }

    #[test]
    fn cached_full_delivered_file_short_circuits_to_stub() {
        // Once full content was actually delivered, the cache_hit shortcut still
        // applies: a re-read resolves to "full" (a cheap `[unchanged]` stub).
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("medium.rs");
        let body = "fn placeholder() { let _ = 1; }\n".repeat(400);
        std::fs::write(&file, &body).unwrap();
        let path = file.to_str().unwrap();

        let mut cache = SessionCache::new();
        cache.store(path, &body);
        cache.mark_full_delivered(path);

        let ctx = AutoModeContext {
            path,
            token_count: 3000,
            task: None,
            cache: Some(&cache),
        };
        let result = resolve(&ctx);
        assert_eq!(result.mode, "full");
        assert_eq!(result.source, "cache_hit");
    }

    #[test]
    fn intent_explore_returns_map() {
        let ctx = AutoModeContext {
            path: "large.rs",
            token_count: 5000,
            task: Some("how does the cache work?"),
            cache: None,
        };
        let result = resolve(&ctx);
        assert_eq!(result.mode, "map");
        assert_eq!(result.source, "intent");
    }

    #[test]
    fn task_names_file_matches_explicit_filename() {
        assert!(task_names_file(
            Some("fix the version sort in versioncmp.c"),
            "src/versioncmp.c"
        ));
        assert!(task_names_file(
            Some("why does graph.ts loop?"),
            "web/src/graph.ts"
        ));
    }

    #[test]
    fn task_names_file_ignores_bare_stems_and_trivia() {
        // A bare stem mention must not match the file.
        assert!(!task_names_file(
            Some("improve the parser"),
            "src/parser.rs"
        ));
        assert!(!task_names_file(None, "src/parser.rs"));
        // Trivial / extension-less basenames are excluded.
        assert!(!task_names_file(Some("touch a.c"), "a.c"));
        assert!(!task_names_file(Some("look at Makefile"), "Makefile"));
    }

    #[test]
    fn task_suspect_file_overrides_intent() {
        let _lock = crate::core::data_dir::test_env_lock();
        let dir = std::env::temp_dir().join(format!("sobre-amr-suspect-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        crate::test_env::set_var("SOBRE_DATA_DIR", dir.to_str().unwrap());

        // An explore-style task that would otherwise map (cf.
        // intent_explore_returns_map) — but it names the file, so the suspect
        // guard keeps the full body for localization.
        let ctx = AutoModeContext {
            path: "large.rs",
            token_count: 5000,
            task: Some("how does large.rs build the cache?"),
            cache: None,
        };
        let result = resolve(&ctx);
        assert_eq!(result.mode, "full");
        assert_eq!(result.source, "task_suspect_file");

        crate::test_env::remove_var("SOBRE_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn heuristic_structure_first_maps_medium_code() {
        // Structure-first: medium code becomes `map` on a cold read.
        assert_eq!(heuristic_mode("rs", 1500), "map");
        assert_eq!(heuristic_mode("c", 800), "map");
    }

    #[test]
    fn heuristic_structure_first_keeps_tiny_and_prose_full() {
        // Below the 500-token floor `full` is already best.
        assert_eq!(heuristic_mode("rs", 400), "full");
        // Non-code (prose / data) is never structure-first mapped.
        assert_eq!(heuristic_mode("md", 4000), "full");
        assert_eq!(heuristic_mode("txt", 1000), "full");
    }

    #[test]
    fn heuristic_large_code_maps_regardless() {
        assert_eq!(heuristic_mode("rs", 9000), "map");
    }

    /// Bug-fix read pattern: while localizing a planted defect the agent reads
    /// many medium source files cold. The structure-first resolver returns
    /// `map` (cheap, localization-friendly) instead of an un-amortized `full`,
    /// while every capability guard still takes precedence because it runs
    /// before this fallback (here: the small-file guard keeps a tiny file full).
    #[test]
    fn structure_first_resolve_bugfix_cold_read() {
        let _lock = crate::core::data_dir::test_env_lock();
        let dir = std::env::temp_dir().join(format!("sobre-amr-sf-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        crate::test_env::set_var("SOBRE_DATA_DIR", dir.to_str().unwrap());

        let suspect = AutoModeContext {
            path: "src/versioncmp.c",
            token_count: 1500,
            task: None,
            cache: None,
        };
        let result = resolve(&suspect);
        assert_eq!(result.mode, "map");
        assert_eq!(result.source, "structure_first");

        let tiny = AutoModeContext {
            path: "src/util.c",
            token_count: 120,
            task: None,
            cache: None,
        };
        assert_eq!(resolve(&tiny).mode, "full");

        crate::test_env::remove_var("SOBRE_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #683: with learning off (the default), a medium code file with no task
    /// resolves through the deterministic size heuristic — never a learning
    /// source — and is byte-stable across repeated calls.
    #[test]
    fn learning_off_by_default_is_deterministic() {
        let _lock = crate::core::data_dir::test_env_lock();
        let dir = std::env::temp_dir().join(format!("sobre-amr-det-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        crate::test_env::set_var("SOBRE_DATA_DIR", dir.to_str().unwrap());

        let ctx = AutoModeContext {
            path: "src/widget.rs",
            token_count: 1500,
            task: None,
            cache: None,
        };
        let a = resolve(&ctx);
        let b = resolve(&ctx);
        assert_eq!(a.mode, "map");
        assert_eq!(a.source, "structure_first");
        assert_eq!((a.mode, a.source), (b.mode, b.source));

        crate::test_env::remove_var("SOBRE_DATA_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
