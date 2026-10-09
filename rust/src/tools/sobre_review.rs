//! `sobre_review`: what a change to a file touches, for a reviewer. The files
//! that depend on it, the call sites of its functions in other files, and its
//! test files, all read from the code graph.

use crate::core::tokens::count_tokens;
use std::path::Path;

/// Affected files listed before the rest is summarized as a count.
const SHOWN_FILES: usize = 30;
/// Functions of the file whose outside call sites are listed.
const SHOWN_FUNCTIONS: usize = 15;

/// Dispatches code review actions (review, diff-review, checklist).
pub fn handle(action: &str, path: Option<&str>, root: &str, depth: Option<usize>) -> String {
    match action {
        "review" => handle_review(path, root, depth.unwrap_or(3)),
        "diff-review" => handle_diff_review(path, root, depth.unwrap_or(2)),
        "checklist" => handle_checklist(path, root, depth.unwrap_or(3)),
        _ => "Unknown action. Use: review, diff-review, checklist".to_string(),
    }
}

fn handle_review(path: Option<&str>, root: &str, depth: usize) -> String {
    let Some(target) = path else {
        return "ERROR: path is required for 'review' action".to_string();
    };
    let output = review_sections(target, root, depth).join("\n");
    let tok = count_tokens(&output);
    format!("{output}\n\n[{tok} tok]")
}

fn review_sections(target: &str, root: &str, depth: usize) -> Vec<String> {
    let rel = crate::core::index_paths::graph_relative_key(target, root);
    let mut sections = vec![format!("## Review: {rel}\n")];

    let affected = crate::tools::sobre_graph_impact::affected_files(target, root, depth);
    sections.push(impact_section(target, affected.as_deref(), depth));
    if let Some(callers) = callers_section(&rel, root) {
        sections.push(callers);
    }

    let tests = related_tests(&rel, root, affected.as_deref());
    sections.push("### Related tests".to_string());
    if tests.is_empty() {
        sections.push("  (no test file is named after it or depends on it)".to_string());
    } else {
        sections.extend(tests.iter().map(|t| format!("  - {t}")));
    }
    sections
}

/// Test files named after `rel` (`foo_test.rs`, `foo.spec.ts`, …) and the
/// test files among its dependents (`tests/basic.rs` using the crate).
fn related_tests(rel: &str, root: &str, affected: Option<&[String]>) -> Vec<String> {
    let mut tests = find_related_tests(rel, root);
    tests.extend(
        affected
            .unwrap_or_default()
            .iter()
            .filter(|f| is_test_path(f))
            .cloned(),
    );
    tests.sort();
    tests.dedup();
    tests
}

/// A test file by its place (`tests/`, `test/`, `__tests__/`) or its name
/// (`*_test.*`, `*.test.*`, `*.spec.*`, `test_*`).
fn is_test_path(path: &str) -> bool {
    let path = path.replace('\\', "/");
    let in_test_dir = path
        .split('/')
        .rev()
        .skip(1)
        .any(|dir| matches!(dir, "tests" | "test" | "__tests__" | "spec"));
    let name = path.rsplit('/').next().unwrap_or(&path);
    in_test_dir
        || name.starts_with("test_")
        || name.contains("_test.")
        || name.contains("_tests.")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// The files that depend on `target`, up to `depth` hops.
fn impact_section(target: &str, affected: Option<&[String]>, depth: usize) -> String {
    match affected {
        None => format!(
            "### Impact\n  '{target}' is not an indexed file or a known type, so its \
             dependents are unknown."
        ),
        Some([]) => {
            format!("### Impact (depth {depth})\n  no file depends on it.")
        }
        Some(files) => {
            let mut out = format!("### Impact (depth {depth}): {} files", files.len());
            for f in files.iter().take(SHOWN_FILES) {
                out.push_str(&format!("\n  - {f}"));
            }
            if files.len() > SHOWN_FILES {
                out.push_str(&format!("\n  ... +{} more", files.len() - SHOWN_FILES));
            }
            out
        }
    }
}

/// Call sites in other files of the functions defined in `rel`. `None` when
/// the graph has no symbols for the file.
fn callers_section(rel: &str, root: &str) -> Option<String> {
    let graph = crate::core::graph_provider::open_or_build(root)?;
    let mut names: Vec<String> = graph
        .find_symbols("", Some(rel), None)
        .into_iter()
        .filter(|s| s.file == rel)
        .map(|s| s.name)
        .collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return None;
    }

    let calls = crate::tools::sobre_graph_calls::load(root)?;
    let mut lines = Vec::new();
    for name in &names {
        let mut sites: Vec<String> = calls
            .callers_of(name)
            .into_iter()
            .filter(|e| e.caller_file != rel)
            .map(|e| format!("{}:L{}", e.caller_file, e.caller_line))
            .collect();
        if sites.is_empty() {
            continue;
        }
        sites.sort();
        sites.dedup();
        let shown = sites.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
        let more = sites.len().saturating_sub(5);
        let more = if more > 0 {
            format!(" +{more} more")
        } else {
            String::new()
        };
        lines.push(format!("  {name} ← {shown}{more}"));
        if lines.len() == SHOWN_FUNCTIONS {
            break;
        }
    }
    Some(if lines.is_empty() {
        "### Callers from other files\n  none found (calls are matched by name)".to_string()
    } else {
        format!(
            "### Callers from other files (matched by name)\n{}",
            lines.join("\n")
        )
    })
}

/// Review every changed file: the files named in `diff_text` (git diff
/// output), or the uncommitted changes when no diff is given.
fn handle_diff_review(diff_input: Option<&str>, root: &str, depth: usize) -> String {
    let changed_files = match diff_input {
        Some(diff_text) => extract_changed_files(diff_text),
        None => crate::tools::sobre_graph_impact::git_changed_files(root),
    };
    if changed_files.is_empty() {
        return if diff_input.is_some() {
            "No changed files detected in diff input.".to_string()
        } else {
            "No uncommitted changes.".to_string()
        };
    }

    let mut sections = vec![format!(
        "## Diff Review: {} file(s) changed\n",
        changed_files.len()
    )];
    for file in &changed_files {
        sections.push("---".to_string());
        sections.extend(review_sections(file, root, depth));
    }

    let output = sections.join("\n");
    let tok = count_tokens(&output);
    format!("{output}\n\n[{tok} tok]")
}

fn handle_checklist(path: Option<&str>, root: &str, depth: usize) -> String {
    let Some(target) = path else {
        return "ERROR: path is required for 'checklist' action".to_string();
    };

    let rel = crate::core::index_paths::graph_relative_key(target, root);
    let mut questions = Vec::new();

    questions.push(format!(
        "- [ ] Are all public API changes in `{rel}` backward-compatible?"
    ));

    let affected_files = crate::tools::sobre_graph_impact::affected_files(target, root, depth);
    let affected = affected_files.as_ref().map_or(0, Vec::len);
    if affected > 0 {
        questions.push(format!(
            "- [ ] {affected} downstream file(s) affected — have they been reviewed?"
        ));
        questions.push(
            "- [ ] Do downstream consumers handle the changed interface correctly?".to_string(),
        );
    }

    let tests = related_tests(&rel, root, affected_files.as_deref());
    if tests.is_empty() {
        questions.push(format!(
            "- [ ] No tests found for `{rel}` — should tests be added?"
        ));
    } else {
        questions.push(format!(
            "- [ ] {} test file(s) found — do they still pass?",
            tests.len()
        ));
        for t in &tests {
            questions.push(format!("  - `{t}`"));
        }
    }

    questions.push("- [ ] Are error paths handled gracefully?".to_string());
    questions.push("- [ ] Is logging/telemetry appropriate (no sensitive data)?".to_string());

    let output = format!("## Review Checklist: {rel}\n\n{}", questions.join("\n"));
    let tok = count_tokens(&output);
    format!("{output}\n\n[{tok} tok]")
}

fn extract_changed_files(diff_text: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in diff_text.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            files.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("diff --git a/")
            && let Some(b_part) = rest.split(" b/").nth(1)
            && !files.contains(&b_part.to_string())
        {
            files.push(b_part.to_string());
        }
    }
    files.dedup();
    files
}

/// Finds test files related to the given source file by naming conventions.
pub fn find_related_tests(file_path: &str, root: &str) -> Vec<String> {
    let p = Path::new(file_path);
    let Some(stem) = p.file_stem().and_then(|s| s.to_str()) else {
        return vec![];
    };

    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");

    let patterns = vec![
        format!("{stem}_test.{ext}"),
        format!("{stem}.test.{ext}"),
        format!("{stem}.spec.{ext}"),
        format!("{stem}_spec.{ext}"),
        format!("test_{stem}.{ext}"),
        format!("{stem}_tests.{ext}"),
        format!("{stem}.test.ts"),
        format!("{stem}.test.tsx"),
        format!("{stem}.spec.ts"),
        format!("{stem}.spec.tsx"),
        format!("{stem}_test.rs"),
        format!("{stem}_test.py"),
        format!("test_{stem}.py"),
        format!("{stem}_test.go"),
    ];

    let root_path = Path::new(root);
    let mut found = Vec::new();

    fn walk_for_tests(
        dir: &Path,
        patterns: &[String],
        root: &Path,
        found: &mut Vec<String>,
        max_depth: usize,
    ) {
        if max_depth == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }

            if path.is_dir() {
                walk_for_tests(&path, patterns, root, found, max_depth - 1);
            } else if patterns.contains(&name) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                found.push(rel);
            }
        }
    }

    walk_for_tests(root_path, &patterns, root_path, &mut found, 8);
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_changed_files_from_diff() {
        let diff = "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,3 +1,4 @@\n+use foo;\n";
        let files = extract_changed_files(diff);
        assert_eq!(files, vec!["src/main.rs"]);
    }

    #[test]
    fn extract_changed_files_multiple() {
        let diff = "diff --git a/a.rs b/a.rs\n+++ b/a.rs\ndiff --git a/b.rs b/b.rs\n+++ b/b.rs\n";
        let files = extract_changed_files(diff);
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"a.rs".to_string()));
        assert!(files.contains(&"b.rs".to_string()));
    }

    #[test]
    fn find_related_tests_patterns() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("utils.ts");
        std::fs::write(&src, "export function foo() {}").unwrap();
        let test_file = dir.path().join("utils.test.ts");
        std::fs::write(&test_file, "test('foo', () => {})").unwrap();
        let spec_file = dir.path().join("utils.spec.ts");
        std::fs::write(&spec_file, "describe('foo', () => {})").unwrap();

        let found = find_related_tests("utils.ts", dir.path().to_str().unwrap());
        assert!(found.iter().any(|f| f.contains("utils.test.ts")));
        assert!(found.iter().any(|f| f.contains("utils.spec.ts")));
    }

    #[test]
    fn checklist_always_has_minimum_questions() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("foo.rs");
        std::fs::write(&f, "fn bar() {}").unwrap();

        let output = handle_checklist(Some("foo.rs"), dir.path().to_str().unwrap(), 2);
        let checkbox_count = output.matches("- [ ]").count();
        assert!(
            checkbox_count >= 3,
            "Expected at least 3 questions, got {checkbox_count}"
        );
    }
}
