//! Composable post-processing stages for `SobreServer::call_tool_guarded`
//! (issue #144).
//!
//! `call_tool_guarded` historically inlined a ~1000-line pre/post-dispatch
//! pipeline. The self-contained, side-effect-isolated *stages* live here as
//! small, individually unit-testable functions so the guarded path stays a thin
//! orchestrator. Each function mirrors exactly one concern of the original
//! pipeline; behaviour is unchanged.

use serde_json::{Map, Value};

use crate::core::config::{CompressionLevel, Config};

/// The tools whose text the terse stage may condense: prose about what was
/// recorded (`sobre_knowledge`, `sobre_session`) and shell output, whose original
/// stays reachable through the footer's `sobre_expand` id. Every other tool
/// returns content the agent acts on — file text, search hits, signatures,
/// diffs, paths, the original `sobre_expand` returns — where the dictionary and
/// line-dropping layers would change what the agent reads.
const TERSE_TOOLS: [&str; 3] = ["sobre_shell", "sobre_knowledge", "sobre_session"];

/// Whether the terse stage must leave this call's output alone: any tool not
/// in [`TERSE_TOOLS`], raw shell, and shell output with structure
/// (`git diff`, tables) that line scoring would break.
fn skip_terse(name: &str, args: Option<&Map<String, Value>>, is_raw_shell: bool) -> bool {
    !TERSE_TOOLS.contains(&name)
        || is_raw_shell
        || (name == "sobre_shell"
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
    fn terse_reaches_only_prose_and_shell_output() {
        for tool in ["sobre_shell", "sobre_knowledge", "sobre_session"] {
            assert!(!skip_terse(tool, None, false), "{tool} is condensable");
        }
        // Content the agent acts on stays as the tool returned it: file text,
        // the original sobre_expand returns, edit evidence (#382), search hits,
        // signatures, diffs and paths.
        for tool in [
            "sobre_read",
            "sobre_expand",
            "sobre_patch",
            "sobre_search",
            "sobre_outline",
            "sobre_compose",
            "sobre_refactor",
            "sobre_checkpoint",
            "sobre_review",
            "sobre_tree",
            "sobre_glob",
            "sobre_graph",
        ] {
            assert!(skip_terse(tool, None, false), "{tool} must stay verbatim");
        }
        assert!(
            skip_terse("sobre_shell", None, true),
            "raw shell skips terse"
        );
    }

    #[test]
    fn compress_terse_keeps_reads_byte_identical() {
        // Content laced with terse triggers (dictionary words, blank lines and a
        // UTF-8 BOM) that the prose pipeline would otherwise mangle. A read must
        // come back byte-for-byte even with the densest compression level (#404).
        let content = "\u{feff}fn run() {\n\n    // command execution pipeline\n    let command = execution();\n    return command;\n}\n";
        let cfg = Config {
            compression_level: 0.7,
            ..Config::default()
        };
        let out = compress_terse(content.to_string(), "sobre_read", None, &cfg, false);
        assert_eq!(
            out, content,
            "sobre_read output must be byte-identical (#404)"
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
            compression_level: 0.7,
            ..Config::default()
        };
        let out = compress_terse(text.clone(), "sobre_shell", None, &cfg, false);
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
