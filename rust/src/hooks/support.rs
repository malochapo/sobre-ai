pub(crate) fn install_named_json_server(
    name: &str,
    display_path: &str,
    config_path: &std::path::Path,
    root_key: &str,
    entry: serde_json::Value,
) {
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if config_path.exists() {
        let content = std::fs::read_to_string(config_path).unwrap_or_default();
        if content.contains("lean-ctx") {
            if !super::mcp_server_quiet_mode() {
                eprintln!("{name} MCP already configured at {display_path}");
            }
            return;
        }
        if update_named_json_server(config_path, &content, root_key, &entry) {
            print_named_json_server_success(name, display_path);
            return;
        }
    }

    if write_named_json_server_root(config_path, root_key, entry) {
        print_named_json_server_success(name, display_path);
    } else {
        tracing::error!("Failed to configure {name}");
    }
}

fn update_named_json_server(
    config_path: &std::path::Path,
    content: &str,
    root_key: &str,
    entry: &serde_json::Value,
) -> bool {
    let Ok(mut json) = crate::core::jsonc::parse_jsonc(content) else {
        return false;
    };
    let Some(obj) = json.as_object_mut() else {
        return false;
    };
    let servers = obj
        .entry(root_key.to_string())
        .or_insert_with(|| serde_json::json!({}));
    let Some(servers_obj) = servers.as_object_mut() else {
        return false;
    };
    servers_obj.insert("lean-ctx".to_string(), entry.clone());
    write_json_config(config_path, &json)
}

fn write_named_json_server_root(
    config_path: &std::path::Path,
    root_key: &str,
    entry: serde_json::Value,
) -> bool {
    let mut servers = serde_json::Map::new();
    servers.insert("lean-ctx".to_string(), entry);

    let mut root = serde_json::Map::new();
    root.insert(root_key.to_string(), serde_json::Value::Object(servers));

    write_json_config(config_path, &serde_json::Value::Object(root))
}

fn write_json_config(config_path: &std::path::Path, value: &serde_json::Value) -> bool {
    let Ok(json_str) = serde_json::to_string_pretty(value) else {
        return false;
    };
    std::fs::write(config_path, json_str).is_ok()
}

fn print_named_json_server_success(name: &str, display_path: &str) {
    if !super::mcp_server_quiet_mode() {
        eprintln!("  \x1b[32m✓\x1b[0m {name} MCP configured at {display_path}");
    }
}

pub(super) fn ensure_codex_hooks_enabled(config_content: &str) -> Option<String> {
    let newline = config_newline(config_content);
    let mut lines = config_lines(config_content);
    let layout = inspect_codex_hooks_layout(&lines);

    let mut changed =
        rewrite_existing_codex_hooks_assignment(&mut lines, layout.features_codex_index);
    changed |=
        clear_duplicate_feature_hooks_assignments(&mut lines, &layout.duplicate_feature_indices);

    if layout.features_codex_index.is_none() {
        insert_codex_hooks_assignment(&mut lines, layout.features_insert_index);
        changed = true;
    }

    changed.then(|| render_codex_hook_config(lines, newline))
}

struct CodexHooksLayout {
    features_insert_index: Option<usize>,
    features_codex_index: Option<usize>,
    duplicate_feature_indices: Vec<usize>,
}

fn config_newline(config_content: &str) -> &'static str {
    if config_content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn config_lines(config_content: &str) -> Vec<String> {
    config_content
        .lines()
        .map(std::string::ToString::to_string)
        .collect()
}

fn inspect_codex_hooks_layout(lines: &[String]) -> CodexHooksLayout {
    let mut features_codex_index = None;
    let mut features_insert_index = None;
    let mut duplicate_feature_indices = Vec::new();
    let mut current_section: Option<&str> = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(section) = parse_toml_section_name(trimmed) {
            current_section = Some(section);
            if section == "features" {
                features_insert_index = Some(idx + 1);
            }
            continue;
        }

        if current_section != Some("features") {
            continue;
        }
        features_insert_index = Some(idx + 1);

        if !is_codex_hooks_assignment(trimmed) {
            continue;
        }

        if features_codex_index.is_none() {
            features_codex_index = Some(idx);
        } else {
            duplicate_feature_indices.push(idx);
        }
    }

    CodexHooksLayout {
        features_insert_index,
        features_codex_index,
        duplicate_feature_indices,
    }
}

fn rewrite_existing_codex_hooks_assignment(lines: &mut [String], index: Option<usize>) -> bool {
    let Some(index) = index else {
        return false;
    };

    let replacement = rewrite_codex_hooks_line(&lines[index]);
    if lines[index] == replacement {
        return false;
    }

    lines[index] = replacement;
    true
}

fn clear_duplicate_feature_hooks_assignments(lines: &mut [String], indices: &[usize]) -> bool {
    let mut changed = false;
    for &idx in indices {
        if !lines[idx].is_empty() {
            lines[idx].clear();
            changed = true;
        }
    }
    changed
}

fn insert_codex_hooks_assignment(lines: &mut Vec<String>, insert_index: Option<usize>) {
    if let Some(insert_index) = insert_index {
        let insert_at = trim_blank_lines_before(lines, insert_index);
        lines.insert(insert_at, "hooks = true".to_string());
        return;
    }

    if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.push(String::new());
    }
    lines.push("[features]".to_string());
    lines.push("hooks = true".to_string());
}

fn trim_blank_lines_before(lines: &[String], mut index: usize) -> usize {
    while index > 0 && lines[index - 1].trim().is_empty() {
        index -= 1;
    }
    index
}

fn render_codex_hook_config(lines: Vec<String>, newline: &str) -> String {
    let mut output = compact_blank_lines(lines).join(newline);
    output.push_str(newline);
    output
}

fn compact_blank_lines(lines: Vec<String>) -> Vec<String> {
    let mut compacted = Vec::with_capacity(lines.len());
    let mut previous_blank = false;
    for line in lines {
        let is_blank = line.trim().is_empty();
        if is_blank && previous_blank {
            continue;
        }
        previous_blank = is_blank;
        compacted.push(line);
    }
    while compacted.last().is_some_and(|line| line.trim().is_empty()) {
        compacted.pop();
    }
    compacted
}

fn parse_toml_section_name(trimmed_line: &str) -> Option<&str> {
    if trimmed_line.starts_with('[') && trimmed_line.ends_with(']') {
        Some(
            trimmed_line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim(),
        )
    } else {
        None
    }
}

fn is_codex_hooks_assignment(trimmed_line: &str) -> bool {
    let without_comment = trimmed_line.split('#').next().unwrap_or("").trim();
    if without_comment
        .strip_prefix("codex_hooks")
        .is_some_and(|rest| rest.trim_start().starts_with('='))
    {
        return true;
    }
    without_comment
        .strip_prefix("hooks")
        .is_some_and(|rest| rest.trim_start().starts_with('=') && !rest.starts_with('_'))
}

fn rewrite_codex_hooks_line(line: &str) -> String {
    let indent_len = line.chars().take_while(|c| c.is_whitespace()).count();
    let indent = &line[..indent_len];
    let comment = line
        .find('#')
        .map(|index| line[index..].trim_end())
        .filter(|comment| !comment.is_empty());

    match comment {
        Some(comment) => format!("{indent}hooks = true  {comment}"),
        None => format!("{indent}hooks = true"),
    }
}
