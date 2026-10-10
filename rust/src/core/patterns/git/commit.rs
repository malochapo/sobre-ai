use super::{
    clone_objects_re, compact_lines, extract_change_stats, is_diff_or_stat_line, stash_re,
};

pub(super) fn compress_add(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "ok".to_string();
    }
    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() <= 3 {
        return trimmed.to_string();
    }
    // Only `add '<path>'` verbose lines are files; anything else in the output
    // (line-ending warnings, a filter's chatter) is not counted as one: a
    // summary states only numbers it parsed from the output.
    let added = lines
        .iter()
        .filter(|l| l.trim_start().starts_with("add '"))
        .count();
    if added > 0 {
        format!("ok (+{added} files)")
    } else {
        format!("ok ({} lines)", lines.len())
    }
}

pub(super) fn compress_fetch(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "ok".to_string();
    }

    let mut new_branches = Vec::new();
    for line in trimmed.lines() {
        let l = line.trim();
        if (l.contains("[new branch]") || l.contains("[new tag]"))
            && let Some(name) = l.split("->").last()
        {
            new_branches.push(name.trim().to_string());
        }
    }

    if new_branches.is_empty() {
        return "ok (fetched)".to_string();
    }
    format!("ok (new: {})", new_branches.join(", "))
}

pub(super) fn compress_clone(output: &str) -> String {
    let mut objects = 0u32;
    for line in output.lines() {
        if let Some(caps) = clone_objects_re().captures(line) {
            objects = caps[1].parse().unwrap_or(0);
        }
    }

    let into = output
        .lines()
        .find(|l| l.contains("Cloning into"))
        .and_then(|l| l.split('\'').nth(1))
        .unwrap_or("repo");

    if objects > 0 {
        format!("cloned '{into}' ({objects} objects)")
    } else {
        format!("cloned '{into}'")
    }
}

pub(super) fn compress_branch(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "ok".to_string();
    }

    let branches: Vec<String> = trimmed
        .lines()
        .filter_map(|line| {
            let l = line.trim();
            if l.is_empty() {
                return None;
            }
            if let Some(rest) = l.strip_prefix('*') {
                Some(format!("*{}", rest.trim()))
            } else {
                Some(l.to_string())
            }
        })
        .collect();

    branches.join(", ")
}

pub(super) fn compress_checkout(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "ok".to_string();
    }

    for line in trimmed.lines() {
        let l = line.trim();
        if l.starts_with("Switched to") || l.starts_with("Already on") {
            let branch = l.split('\'').nth(1).unwrap_or(l);
            return format!("→ {branch}");
        }
    }

    compact_lines(trimmed, 3)
}

pub(super) fn compress_stash(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "ok".to_string();
    }

    let line_count = trimmed.lines().count();
    if line_count <= 5 {
        return trimmed.to_string();
    }

    let stashes: Vec<String> = trimmed
        .lines()
        .filter_map(|line| {
            stash_re()
                .captures(line)
                .map(|caps| format!("@{}: {}", &caps[1], &caps[2]))
        })
        .collect();

    if stashes.is_empty() {
        return compact_lines(trimmed, 30);
    }
    stashes.join("\n")
}

pub(super) fn compress_blame(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= 100 {
        return output.to_string();
    }

    let unique_authors: std::collections::HashSet<&str> = lines
        .iter()
        .filter_map(|l| l.split('(').nth(1)?.split_whitespace().next())
        .collect();

    let mut result = format!("{} lines, {} authors:\n", lines.len(), unique_authors.len());
    let mut current_author = String::new();
    let mut range_start = 0usize;
    let mut range_end = 0usize;

    for (i, line) in lines.iter().enumerate() {
        let author = line
            .split('(')
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .unwrap_or("?");
        if author == current_author {
            range_end = i + 1;
        } else {
            if !current_author.is_empty() {
                result.push_str(&format!(
                    "  L{}-{}: {current_author}\n",
                    range_start + 1,
                    range_end
                ));
            }
            current_author = author.to_string();
            range_start = i;
            range_end = i + 1;
        }
    }
    if !current_author.is_empty() {
        result.push_str(&format!(
            "  L{}-{}: {current_author}\n",
            range_start + 1,
            range_end
        ));
    }
    result.trim_end().to_string()
}

pub(super) fn compress_show(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    let mut hash = String::new();
    let mut message = String::new();
    let mut diff_start: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("commit ") && hash.is_empty() {
            hash = trimmed[7..14.min(trimmed.len())].to_string();
        } else if trimmed.starts_with("diff --git") && diff_start.is_none() {
            diff_start = Some(i);
        } else if !trimmed.is_empty()
            && !trimmed.starts_with("Author:")
            && !trimmed.starts_with("Date:")
            && !trimmed.starts_with("Merge:")
            && !is_diff_or_stat_line(trimmed)
            && message.is_empty()
            && !hash.is_empty()
            && diff_start.is_none()
        {
            message = trimmed.to_string();
        }
    }

    if hash.is_empty() {
        return compact_lines(output.trim(), 10);
    }

    let mut result = format!("{hash} {message}");

    if let Some(start) = diff_start {
        let diff_portion: String = lines[start..].join("\n");
        let compressed_diff = super::diff::compress_diff_keep_hunks(&diff_portion);
        result.push('\n');
        result.push_str(&compressed_diff);
    } else {
        let stats = extract_change_stats(output);
        if !stats.is_empty() {
            result.push_str(&format!(" [{stats}]"));
        }
    }

    result
}

pub(super) fn compress_submodule(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "ok".to_string();
    }

    let mut modules = Vec::new();
    for line in trimmed.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let status_char = if line.starts_with('+') {
                "~"
            } else if line.starts_with('-') {
                "!"
            } else {
                ""
            };
            modules.push(format!("{status_char}{}", parts.last().unwrap_or(&"?")));
        }
    }

    if modules.is_empty() {
        return compact_lines(trimmed, 5);
    }
    format!("{} submodules: {}", modules.len(), modules.join(", "))
}

pub(super) fn compress_worktree(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "ok".to_string();
    }

    let mut worktrees = Vec::new();
    let mut current_path = String::new();
    let mut current_branch = String::new();

    for line in trimmed.lines() {
        let l = line.trim();
        if !l.contains(' ') && !l.is_empty() && current_path.is_empty() {
            current_path = l.to_string();
        } else if l.starts_with("HEAD ") {
            // skip
        } else if l.starts_with("branch ") || l.contains("detached") || l.contains("bare") {
            current_branch = l.to_string();
        } else if l.is_empty() && !current_path.is_empty() {
            let short_path = current_path.rsplit('/').next().unwrap_or(&current_path);
            worktrees.push(format!("{short_path} [{current_branch}]"));
            current_path.clear();
            current_branch.clear();
        }
    }
    if !current_path.is_empty() {
        let short_path = current_path.rsplit('/').next().unwrap_or(&current_path);
        worktrees.push(format!("{short_path} [{current_branch}]"));
    }

    if worktrees.is_empty() {
        return compact_lines(trimmed, 5);
    }
    format!("{} worktrees:\n{}", worktrees.len(), worktrees.join("\n"))
}

pub(super) fn compress_bisect(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return "ok".to_string();
    }

    for line in trimmed.lines() {
        let l = line.trim();
        if l.contains("is the first bad commit") {
            let hash = l.split_whitespace().next().unwrap_or("?");
            let short = &hash[..7.min(hash.len())];
            return format!("found: {short} is first bad commit");
        }
        if l.starts_with("Bisecting:") {
            return l.to_string();
        }
    }

    compact_lines(trimmed, 5)
}
