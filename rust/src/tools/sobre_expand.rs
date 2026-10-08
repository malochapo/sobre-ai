//! `sobre_expand`: return the full output that compression elided.
//!
//! Every compressed output whose content was cut is saved verbatim (secrets
//! redacted) in the `{state}/tee/` store and its footer names the id. This
//! tool reads that file back: whole, as a line range, or as the lines matching
//! a substring. Re-running the command is not equivalent — side effects,
//! non-deterministic output and long runtimes make the saved copy the only
//! faithful one.

pub fn handle(args: &serde_json::Value) -> String {
    let Some(id) = args.get("id").and_then(|v| v.as_str()) else {
        return "ERROR: 'id' is required — use the id named in a compressed output's footer."
            .to_string();
    };
    let Some(path) = crate::proxy::ccr::resolve_tee(id) else {
        return format!(
            "ERROR: no saved output for '{id}' (saved outputs expire after 24h). Re-run the command with raw=true."
        );
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return format!("ERROR: saved output '{id}' could not be read.");
    };
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(id);

    if let Some(pattern) = args.get("search").and_then(|v| v.as_str()) {
        return format!(
            "[{name}] lines matching \"{pattern}\":\n{}",
            search_lines(&content, pattern)
        );
    }
    let line_arg = |key| {
        args.get(key)
            .and_then(serde_json::Value::as_u64)
            .map(|v| v as usize)
    };
    let total = content.lines().count();
    match (line_arg("start_line"), line_arg("end_line")) {
        (None, None) => format!("[{name}] ({total} lines):\n{content}"),
        (start, end) => {
            let (s, e) = (start.unwrap_or(1), end.unwrap_or(total));
            format!(
                "[{name}] lines {s}-{e} of {total}:\n{}",
                range_lines(&content, s, e)
            )
        }
    }
}

/// 1-indexed inclusive line range, clamped to the available lines.
fn range_lines(s: &str, start: usize, end: usize) -> String {
    let v: Vec<&str> = s.lines().collect();
    let a = start.saturating_sub(1).min(v.len());
    let b = end.min(v.len());
    if a >= b {
        return String::new();
    }
    v[a..b].join("\n")
}

fn search_lines(s: &str, pattern: &str) -> String {
    let hits: Vec<String> = s
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains(pattern))
        .map(|(i, l)| format!("{}: {}", i + 1, l))
        .collect();
    if hits.is_empty() {
        format!("(no lines match \"{pattern}\")")
    } else {
        hits.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_is_one_indexed_inclusive_and_clamped() {
        let s = "a\nb\nc\nd";
        assert_eq!(range_lines(s, 2, 3), "b\nc");
        assert_eq!(range_lines(s, 3, 99), "c\nd");
        assert_eq!(range_lines(s, 9, 12), "");
    }

    #[test]
    fn search_numbers_matching_lines() {
        assert_eq!(
            search_lines("ok\nERROR x\nok\nERROR y", "ERROR"),
            "2: ERROR x\n4: ERROR y"
        );
        assert!(search_lines("ok", "ERROR").starts_with("(no lines match"));
    }

    #[test]
    fn missing_or_unknown_id_is_an_error() {
        assert!(handle(&serde_json::json!({})).starts_with("ERROR"));
        assert!(handle(&serde_json::json!({ "id": "../../etc/passwd" })).starts_with("ERROR"));
    }
}
