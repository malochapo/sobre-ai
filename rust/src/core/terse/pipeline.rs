//! The terse pipeline: the one entry point for generic compression of tool
//! and command output, used by the MCP tool results and by the shell
//! compression engine alike.

use super::TerseResult;
use super::counter;
use super::engine;
use super::residual;
use crate::core::config::CompressionLevel;

const MAX_TERSE_INPUT_BYTES: usize = 64_000;

/// Runs the pipeline on tool or command output; `pattern_compressed` is a
/// pattern compressor's output for it, when one ran. Inputs over 64 KB are
/// returned unchanged.
pub fn compress(
    input: &str,
    level: &CompressionLevel,
    pattern_compressed: Option<&str>,
) -> TerseResult {
    if !level.is_active() || input.is_empty() || input.len() > MAX_TERSE_INPUT_BYTES {
        return TerseResult::passthrough(input.to_string());
    }

    // Structured-data fast path: tool/command output that is a single JSON
    // document (API responses, `gh api`, `cargo --message-format=json`)
    // compacts losslessly far better than the token engine.
    // sobre_read outputs carry a header line, so they never match here —
    // only genuine pure-JSON payloads do, keeping file reads byte-exact.
    if let Some(compact) = crate::core::structured_compact::compact_json(input) {
        let before = counter::count(input);
        let after = counter::count(&compact);
        if after < before {
            return TerseResult {
                output: compact,
                savings_pct: counter::savings_pct(before, after),
                quality_passed: true,
            };
        }
    }

    match pattern_compressed {
        Some(after_patterns) => compress_with_patterns(input, after_patterns, level),
        // Fenced code blocks (``` / ~~~) carry verbatim source — diffs, file
        // excerpts, edit evidence. The dictionary/line-score layers
        // would corrupt them, so prose is compressed around the fences and
        // fence content stays byte-exact.
        None => match compress_fence_aware(input, level) {
            Some(fenced) => fenced,
            None => compress_direct(input, level),
        },
    }
}

/// Returns true when `line` opens/closes a fenced code block (CommonMark:
/// at most 3 leading spaces, then ≥ 3 backticks or tildes).
fn is_fence_delimiter(line: &str) -> Option<char> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    ['`', '~']
        .into_iter()
        .find(|&ch| trimmed.chars().take_while(|&c| c == ch).count() >= 3)
}

/// Compress prose around fenced code blocks, keeping fence content (and the
/// fence delimiter lines) byte-exact. Returns `None` when the input contains
/// no fence — callers fall back to the plain path. An unterminated fence
/// protects everything from its opener to the end of input (conservative:
/// never corrupt what might be code).
fn compress_fence_aware(input: &str, level: &CompressionLevel) -> Option<TerseResult> {
    if !input.contains("```") && !input.contains("~~~") {
        return None;
    }

    // Split into alternating prose/fence segments, preserving line endings.
    let mut segments: Vec<(bool, String)> = Vec::new(); // (is_fence, text)
    let mut current = String::new();
    let mut fence_char: Option<char> = None;

    for line in input.split_inclusive('\n') {
        let stripped = line.strip_suffix('\n').unwrap_or(line);
        match fence_char {
            None => {
                if let Some(ch) = is_fence_delimiter(stripped) {
                    if !current.is_empty() {
                        segments.push((false, std::mem::take(&mut current)));
                    }
                    fence_char = Some(ch);
                    current.push_str(line);
                } else {
                    current.push_str(line);
                }
            }
            Some(ch) => {
                current.push_str(line);
                if is_fence_delimiter(stripped) == Some(ch) {
                    segments.push((true, std::mem::take(&mut current)));
                    fence_char = None;
                }
            }
        }
    }
    if !current.is_empty() {
        // Unterminated fence stays protected; trailing prose is compressible.
        segments.push((fence_char.is_some(), current));
    }

    if !segments.iter().any(|(is_fence, _)| *is_fence) {
        return None;
    }

    let mut output = String::with_capacity(input.len());
    for (is_fence, text) in &segments {
        if *is_fence {
            output.push_str(text);
        } else {
            let res = engine::compress(text, level);
            if res.quality_passed && res.tokens_after < res.tokens_before {
                output.push_str(&res.output);
                // Keep prose/fence boundaries on separate lines.
                if !output.ends_with('\n') && text.ends_with('\n') {
                    output.push('\n');
                }
            } else {
                output.push_str(text);
            }
        }
    }

    let tokens_before = counter::count(input);
    let tokens_after = counter::count(&output);
    Some(TerseResult {
        output,
        savings_pct: counter::savings_pct(tokens_before, tokens_after),
        quality_passed: true,
    })
}

fn compress_direct(input: &str, level: &CompressionLevel) -> TerseResult {
    let result = engine::compress(input, level);

    if !result.quality_passed {
        return TerseResult::passthrough(input.to_string());
    }

    TerseResult {
        output: result.output,
        savings_pct: counter::savings_pct(result.tokens_before, result.tokens_after),
        quality_passed: result.quality_passed,
    }
}

fn compress_with_patterns(
    original: &str,
    after_patterns: &str,
    level: &CompressionLevel,
) -> TerseResult {
    let res = residual::compress_residual(original, after_patterns, level);

    TerseResult {
        output: res.output,
        savings_pct: res.total_savings_pct,
        quality_passed: res.quality_passed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_level_passthrough() {
        let result = compress("hello world", &CompressionLevel::Off, None);
        assert_eq!(result.output, "hello world");
        assert_eq!(result.savings_pct, 0.0);
    }

    #[test]
    fn direct_compression_works() {
        let text = "line one\n\n\nline two\n\n\nline three\n\n\n";
        let result = compress(text, &CompressionLevel::Standard, None);
        assert!(
            !result.output.contains("\n\n\n"),
            "should remove empty lines"
        );
    }

    #[test]
    fn empty_input_passthrough() {
        let result = compress("", &CompressionLevel::Max, None);
        assert_eq!(result.output, "");
    }

    /// Fenced code (edit evidence diffs) must survive byte-exact —
    /// no dictionary abbreviation, no blank-line stripping, no line drops.
    #[test]
    fn fenced_code_is_byte_exact() {
        let fence = "```diff\n--- a.py\n+++ a.py\n         \"\"\"Опубликовать задачу.\n\n-        return 0\n+        return 1\n\n             flag=True,\n             flag=True,\n```\n";
        let input = format!(
            "edit applied successfully to the file\n\n{fence}\nthe operation finished and the file was written\n"
        );
        let result = compress(&input, &CompressionLevel::Max, None);
        assert!(
            result.output.contains(fence),
            "fence must be untouched, got:\n{}",
            result.output
        );
        assert!(
            result.output.contains("return 0"),
            "no abbreviation inside fence"
        );
    }

    #[test]
    fn unterminated_fence_is_protected() {
        let input = "prose before\n```python\nreturn 0\n\nreturn 1\n";
        let result = compress(input, &CompressionLevel::Max, None);
        assert!(
            result.output.contains("```python\nreturn 0\n\nreturn 1\n"),
            "everything after an unterminated opener stays verbatim, got:\n{}",
            result.output
        );
    }

    #[test]
    fn no_fence_returns_none_from_fence_path() {
        assert!(compress_fence_aware("plain text, no code", &CompressionLevel::Max).is_none());
        assert!(compress_fence_aware("inline `code` only", &CompressionLevel::Max).is_none());
    }
}
