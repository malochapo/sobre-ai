//! Composable post-processing stages for `LeanCtxServer::call_tool_guarded`
//! (issue #144).
//!
//! `call_tool_guarded` historically inlined a ~1000-line pre/post-dispatch
//! pipeline. The self-contained, side-effect-isolated *stages* live here as
//! small, individually unit-testable functions so the guarded path stays a thin
//! orchestrator. Each function mirrors exactly one concern of the original
//! pipeline; behaviour is unchanged.

use serde_json::{Map, Value};

use crate::core::config::{CompressionLevel, Config};

/// Whether the terse compression stage must be skipped for this call (raw shell,
/// any read-family output, edit evidence, or structural shell output).
///
/// Read-family tools return file content the agent reads and *edits against*. The
/// prose terse pipeline (dictionary `return`→`ret`, `string`→`str`, … plus
/// line-score filtering) corrupts source and drops repeated lines, turning a
/// `mode="full"` read — whose contract is "guaranteed complete content" — into a
/// lossy, un-editable digest. The read tools already apply their own mode-aware,
/// structure-preserving compression (map/signatures/aggressive), so the generic
/// terse layer must never post-process their output. Previously this only skipped
/// when the read had *already saved* tokens, so verbatim `full`/`lines:` reads
/// (0 savings) were silently dictionary-mangled and de-duplicated.
///
/// `ctx_patch` is exempt for the same reason (#382): its `evidence (diff)` block
/// (anchored ops and the `replace_unique` delegate to the edit handler alike)
/// embeds verbatim source lines that must stay byte-accurate — agents treat the
/// evidence as proof of what was written. The rest of its output (status line,
/// pre/postimage hashes) is already minimal, so terse has nothing to gain.
fn skip_terse(name: &str, args: Option<&Map<String, Value>>, is_raw_shell: bool) -> bool {
    let mode = crate::server::helpers::get_str(args, "mode");
    is_raw_shell
        || crate::core::terse::is_verbatim_read(name, mode.as_deref())
        || name == "ctx_patch"
        || (name == "ctx_shell"
            && crate::server::helpers::get_str(args, "command")
                .is_some_and(|c| crate::shell::compress::has_structural_output(&c)))
}

/// Apply the session terse-compression stage. Returns the (possibly) compressed
/// text; the original is returned untouched when compression is inactive, must
/// be skipped, or fails the quality/savings gate. Condensed output ends with a
/// one-line label, so the agent knows it is not reading the tool's raw text.
pub(super) fn compress_terse(
    text: String,
    name: &str,
    args: Option<&Map<String, Value>>,
    config: &Config,
    is_raw_shell: bool,
) -> String {
    if skip_terse(name, args, is_raw_shell) {
        return text;
    }
    let compression = CompressionLevel::effective(config);
    if !compression.is_active() {
        return text;
    }
    let terse_result = crate::core::terse::pipeline::compress(&text, &compression, None);
    if terse_result.quality_passed && terse_result.savings_pct >= 3.0 {
        format!(
            "{}\n[sobre: condensed at compression {}, {:.0}% smaller]",
            terse_result.output.trim_end(),
            compression.label(),
            terse_result.savings_pct
        )
    } else {
        text
    }
}

/// Final sent-token count: the cached pre-terse count when post-processing
/// left the text unchanged, a recount otherwise.
pub(super) fn final_token_count(
    result_text: &str,
    pre_terse_len: usize,
    output_tokens: u64,
) -> usize {
    if result_text.len() == pre_terse_len {
        usize::try_from(output_tokens).unwrap_or(usize::MAX)
    } else {
        crate::core::tokens::count_tokens(result_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_terse_for_raw_shell_and_reads() {
        assert!(skip_terse("ctx_shell", None, true), "raw shell skips terse");
        // Reads always skip terse — even a verbatim `full` read that saved 0 tokens —
        // so file content stays byte-faithful for editing (no `return`→`ret` mangling,
        // no de-dup of repeated lines).
        assert!(
            skip_terse("ctx_read", None, false),
            "full/verbatim read (0 savings) must still skip terse"
        );
        assert!(
            !skip_terse("ctx_search", None, false),
            "ordinary tool output is eligible for terse"
        );
        // #382: ctx_patch embeds verbatim source in its evidence diff (anchored
        // ops and the replace_unique delegate alike) — the terse dictionary
        // (`return`→`ret`) and line-score filtering corrupted it into a false
        // "the edit went wrong" signal for agents.
        assert!(
            skip_terse("ctx_patch", None, false),
            "edit evidence must stay byte-accurate"
        );
    }

    #[test]
    fn skip_terse_mode_based_guard_protects_verbatim() {
        use serde_json::json;
        let full_args = json!({"mode": "full"}).as_object().unwrap().clone();
        let lines_args = json!({"mode": "lines:1-20"}).as_object().unwrap().clone();
        let map_args = json!({"mode": "map"}).as_object().unwrap().clone();
        // Defense in depth: a non-read tool requesting a verbatim mode is still
        // protected, so a future read path is byte-exact before it is named (#404).
        assert!(skip_terse("ctx_future_reader", Some(&full_args), false));
        assert!(skip_terse("ctx_x", Some(&lines_args), false));
        // A non-read tool with a lossy/structured mode stays terse-eligible.
        assert!(!skip_terse("ctx_search", Some(&map_args), false));
    }

    #[test]
    fn compress_terse_keeps_reads_byte_identical() {
        // Content laced with terse triggers (dictionary words, blank lines and a
        // UTF-8 BOM) that the prose pipeline would otherwise mangle. A read must
        // come back byte-for-byte even with the densest compression level (#404).
        let content = "\u{feff}fn run() {\n\n    // command execution pipeline\n    let command = execution();\n    return command;\n}\n";
        let cfg = Config {
            compression_level: CompressionLevel::Max.value(),
            ..Config::default()
        };
        let out = compress_terse(content.to_string(), "ctx_read", None, &cfg, false);
        assert_eq!(
            out, content,
            "ctx_read output must be byte-identical (#404)"
        );
    }

    #[test]
    fn condensed_output_is_labelled() {
        // Pretty-printed JSON always compacts (the json-compact layer).
        let text = format!(
            "{{\n{}\n}}\n",
            (0..40)
                .map(|i| format!("    \"key_{i}\": \"value number {i}\""))
                .collect::<Vec<_>>()
                .join(",\n")
        );
        let cfg = Config {
            compression_level: CompressionLevel::Max.value(),
            ..Config::default()
        };
        let out = compress_terse(text.clone(), "ctx_search", None, &cfg, false);
        assert_ne!(out, text, "this input is expected to condense");
        assert!(
            out.ends_with("% smaller]"),
            "condensed output must carry its label: {out}"
        );
    }

    #[test]
    fn final_token_count_uses_cached_count_when_unchanged() {
        let text = "hello world";
        assert_eq!(final_token_count(text, text.len(), 42), 42);
    }
}
