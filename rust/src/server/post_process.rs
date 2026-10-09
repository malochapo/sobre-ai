//! Composable post-processing stages for `SobreServer::call_tool_guarded`.
//!
//! `call_tool_guarded` historically inlined a ~1000-line pre/post-dispatch
//! pipeline. The self-contained, side-effect-isolated *stages* live here as
//! small, individually unit-testable functions so the guarded path stays a thin
//! orchestrator. Each function mirrors exactly one concern of the original
//! pipeline; behaviour is unchanged.

use crate::core::config::{CompressionLevel, Config};

/// The tools whose text the terse stage condenses here: prose about what was
/// recorded (`sobre_knowledge`, `sobre_session`). Shell output is condensed
/// inside `sobre_shell` ([`condense_shell_output`]), before it decides whether
/// to keep the full output, so a condensed answer always carries its
/// `sobre_expand` id. Every other tool returns content the agent acts on —
/// file text, search hits, signatures, diffs, paths, the original
/// `sobre_expand` returns — where the dictionary and line-dropping layers
/// would change what the agent reads.
const TERSE_TOOLS: [&str; 2] = ["sobre_knowledge", "sobre_session"];

/// Whether the terse stage must leave this call's output alone: any tool not
/// in [`TERSE_TOOLS`].
fn skip_terse(name: &str) -> bool {
    !TERSE_TOOLS.contains(&name)
}

/// Condenses a shell command's (already pattern-compressed) output with the
/// terse stage, labelled. Output the command's policy protects
/// (`passthrough_commands`, the built-in passthrough and verbatim commands)
/// and output with structure (`git diff`, tables) is returned as it is.
/// `sobre_shell` runs this before deciding whether to keep the full output,
/// so whatever this cuts stays reachable through `sobre_expand`.
pub(crate) fn condense_shell_output(command: &str, text: String, config: &Config) -> String {
    let protected =
        crate::shell::output_policy::classify(command, &config.passthrough_commands).is_protected();
    if protected || crate::shell::compress::has_structural_output(command) {
        return text;
    }
    let condensed = condense(&text, config);
    if condensed != text
        && crate::core::output_verification::verify_output(&text, &condensed)
            .warnings
            .is_empty()
    {
        condensed
    } else {
        text
    }
}

/// Apply the session terse-compression stage. Returns the (possibly) compressed
/// text; the original is returned untouched when compression is inactive, must
/// be skipped, or fails the quality/savings gate. Condensed output ends with a
/// one-line label, so the agent knows it is not reading the tool's raw text.
pub(super) fn compress_terse(text: String, name: &str, config: &Config) -> String {
    if skip_terse(name) {
        return text;
    }
    condense(&text, config)
}

/// The terse stage at the configured compression, labelled; `text` itself
/// when compression is off, the result loses quality, or it saves under 3%.
fn condense(text: &str, config: &Config) -> String {
    let compression = CompressionLevel::effective(config);
    if !compression.is_active() {
        return text.to_string();
    }
    let terse_result = crate::core::terse::pipeline::compress(text, &compression, None);
    if terse_result.quality_passed && terse_result.savings_pct >= 3.0 {
        format!(
            "{}\n[sobre: condensed at compression {}, {:.0}% smaller]",
            terse_result.output.trim_end(),
            compression.label(),
            terse_result.savings_pct
        )
    } else {
        text.to_string()
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
    fn terse_reaches_only_prose_here() {
        for tool in ["sobre_knowledge", "sobre_session"] {
            assert!(!skip_terse(tool), "{tool} is condensable");
        }
        // Content the agent acts on stays as the tool returned it: file text,
        // the original sobre_expand returns, edit evidence, search hits,
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
            // condensed inside the tool, before it keeps the full output
            "sobre_shell",
        ] {
            assert!(skip_terse(tool), "{tool} must stay as the tool returned it");
        }
    }

    /// Output of a command the user listed in `passthrough_commands`
    /// is never condensed.
    #[test]
    fn passthrough_commands_are_never_condensed() {
        let config = Config {
            compression_level: 0.7,
            passthrough_commands: vec!["mytool".to_string()],
            ..Config::default()
        };
        let text = "the execution of the command completed and the command returned \
                    the result of the execution of the command\n"
            .repeat(20);
        assert_eq!(
            condense_shell_output("mytool --report", text.clone(), &config),
            text
        );
    }

    #[test]
    fn compress_terse_keeps_reads_byte_identical() {
        // Content laced with terse triggers (dictionary words, blank lines and a
        // UTF-8 BOM) that the prose pipeline would otherwise mangle. A read must
        // come back byte-for-byte even with the densest compression level.
        let content = "\u{feff}fn run() {\n\n    // command execution pipeline\n    let command = execution();\n    return command;\n}\n";
        let cfg = Config {
            compression_level: 0.7,
            ..Config::default()
        };
        let out = compress_terse(content.to_string(), "sobre_read", &cfg);
        assert_eq!(out, content, "sobre_read output must be byte-identical");
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
        let out = condense_shell_output("my-report", text.clone(), &cfg);
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
