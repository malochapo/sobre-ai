use super::ccr;
use crate::core::tokens::count_tokens;
use crate::core::web::distill;

/// Byte-ish budget for the research-prose squeeze (~5k tokens on English prose).
/// Only oversized prose is truncated; the squeeze's main job is dedup + blank-collapse,
/// not cutting.
const RESEARCH_PROSE_CAP: usize = 20_000;

fn research_prose_cap() -> usize {
    RESEARCH_PROSE_CAP
}

/// Proxy compression funnel: routes a tool result to the right compressor.
///
/// 1. Already-cited research output (from `ctx_url_read` / the web layer) is kept
///    verbatim — it is distilled and citation-stamped, so the shell pipeline must
///    not touch its footer or claim markers.
/// 2. Prose results (web fetches, doc reads, research MCP bridges) are squeezed
///    by the prose-aware research compressor instead of the log/code-tuned shell
///    engine.
/// 3. Everything else (shell/build/search output) flows through the unified
///    `compress_if_beneficial` pipeline. A `$ ...` command hint is extracted so
///    the pattern engine gets the same routing as the CLI and MCP paths.
///
/// A rewritten result always ends with a `[sobre: …]` line (see [`label`]).
/// Text below [`ccr::MIN_TEE_BYTES`] is never rewritten: its original would
/// not be kept.
pub(crate) fn compress_tool_result(content: &str, tool_name: Option<&str>) -> String {
    if content.len() < ccr::MIN_TEE_BYTES {
        return content.to_string();
    }
    let compressed = compress_inner(content, tool_name);
    label(content, compressed)
}

/// Ends a rewritten tool result with the line that says so (#482): the token
/// counts and where the original is kept in the content-addressed tee store.
/// Both are pure functions of the content, so the rewritten result is
/// byte-stable across turns and never invalidates the provider cache prefix
/// (#448). The original is kept as is when it cannot be stored, or when the
/// labeled rewrite is not smaller.
fn label(original: &str, result: String) -> String {
    if result == original {
        return result;
    }
    let Some(handle) = ccr::persist(original) else {
        return original.to_string();
    };
    let before = count_tokens(original);
    let after = count_tokens(&result);
    let name = std::path::Path::new(&handle)
        .file_name()
        .map_or_else(|| handle.clone(), |n| n.to_string_lossy().into_owned());
    let labeled = format!(
        "{}\n[sobre: compressed by the proxy, {before} → {after} tokens; full original at {handle}, or ctx_expand(id=\"{name}\") for a slice]",
        result.trim_end()
    );
    if labeled.len() < original.len() {
        labeled
    } else {
        original.to_string()
    }
}

fn compress_inner(content: &str, tool_name: Option<&str>) -> String {
    if content.trim().is_empty() || content.len() < 200 {
        return content.to_string();
    }

    // #479: lean-ctx's own MCP tools already applied their compression policy at
    // the tool boundary — honouring `raw=true`/`bypass`, `<lc_safe>` spans and
    // the configured aggressiveness. Their `raw` intent lives in the originating
    // `tool_use` input, which is invisible here, so re-compressing on the wire
    // would silently undo an explicit `raw=true` and double-compress everything
    // else. Pass results from `ctx_*` tools through untouched.
    if tool_name.is_some_and(is_lean_ctx_tool) {
        return content.to_string();
    }

    // #709: honour explicit <lc_safe>…</lc_safe> spans on the proxy path too.
    // Protected spans pass through verbatim; each unprotected segment flows back
    // through the normal funnel (markers are stripped, so this never recurses).
    if crate::core::protect::has_markers(content) {
        return crate::core::protect::compress_preserving(content, |seg| {
            compress_inner(seg, tool_name)
        });
    }

    if extract_command_hint(content).is_none()
        && looks_like_prose(content)
        && let Some(out) = squeeze_research_prose(content)
    {
        return out;
    }

    let cmd = infer_command(content, tool_name);

    // Proxy fidelity guard. A foreign shell tool gives us at most a generic
    // command (`"shell"`) or none, so the engine's command-gated build/test
    // verbatim guards never fire. When the *output* is unmistakably a build or
    // test run, preserve it verbatim (bounded by safety-line-preserving
    // truncation) so compiler errors, panics and test summaries reach the model
    // intact — the exact signal a bug-fix task depends on.
    let generic_command = cmd.is_empty() || cmd == "shell";
    if generic_command
        && (output_looks_like_test_run(content) || output_looks_like_build_failure(content))
    {
        return crate::shell::compress::engine::preserve_verbatim_pub(content);
    }

    crate::shell::compress::engine::compress_if_beneficial(&cmd, content)
}

/// Strong, ecosystem-spanning signals that an output is a *test run* (passing or
/// failing). Conservative — matches the summary/result lines a bug-fix task must
/// never lose. Only consulted on the proxy path when the real command is unknown.
fn output_looks_like_test_run(content: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "test result:",            // rust
        "short test summary info", // pytest
        " passed in ",             // pytest summary
        " failed in ",             // pytest summary
        "=== RUN",                 // go
        "--- FAIL:",               // go
        "--- PASS:",               // go
        "Test Suites:",            // jest
        " examples, ",             // rspec ("5 examples, 0 failures")
        "FAILED",                  // generic test failure marker
    ];
    NEEDLES.iter().any(|n| content.contains(n))
}

/// Strong, specific signals of a build / compile / runtime failure across the
/// major toolchains. Used only on the proxy path for generically-named tools so
/// the failing diagnostics (paths, lines, messages) survive intact.
fn output_looks_like_build_failure(content: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "error[",                            // rustc (E0277 …)
        ": error:",                          // gcc / clang "file.c:12:5: error:"
        "fatal error:",                      // gcc / clang
        "undefined reference to",            // linker
        "panicked at",                       // rust runtime
        "could not compile",                 // cargo
        "Traceback (most recent call last)", // python
        "AssertionError",                    // python / junit
        "make: ***",                         // make
        "Build FAILED",
        "BUILD FAILED",
        "Segmentation fault",
    ];
    NEEDLES.iter().any(|n| content.contains(n))
}

/// Code/shell symbols whose density cleanly separates source/logs from prose.
const CODE_SYMBOLS: &str = "{}<>;=|\\$`";

/// Conservative prose detector: substantial, letter-dense, low code-symbol, with
/// real sentences and long lines. Code, logs, tables and JSON all fail this.
fn looks_like_prose(content: &str) -> bool {
    let sample: String = content.chars().take(4000).collect();
    let total = sample.chars().count();
    if total < 600 {
        return false;
    }
    let total_f = total as f32;
    let alpha = sample.chars().filter(|c| c.is_alphabetic()).count() as f32;
    let spaces = sample.chars().filter(|c| *c == ' ').count() as f32;
    let symbols = sample.chars().filter(|c| CODE_SYMBOLS.contains(*c)).count() as f32;

    if alpha / total_f < 0.6 || spaces / total_f < 0.12 || symbols / total_f > 0.06 {
        return false;
    }
    if sample.matches(['.', '!', '?']).count() < 4 {
        return false;
    }

    let non_empty: Vec<&str> = sample.lines().filter(|l| !l.trim().is_empty()).collect();
    if non_empty.is_empty() {
        return false;
    }
    let avg_len =
        non_empty.iter().map(|l| l.chars().count()).sum::<usize>() as f32 / non_empty.len() as f32;
    avg_len >= 40.0
}

/// Apply the prose squeeze, returning the result only when it
/// actually saves tokens; otherwise `None` so the normal pipeline can try.
fn squeeze_research_prose(content: &str) -> Option<String> {
    let before = count_tokens(content);
    let squeezed = distill::squeeze_prose(content, research_prose_cap());
    if squeezed.trim().is_empty() {
        return None;
    }
    let after = count_tokens(&squeezed);
    if after + 2 >= before {
        return None;
    }
    Some(squeezed)
}

/// True when `name` refers to one of lean-ctx's own `ctx_*` MCP tools, whose
/// results are already compressed at the tool boundary and must not be touched
/// again by the proxy (#479).
///
/// Clients namespace MCP tools differently, so a plain `starts_with("ctx_")`
/// misses the real-world callers: Claude Code (the reporter's setup) sends
/// `mcp__lean-ctx__ctx_shell`, others use `lean-ctx:ctx_read`. Strip the client
/// prefix down to the bare tool segment before matching.
fn is_lean_ctx_tool(name: &str) -> bool {
    let bare = name
        .rsplit("__")
        .next()
        .unwrap_or(name)
        .rsplit([':', '/', '.'])
        .next()
        .unwrap_or(name);
    bare.starts_with("ctx_") || name.starts_with("ctx_")
}

fn infer_command(content: &str, tool_name: Option<&str>) -> String {
    if let Some(cmd) = extract_command_hint(content) {
        return cmd;
    }

    if let Some(name) = tool_name {
        let nl = name.to_lowercase();
        if nl.contains("bash") || nl.contains("shell") || nl.contains("terminal") {
            return "shell".to_string();
        }
        if nl.contains("search") || nl.contains("grep") || nl.contains("find") {
            return "grep".to_string();
        }
    }

    String::new()
}

fn extract_command_hint(content: &str) -> Option<String> {
    for line in content.lines().take(3) {
        let trimmed = line.trim();
        if let Some(cmd) = trimmed.strip_prefix("$ ") {
            return Some(cmd.to_string());
        }
        if let Some(cmd) = trimmed.strip_prefix("% ") {
            return Some(cmd.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #980/#985: search tool results must compress without corrupting source.
    #[test]
    fn search_tool_result_is_compressed_without_corrupting_source() {
        let raw = (0..60)
            .map(|i| {
                format!(
                    "src/h.go:{i}:func handler{i}(ctx context.Context) (api.Result, error) \
                     {{ return doWork(ctx) }}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let out = compress_tool_result(&raw, Some("search_files"));

        assert!(
            out.len() < raw.len(),
            "search results must still compress ({} -> {})",
            raw.len(),
            out.len()
        );
        for keyword in ["context.Context", "(api.Result, error)", "return"] {
            assert!(
                out.contains(keyword),
                "the terse dictionary rewrote `{keyword}` out of a search result:\n{out}"
            );
        }
    }

    /// Invariant 6: a rewrite keeps the original, and text below
    /// [`ccr::MIN_TEE_BYTES`] is not kept, so it is never rewritten.
    #[test]
    fn text_too_small_to_keep_is_never_rewritten() {
        let mut small = String::from("$ cargo build\n");
        for _ in 0..12 {
            small.push_str("src/a.rs:1:1: warning: unused\n");
        }
        assert!(small.len() < ccr::MIN_TEE_BYTES);
        assert_ne!(
            compress_inner(&small, Some("bash")),
            small,
            "the engine alone would rewrite it"
        );
        assert_eq!(compress_tool_result(&small, Some("bash")), small);
    }

    #[test]
    fn short_content_unchanged() {
        let short = "hello world";
        assert_eq!(compress_tool_result(short, None), short);
    }

    #[test]
    fn empty_content_unchanged() {
        assert_eq!(compress_tool_result("", None), "");
        assert_eq!(compress_tool_result("   ", None), "   ");
    }

    #[test]
    fn command_hint_extraction() {
        assert_eq!(
            extract_command_hint("$ cargo build\nCompiling foo"),
            Some("cargo build".to_string())
        );
        assert_eq!(extract_command_hint("no prefix here"), None);
    }

    #[test]
    fn tool_name_inference() {
        assert_eq!(infer_command("some text", Some("bash_execute")), "shell");
        assert_eq!(infer_command("some text", Some("search_files")), "grep");
        assert_eq!(infer_command("some text", Some("unknown_tool")), "");
    }

    #[test]
    fn lean_ctx_tool_results_pass_through_verbatim() {
        // A ctx_shell result the tool already produced (raw=true, or its own
        // compression). The proxy must NOT re-compress / re-truncate it — that
        // was the #479 defect where `raw=true` was silently undone on the wire.
        let raw = (1..=120)
            .map(|i| format!("Line {i:04}: the quick brown fox jumps over the lazy dog"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(raw.len() > 200);
        // Bare names AND the namespaced forms real MCP clients emit (Claude Code
        // `mcp__lean-ctx__ctx_shell`, colon-style `lean-ctx:ctx_read`).
        for tool in [
            "ctx_shell",
            "ctx_read",
            "ctx_search",
            "mcp__lean-ctx__ctx_shell",
            "lean-ctx:ctx_read",
        ] {
            assert_eq!(
                compress_tool_result(&raw, Some(tool)),
                raw,
                "{tool} output must pass through the proxy verbatim"
            );
        }
        // A foreign tool with identical output is still compressed: the proxy
        // keeps adding value for non-lean-ctx tools.
        assert_ne!(
            compress_tool_result(&raw, Some("bash")),
            raw,
            "foreign-tool output should still be compressed by the proxy"
        );
    }

    #[test]
    fn prose_is_squeezed_and_deduped() {
        let para = "Rust is a multi-paradigm systems programming language that \
                    emphasizes performance, type safety, and fearless concurrency, \
                    achieving memory safety without a garbage collector at runtime.";
        // Repeated paragraph (well over the 600-char prose floor) → dedup keeps one.
        let input = format!("{}\n", [para; 8].join("\n\n"));
        assert!(input.len() > 600);
        let out = compress_tool_result(&input, Some("web_fetch"));
        assert_eq!(out.matches("fearless concurrency").count(), 1);
        assert!(out.contains("performance, type safety"));
    }

    #[test]
    fn code_output_is_not_treated_as_prose() {
        let code = "fn main() {\n    let x = vec![1, 2, 3];\n    \
                    for i in &x { println!(\"{}\", i); }\n}\n"
            .repeat(20);
        assert!(!looks_like_prose(&code));
    }

    #[test]
    fn shell_log_is_not_treated_as_prose() {
        let log = "$ cargo build\n   Compiling foo v0.1.0\n    Finished dev\n".repeat(20);
        assert!(!looks_like_prose(&log));
    }

    #[test]
    fn foreign_shell_build_failure_preserved_verbatim() {
        // A forge/pi-style shell tool: the name says "shell" and the output has
        // no `$ cmd` hint, so the engine's command-gated guards cannot fire. The
        // compiler error must still reach the model intact for a bug-fix task.
        let mut log = String::from("gcc -O2 -c src/versioncmp.c -o versioncmp.o\n");
        log.push_str("src/versioncmp.c: In function 'version_cmp':\n");
        log.push_str(
            "src/versioncmp.c:142:17: error: invalid operands to binary < (have 'char *' and 'int')\n",
        );
        for i in 0..40 {
            log.push_str(&format!("  note: expansion context line {i}\n"));
        }
        log.push_str("make: *** [Makefile:23: versioncmp.o] Error 1\n");

        let out = compress_tool_result(&log, Some("shell"));
        assert!(
            out.contains("versioncmp.c:142:17: error:"),
            "compiler error must survive the proxy"
        );
        assert!(
            out.contains("make: ***"),
            "make failure summary must survive"
        );
    }

    #[test]
    fn foreign_shell_test_failure_preserved_verbatim() {
        let mut log = String::from("running 3 tests\n");
        log.push_str("test version::tests::sorts_numeric ... FAILED\n");
        for i in 0..40 {
            log.push_str(&format!("note line {i} with some filler content here\n"));
        }
        log.push_str("test result: FAILED. 2 passed; 1 failed; 0 ignored\n");

        let out = compress_tool_result(&log, Some("bash"));
        assert!(
            out.contains("test result: FAILED"),
            "test summary must survive the proxy"
        );
        assert!(out.contains("sorts_numeric ... FAILED"));
    }

    #[test]
    fn plain_shell_log_not_forced_verbatim() {
        let log = "Listening on port 8080\nRequest received from 10.0.0.2\n".repeat(20);
        assert!(!output_looks_like_test_run(&log));
        assert!(!output_looks_like_build_failure(&log));
    }

    fn big_compressible_log() -> String {
        (1..=400)
            .map(|i| format!("[info] processed item {i:04} ok"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn live_compression_is_recoverable_via_ccr_handle() {
        let _lock = crate::core::data_dir::test_env_lock();
        let log = big_compressible_log();
        let out = compress_tool_result(&log, Some("bash"));
        assert!(
            out.len() < log.len(),
            "a large foreign log must be compressed"
        );

        // The compressed result carries the content-addressed handle, and the
        // handle points at the *verbatim* original — live compression is now
        // non-lossy (#482), recoverable with a plain native file read.
        let handle = ccr::persist(&log).expect("same content -> same handle");
        assert!(out.contains(&handle), "CCR handle must be embedded: {out}");
        let recovered = std::fs::read_to_string(&handle).expect("tee file readable");
        assert!(
            recovered.contains("processed item 0007 ok")
                && recovered.contains("processed item 0400 ok"),
            "verbatim original must be fully recoverable"
        );
    }

    #[test]
    fn live_compression_output_is_byte_stable_across_turns() {
        let _lock = crate::core::data_dir::test_env_lock();
        let log = big_compressible_log();
        let a = compress_tool_result(&log, Some("bash"));
        let b = compress_tool_result(&log, Some("bash"));
        assert_eq!(
            a, b,
            "the CCR handle is content-addressed, so the rewritten result must be \
             byte-identical across turns (provider cache prefix stays valid, #448)"
        );
    }

    /// Invariant 6: every rewrite says so and names the kept original,
    /// including one that saves fewer bytes than [`ccr::MIN_TEE_BYTES`].
    #[test]
    fn every_rewrite_ends_with_a_sobre_label() {
        let _lock = crate::core::data_dir::test_env_lock();
        let mut small_saving =
            String::from("$ git status\nOn branch main\nChanges not staged for commit:\n");
        for i in 0..30 {
            small_saving.push_str(&format!("\tmodified:   src/m{i}.rs\n"));
        }
        let saved = small_saving.len() - compress_inner(&small_saving, Some("bash")).len();
        assert!(saved > 0 && saved < ccr::MIN_TEE_BYTES, "saved {saved}");
        for input in [small_saving, big_compressible_log()] {
            let out = compress_tool_result(&input, Some("bash"));
            assert!(out.len() < input.len(), "{out}");
            let last = out.lines().last().unwrap();
            assert!(
                last.starts_with("[sobre: compressed by the proxy, ") && last.ends_with(']'),
                "a rewrite must end with its label: {out}"
            );
            let handle = ccr::persist(&input).unwrap();
            assert!(
                last.contains(&format!("full original at {handle}")),
                "{last}"
            );
        }
    }
}
