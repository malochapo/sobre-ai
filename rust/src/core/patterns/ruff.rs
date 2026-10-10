macro_rules! static_regex {
    ($pattern:expr_2021) => {{
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| {
            regex::Regex::new($pattern).expect(concat!("BUG: invalid static regex: ", $pattern))
        })
    }};
}

fn ruff_line_re() -> &'static regex::Regex {
    static_regex!(r"^(.+?):(\d+):(\d+):\s+([A-Z]\d+)\s+(.+)$")
}
fn ruff_fixed_re() -> &'static regex::Regex {
    static_regex!(r"Found (\d+) errors?.*?(\d+) fixable")
}

/// Ruff's output, by subcommand. `ruff check` output never gets here: shell
/// compression keeps a findings-only linter's output verbatim. Findings do
/// reach `compress_check` from `ruff <path>`, the check command of ruff
/// before 0.5.
pub fn compress(command: &str, output: &str) -> Option<String> {
    if command.contains("format") || command.contains("fmt") {
        return Some(compress_format(output));
    }
    Some(compress_check(output))
}

/// Up to 30 findings, the output as it is; past that, every finding line
/// (without the code frames under them) and the count per rule.
fn compress_check(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() || trimmed.contains("All checks passed") {
        return "clean".to_string();
    }

    let mut by_rule: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut files: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut issue_lines: Vec<&str> = Vec::new();

    for line in trimmed.lines() {
        if let Some(caps) = ruff_line_re().captures(line) {
            let file = caps[1].to_string();
            let rule = caps[4].to_string();
            files.insert(file);
            *by_rule.entry(rule).or_insert(0) += 1;
            issue_lines.push(line);
        }
    }

    if by_rule.is_empty() {
        if let Some(caps) = ruff_fixed_re().captures(trimmed) {
            return format!("{} errors ({} fixable)", &caps[1], &caps[2]);
        }
        return super::compact_lines(trimmed, 10);
    }

    let total: u32 = by_rule.values().sum();

    if total <= 30 {
        return trimmed.to_string();
    }

    let mut rules: Vec<(String, u32)> = by_rule.into_iter().collect();
    rules.sort_by_key(|x| std::cmp::Reverse(x.1));

    let mut parts = Vec::new();
    parts.push(format!("{total} issues in {} files", files.len()));
    for line in &issue_lines {
        parts.push(format!("  {line}"));
    }
    parts.push(String::new());
    parts.push("by rule:".to_string());
    for (rule, count) in rules.iter().take(8) {
        parts.push(format!("  {rule}: {count}"));
    }
    if rules.len() > 8 {
        parts.push(format!("  ... +{} more rules", rules.len() - 8));
    }

    parts.join("\n")
}

fn compress_format(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "ok (formatted)".to_string();
    }

    let reformatted: Vec<&str> = trimmed
        .lines()
        .filter(|l| l.contains("reformatted") || l.contains("would reformat"))
        .collect();

    let unchanged: Vec<&str> = trimmed
        .lines()
        .filter(|l| l.contains("left unchanged") || l.contains("already formatted"))
        .collect();

    if !reformatted.is_empty() {
        return format!("{} files reformatted", reformatted.len());
    }
    if !unchanged.is_empty() {
        return format!("ok ({} files already formatted)", unchanged.len());
    }

    super::compact_lines(trimmed, 5)
}

#[cfg(test)]
mod tests {
    use super::compress;

    /// Past 30 findings the summary still lists every one.
    #[test]
    fn every_finding_is_listed() {
        let findings: Vec<String> = (0..45)
            .map(|i| {
                format!(
                    "src/mod{i}.py:{}:8: F401 [*] `os` imported but unused",
                    i + 1
                )
            })
            .collect();
        let output = format!("{}\nFound 45 errors.\n", findings.join("\n"));
        let result = compress("ruff src/", &output).unwrap();
        for finding in &findings {
            assert!(
                result.contains(finding.as_str()),
                "{finding} lost:\n{result}"
            );
        }
    }
}
