use similar::{ChangeTag, TextDiff};

/// Removes ANSI escape codes from a string, returning clean text.
pub fn strip_ansi(s: &str) -> String {
    if !s.contains('\x1b') {
        return s.to_string();
    }
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
            continue;
        }
        if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
            continue;
        }
        result.push(c);
    }
    result
}

/// Strips comments, blank lines, and normalizes indentation for maximum token savings.
pub fn aggressive_compress(content: &str, ext: Option<&str>) -> String {
    // Structured data (JSON/JSONL) carries no comments and barely compresses via
    // the line-based path below (~0% measured). Strip insignificant whitespace
    // losslessly instead — key order, numbers, and string contents are preserved.
    // Markdown-family files opt into the lossy structural compactor by
    // extension alone; a plain `.txt` must additionally show a real ATX
    // heading — hyphen lists or wrapped prose are not enough to treat a text
    // file as a structured document (#655).
    let markdown_family = matches!(ext, Some("md" | "markdown" | "mdown"));
    let structured_txt =
        matches!(ext, Some("txt")) && crate::core::markdown_compact::has_markdown_headings(content);
    if (markdown_family || structured_txt)
        && let Some(compacted) = crate::core::markdown_compact::compact_markdown(content)
    {
        return compacted;
    }
    if let Some(compacted) = crate::core::structured_compact::compact_structured(content, ext) {
        return compacted;
    }

    // Tabular data (CSV/TSV, #982): a redundant table with constant columns
    // compacts losslessly through the columnar crusher far better than the
    // line-based path below. Fires only when it at least halves the input;
    // otherwise fall through. The exact bytes stay recoverable via a full re-read.
    if let Some(delim) = tabular_delimiter(ext)
        && let Some(crushed) = crate::core::tabular_crush::crush_text_if_beneficial(content, delim)
    {
        return crushed;
    }

    // YAML (#985): a verbose document compacts losslessly to compact JSON through
    // the shared crusher — formatting dropped, redundant `items`/`list` arrays
    // factored — far better than the line-based path below. Fires only when it
    // clears the reduction gate; the exact bytes stay recoverable via a full
    // re-read.
    if is_yaml_ext(ext)
        && let Some(crushed) = crate::core::yaml_crush::crush_text_if_beneficial(content)
    {
        return crushed;
    }

    let mut result: Vec<String> = Vec::new();
    let is_python = matches!(ext, Some("py"));
    let is_html = matches!(ext, Some("html" | "htm" | "xml" | "svg"));
    let is_sql = matches!(ext, Some("sql"));
    let is_shell = matches!(ext, Some("sh" | "bash" | "zsh" | "fish"));

    let mut in_block_comment = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        if in_block_comment {
            if trimmed.contains("*/") || (is_html && trimmed.contains("-->")) {
                in_block_comment = false;
            }
            continue;
        }

        if trimmed.starts_with("/*") || (is_html && trimmed.starts_with("<!--")) {
            if !(trimmed.contains("*/") || trimmed.contains("-->")) {
                in_block_comment = true;
            }
            continue;
        }

        if trimmed.starts_with("//") && !trimmed.starts_with("///") {
            continue;
        }
        if trimmed.starts_with('*') || trimmed.starts_with("*/") {
            continue;
        }
        if is_python && trimmed.starts_with('#') {
            continue;
        }
        if is_sql && trimmed.starts_with("--") {
            continue;
        }
        if is_shell && trimmed.starts_with('#') && !trimmed.starts_with("#!") {
            continue;
        }
        if !is_python && trimmed.starts_with('#') && trimmed.contains('[') {
            continue;
        }

        if trimmed == "}" || trimmed == "};" || trimmed == ");" || trimmed == "});" {
            if let Some(last) = result.last() {
                let last_trimmed = last.trim();
                if matches!(last_trimmed, "}" | "};" | ");" | "});") {
                    if let Some(last_mut) = result.last_mut() {
                        last_mut.push_str(trimmed);
                    }
                    continue;
                }
            }
            result.push(trimmed.to_string());
            continue;
        }

        let normalized = normalize_indentation(line);
        result.push(normalized);
    }

    result.join("\n")
}

/// Lightweight post-processing cleanup: collapses consecutive closing braces,
/// removes whitespace-only lines, and limits consecutive blank lines to 1.
pub fn lightweight_cleanup(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();

    let mut result: Vec<String> = Vec::new();
    let mut blank_count = 0u32;
    let mut brace_run: Vec<&str> = Vec::new();

    let flush_brace_run = |run: &mut Vec<&str>, out: &mut Vec<String>| {
        if total <= 200 || run.len() <= 5 {
            for l in run.iter() {
                out.push(l.to_string());
            }
        } else {
            out.push(run[0].to_string());
            out.push(run[1].to_string());
            out.push(format!("[{} brace-only lines collapsed]", run.len() - 2));
        }
        run.clear();
    };

    for line in &lines {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            flush_brace_run(&mut brace_run, &mut result);
            blank_count += 1;
            if blank_count <= 1 {
                result.push(String::new());
            }
            continue;
        }
        blank_count = 0;

        if matches!(trimmed, "}" | "};" | ");" | "});" | ")") {
            brace_run.push(trimmed);
            continue;
        }

        flush_brace_run(&mut brace_run, &mut result);
        result.push(line.to_string());
    }
    flush_brace_run(&mut brace_run, &mut result);

    result.join("\n")
}

/// Safeguard: prevents compression from inflating output or destroying small outputs.
/// For small outputs (<2000 tokens), rejects extreme compression (>95% reduction)
/// that likely lost important content. For large outputs, trusts the pattern.
pub fn safeguard_ratio(original: &str, compressed: &str) -> String {
    let orig_tokens = super::tokens::count_tokens(original);
    let comp_tokens = super::tokens::count_tokens(compressed);

    if orig_tokens == 0 {
        return compressed.to_string();
    }

    if comp_tokens > orig_tokens {
        return original.to_string();
    }

    let ratio = comp_tokens as f64 / orig_tokens as f64;
    if ratio < 0.05 && orig_tokens < 2000 {
        original.to_string()
    } else {
        compressed.to_string()
    }
}

/// Delimiter for a delimited-table extension, or `None` for non-tabular files.
pub(crate) fn tabular_delimiter(ext: Option<&str>) -> Option<char> {
    match ext {
        Some("csv") => Some(','),
        Some("tsv" | "tab") => Some('\t'),
        _ => None,
    }
}

/// True for a YAML file extension (`.yaml` / `.yml`).
pub(crate) fn is_yaml_ext(ext: Option<&str>) -> bool {
    matches!(ext, Some("yaml" | "yml"))
}

fn normalize_indentation(line: &str) -> String {
    let content = line.trim_start();
    let leading = line.len() - content.len();
    let has_tabs = line.starts_with('\t');
    let reduced = if has_tabs { leading } else { leading / 2 };
    format!("{}{}", " ".repeat(reduced), content)
}

/// Produces a compact unified diff between old and new content with line numbers.
pub fn diff_content(old_content: &str, new_content: &str) -> String {
    if old_content == new_content {
        return "(no changes)".to_string();
    }

    let diff = TextDiff::from_lines(old_content, new_content);
    let mut changes = Vec::new();
    let mut additions = 0usize;
    let mut deletions = 0usize;

    for change in diff.iter_all_changes() {
        let line_no = change.new_index().or(change.old_index()).map(|i| i + 1);
        let text = change.value().trim_end_matches('\n');
        match change.tag() {
            ChangeTag::Insert => {
                additions += 1;
                if let Some(n) = line_no {
                    changes.push(format!("+{n}: {text}"));
                }
            }
            ChangeTag::Delete => {
                deletions += 1;
                if let Some(n) = line_no {
                    changes.push(format!("-{n}: {text}"));
                }
            }
            ChangeTag::Equal => {}
        }
    }

    if changes.is_empty() {
        return "(no changes)".to_string();
    }

    changes.push(format!("\ndiff +{additions}/-{deletions} lines"));
    changes.join("\n")
}

/// Compresses content using the active task intent to preserve task-relevant sections.
pub fn task_aware_compress(
    content: &str,
    ext: Option<&str>,
    intent: &super::intent_engine::StructuredIntent,
) -> String {
    use super::intent_engine::{IntentScope, TaskType};

    let budget_ratio = match intent.scope {
        IntentScope::SingleFile => 0.7,
        IntentScope::MultiFile => 0.5,
        IntentScope::CrossModule => 0.35,
        IntentScope::ProjectWide => 0.25,
    };

    match intent.task_type {
        TaskType::FixBug | TaskType::Debug => {
            let filtered = super::task_relevance::information_bottleneck_filter_typed(
                content,
                &intent.keywords,
                budget_ratio,
                Some(intent.task_type),
                &[],
            );
            safeguard_ratio(content, &filtered)
        }
        TaskType::Refactor | TaskType::Review => {
            let cleaned = lightweight_cleanup(content);
            let filtered = super::task_relevance::information_bottleneck_filter_typed(
                &cleaned,
                &intent.keywords,
                budget_ratio.max(0.5),
                Some(intent.task_type),
                &[],
            );
            safeguard_ratio(content, &filtered)
        }
        TaskType::Generate | TaskType::Test => {
            let compressed = aggressive_compress(content, ext);
            safeguard_ratio(content, &compressed)
        }
        TaskType::Explore | TaskType::Config | TaskType::Deploy => {
            let cleaned = lightweight_cleanup(content);
            safeguard_ratio(content, &cleaned)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_insertion() {
        let old = "line1\nline2\nline3";
        let new = "line1\nline2\nnew_line\nline3";
        let result = diff_content(old, new);
        assert!(result.contains('+'), "should show additions");
        assert!(result.contains("new_line"));
    }

    #[test]
    fn test_diff_deletion() {
        let old = "line1\nline2\nline3";
        let new = "line1\nline3";
        let result = diff_content(old, new);
        assert!(result.contains('-'), "should show deletions");
        assert!(result.contains("line2"));
    }

    #[test]
    fn test_diff_no_changes() {
        let content = "same\ncontent";
        assert_eq!(diff_content(content, content), "(no changes)");
    }

    #[test]
    fn test_lightweight_cleanup_collapses_braces() {
        let mut lines: Vec<String> = (0..210).map(|i| format!("line {i}")).collect();
        lines.extend(
            ["}", "}", "}", "}", "}", "}", "}", "}"]
                .iter()
                .map(std::string::ToString::to_string),
        );
        lines.push("fn next() {}".to_string());
        let input = lines.join("\n");
        let result = lightweight_cleanup(&input);
        assert!(
            result.contains("[6 brace-only lines collapsed]"),
            "should collapse long brace runs in large files"
        );
        assert!(result.contains("fn next()"));
    }

    #[test]
    fn test_lightweight_cleanup_blank_lines() {
        let input = "line1\n\n\n\n\nline2";
        let result = lightweight_cleanup(input);
        let blank_runs = result.split("line1").nth(1).unwrap();
        let blanks = blank_runs.matches('\n').count();
        assert!(blanks <= 2, "should collapse multiple blank lines");
    }

    #[test]
    fn test_safeguard_ratio_prevents_over_compression_on_small_output() {
        let original = "a ".repeat(100); // ~100 tokens, < 2000
        let too_compressed = "a";
        let result = safeguard_ratio(&original, too_compressed);
        assert_eq!(
            result, original,
            "should return original when ratio < 0.05 and output is small"
        );
    }

    #[test]
    fn test_safeguard_ratio_allows_strong_compression_on_large_output() {
        let original = "line content here\n".repeat(1000); // ~4000 tokens, > 2000
        let compressed = "summary: 1000 lines";
        let result = safeguard_ratio(&original, compressed);
        assert_eq!(
            result, compressed,
            "should allow strong compression for large outputs"
        );
    }

    #[test]
    fn test_aggressive_strips_comments() {
        let code = "fn main() {\n    // a comment\n    let x = 1;\n}";
        let result = aggressive_compress(code, Some("rs"));
        assert!(!result.contains("// a comment"));
        assert!(result.contains("let x = 1"));
    }

    #[test]
    fn test_aggressive_python_comments() {
        let code = "def main():\n    # comment\n    x = 1";
        let result = aggressive_compress(code, Some("py"));
        assert!(!result.contains("# comment"));
        assert!(result.contains("x = 1"));
    }

    #[test]
    fn test_aggressive_preserves_doc_comments() {
        let code = "/// Doc comment\nfn main() {}";
        let result = aggressive_compress(code, Some("rs"));
        assert!(result.contains("/// Doc comment"));
    }

    #[test]
    fn test_aggressive_block_comment() {
        let code = "/* start\n * middle\n */ end\nfn main() {}";
        let result = aggressive_compress(code, Some("rs"));
        assert!(!result.contains("start"));
        assert!(!result.contains("middle"));
        assert!(result.contains("fn main()"));
    }

    #[test]
    fn test_strip_ansi_removes_escape_codes() {
        let input = "\x1b[31mERROR\x1b[0m: something failed";
        let result = strip_ansi(input);
        assert_eq!(result, "ERROR: something failed");
        assert!(!result.contains('\x1b'));
    }

    #[test]
    fn test_strip_ansi_passthrough_clean_text() {
        let input = "clean text without escapes";
        let result = strip_ansi(input);
        assert_eq!(result, input);
    }
}
