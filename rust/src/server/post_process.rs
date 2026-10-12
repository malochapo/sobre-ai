//! Post-processing stages: the terse condensing of shell output and the final
//! token count of `SobreServer::call_tool_guarded`. Each is a small function
//! with no side effect, so the guarded path stays an orchestrator.
//!
//! Only shell output is condensed, inside `sobre_shell`
//! ([`condense_shell_output`]), before it decides whether to keep the full
//! output, so a condensed answer always carries its `sobre_expand` id. Every
//! other tool returns content the agent acts on — file text, search hits,
//! signatures, diffs, paths, the original `sobre_expand` returns — where the
//! dictionary and line-dropping layers would change what the agent reads.

use crate::core::config::{CompressionLevel, Config};

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

/// The terse stage at the configured compression, labelled so the agent knows
/// it is not reading the command's raw text; `text` itself when compression
/// is off, the result loses quality, or it saves under 3%.
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

/// Final sent-token count: the tool's own count when nothing was appended to
/// its text, a recount otherwise.
pub(super) fn final_token_count(
    result_text: &str,
    tool_text_len: usize,
    tool_tokens: u64,
) -> usize {
    if result_text.len() == tool_text_len {
        usize::try_from(tool_tokens).unwrap_or(usize::MAX)
    } else {
        crate::core::tokens::count_tokens(result_text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
