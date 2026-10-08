// ---------------------------------------------------------------------------
// JSON removal — textual approach preserving comments and formatting
// ---------------------------------------------------------------------------

pub(super) fn remove_sobre_from_json(content: &str) -> Option<String> {
    // Try textual removal first (preserves comments, formatting, key order)
    if let Some(result) = remove_sobre_from_json_textual(content) {
        return Some(result);
    }

    // Fallback to serde-based approach for edge cases
    remove_sobre_from_json_serde(content)
}

/// Textual JSON key removal: finds `"sobre"` key-value pairs and removes
/// them from the raw text without re-serializing. Preserves JSONC comments,
/// formatting, trailing commas, and key ordering.
fn remove_sobre_from_json_textual(content: &str) -> Option<String> {
    let mut result = content.to_string();
    let mut modified = false;

    // Repeatedly find and remove "sobre" entries until none remain.
    // Each iteration rescans because positions shift after removal.
    while let Some(key_start) = find_json_key_position(result.as_bytes(), "sobre") {
        let Some(new_result) = remove_json_entry_at(&result, key_start) else {
            break;
        };

        result = new_result;
        modified = true;
    }

    if modified {
        // Validate the result is still valid JSON(C) if the input was valid
        if crate::core::jsonc::parse_jsonc(&result).is_ok() {
            Some(result)
        } else if crate::core::jsonc::parse_jsonc(content).is_ok() {
            // Input was valid but our textual removal broke it — don't use this result
            None
        } else {
            // Input was already invalid, return our best effort
            Some(result)
        }
    } else {
        None
    }
}

/// Find the byte position of a JSON key `"key_name"` that is followed by `:`.
fn find_json_key_position(bytes: &[u8], key_name: &str) -> Option<usize> {
    let needle = format!("\"{key_name}\"");
    let needle_bytes = needle.as_bytes();
    let mut i = 0;

    while i + needle_bytes.len() <= bytes.len() {
        if &bytes[i..i + needle_bytes.len()] == needle_bytes {
            // Check it's followed by `:` (after optional whitespace)
            let after = i + needle_bytes.len();
            let mut j = after;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b':' {
                // Make sure we're not inside a string by checking if we have
                // an even number of unescaped quotes before this position
                if !is_inside_string(bytes, i) {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// Check if position `pos` is inside a JSON string literal.
fn is_inside_string(bytes: &[u8], pos: usize) -> bool {
    let mut in_string = false;
    let mut i = 0;
    while i < pos {
        match bytes[i] {
            b'"' if !in_string => in_string = true,
            b'"' if in_string => in_string = false,
            b'\\' if in_string => {
                i += 1; // skip escaped char
            }
            b'/' if !in_string && i + 1 < bytes.len() => {
                if bytes[i + 1] == b'/' {
                    // Line comment — skip to end of line
                    while i < pos && i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                } else if bytes[i + 1] == b'*' {
                    // Block comment — skip to */
                    i += 2;
                    while i + 1 < bytes.len() {
                        if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    in_string
}

/// Remove a JSON key-value entry starting at `key_start` position.
/// Handles surrounding commas and whitespace.
fn remove_json_entry_at(content: &str, key_start: usize) -> Option<String> {
    let bytes = content.as_bytes();

    // Find the colon after the key
    let key_name_end = content[key_start + 1..].find('"')? + key_start + 2;
    let mut colon_pos = key_name_end;
    while colon_pos < bytes.len() && bytes[colon_pos] != b':' {
        colon_pos += 1;
    }
    if colon_pos >= bytes.len() {
        return None;
    }

    // Skip the value
    let value_start = colon_pos + 1;
    let value_end = skip_json_value(bytes, value_start)?;

    // Determine the range to remove, including surrounding comma and whitespace.
    // Scan backwards from key_start to find leading comma or whitespace.
    let mut remove_start = key_start;

    // Look backwards for a comma (we might be after a comma)
    let mut scan_back = key_start;
    while scan_back > 0 {
        scan_back -= 1;
        let ch = bytes[scan_back];
        if ch == b',' {
            remove_start = scan_back;
            break;
        }
        if ch == b'{' || ch == b'[' {
            break;
        }
        if !ch.is_ascii_whitespace() {
            break;
        }
    }

    // Extend remove_start back to include the newline before the comma/key
    if remove_start > 0 && remove_start == key_start {
        let mut ns = remove_start;
        while ns > 0 && bytes[ns - 1].is_ascii_whitespace() && bytes[ns - 1] != b'\n' {
            ns -= 1;
        }
        if ns > 0 && bytes[ns - 1] == b'\n' {
            remove_start = ns;
        }
    }

    let mut remove_end = value_end;

    // Look forward for a trailing comma
    let mut scan_fwd = value_end;
    while scan_fwd < bytes.len() && bytes[scan_fwd].is_ascii_whitespace() {
        scan_fwd += 1;
    }
    if scan_fwd < bytes.len() && bytes[scan_fwd] == b',' {
        // If we already consumed a leading comma, don't consume trailing too
        if remove_start < key_start && remove_start < bytes.len() && bytes[remove_start] == b',' {
            // Already have leading comma removed, skip trailing
        } else {
            remove_end = scan_fwd + 1;
        }
    }

    // Skip trailing whitespace/newline after the removed entry
    while remove_end < bytes.len()
        && (bytes[remove_end] == b' ' || bytes[remove_end] == b'\t' || bytes[remove_end] == b'\r')
    {
        remove_end += 1;
    }
    if remove_end < bytes.len() && bytes[remove_end] == b'\n' {
        remove_end += 1;
    }

    let mut result = String::with_capacity(content.len());
    result.push_str(&content[..remove_start]);
    result.push_str(&content[remove_end..]);
    Some(result)
}

/// Skip over a JSON value (object, array, string, number, boolean, null)
/// starting from `start`. Returns the position after the value.
fn skip_json_value(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;

    // Skip whitespace
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }

    match bytes[i] {
        b'{' | b'[' => {
            let open = bytes[i];
            let close = if open == b'{' { b'}' } else { b']' };
            let mut depth = 1;
            i += 1;
            while i < bytes.len() && depth > 0 {
                match bytes[i] {
                    c if c == open => depth += 1,
                    c if c == close => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(i + 1);
                        }
                    }
                    b'"' => {
                        i += 1;
                        while i < bytes.len() {
                            if bytes[i] == b'\\' {
                                i += 1;
                            } else if bytes[i] == b'"' {
                                break;
                            }
                            i += 1;
                        }
                    }
                    b'/' if i + 1 < bytes.len() => {
                        if bytes[i + 1] == b'/' {
                            while i < bytes.len() && bytes[i] != b'\n' {
                                i += 1;
                            }
                            continue;
                        } else if bytes[i + 1] == b'*' {
                            i += 2;
                            while i + 1 < bytes.len() {
                                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                                    i += 1;
                                    break;
                                }
                                i += 1;
                            }
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            Some(i)
        }
        b'"' => {
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 1;
                } else if bytes[i] == b'"' {
                    return Some(i + 1);
                }
                i += 1;
            }
            None
        }
        _ => {
            // Number, boolean, null
            while i < bytes.len() && !matches!(bytes[i], b',' | b'}' | b']' | b'\n' | b'\r') {
                i += 1;
            }
            Some(i)
        }
    }
}

/// Fallback: serde-based JSON removal (destroys comments/formatting).
fn remove_sobre_from_json_serde(content: &str) -> Option<String> {
    let mut parsed: serde_json::Value = crate::core::jsonc::parse_jsonc(content).ok()?;
    let mut modified = false;

    if let Some(servers) = parsed.get_mut("mcpServers").and_then(|s| s.as_object_mut()) {
        modified |= servers.remove("sobre").is_some();
    }

    if let Some(servers) = parsed.get_mut("servers").and_then(|s| s.as_object_mut()) {
        modified |= servers.remove("sobre").is_some();
    }

    if let Some(mcp) = parsed.get_mut("mcp").and_then(|s| s.as_object_mut()) {
        modified |= mcp.remove("sobre").is_some();
    }

    // Zed uses `context_servers` instead of `mcpServers`
    if let Some(ctx) = parsed
        .get_mut("context_servers")
        .and_then(|s| s.as_object_mut())
    {
        modified |= ctx.remove("sobre").is_some();
    }

    if modified {
        Some(serde_json::to_string_pretty(&parsed).ok()? + "\n")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// TOML removal
// ---------------------------------------------------------------------------

pub(super) fn remove_sobre_from_toml(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut skip = false;
    let mut in_features = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let section = trimmed.trim_start_matches('[').trim_end_matches(']').trim();
            if section == "mcp_servers.sobre"
                || section == "mcp_servers.\"sobre\""
                || section.starts_with("mcp_servers.sobre.")
                || section.starts_with("mcp_servers.\"sobre\".")
            {
                skip = true;
                continue;
            }
            skip = false;
            in_features = section == "features";
        }

        if skip {
            continue;
        }

        // Install turns on Codex's hooks flag inside [features] only, under
        // either of Codex's names for it; undo exactly that.
        let without_comment = trimmed.split('#').next().unwrap_or("").trim();
        if in_features
            && crate::hooks::is_codex_hooks_assignment(trimmed)
            && without_comment.contains("true")
        {
            out.push_str(&line.replace("true", "false"));
            out.push('\n');
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    let cleaned: String = out
        .lines()
        .filter(|l| l.trim() != "[]")
        .collect::<Vec<_>>()
        .join("\n");
    if cleaned.is_empty() {
        cleaned
    } else {
        cleaned + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::super::agents::{HookCleanupResult, remove_sobre_from_hooks_json};
    use super::super::{backup_before_modify, bak_path_for};
    use super::*;

    // --- TOML tests ---

    #[test]
    fn remove_toml_mcp_server_section() {
        let input = "\
[features]
hooks = true

[mcp_servers.sobre]
command = \"/usr/local/bin/sobre\"
args = []

[mcp_servers.other-tool]
command = \"/usr/bin/other\"
";
        let result = remove_sobre_from_toml(input);
        assert!(!result.contains("sobre"), "sobre section should be removed");
        assert!(
            result.contains("[mcp_servers.other-tool]"),
            "other sections should be preserved"
        );
        assert!(
            result.contains("hooks = false"),
            "hooks should be set to false"
        );
    }

    #[test]
    fn remove_toml_turns_off_only_the_features_hooks_flag() {
        let input = "\
[features]
codex_hooks = true

[tools.other]
hooks = true
";
        let result = remove_sobre_from_toml(input);
        assert!(result.contains("codex_hooks = false"), "{result}");
        assert!(
            result.contains("[tools.other]\nhooks = true"),
            "a hooks key outside [features] belongs to someone else: {result}"
        );
    }

    #[test]
    fn remove_toml_only_sobre() {
        let input = "\
[mcp_servers.sobre]
command = \"sobre\"
";
        let result = remove_sobre_from_toml(input);
        assert!(
            result.trim().is_empty(),
            "should produce empty output: {result}"
        );
    }

    #[test]
    fn remove_toml_no_sobre() {
        let input = "\
[mcp_servers.other]
command = \"other\"
";
        let result = remove_sobre_from_toml(input);
        assert!(
            result.contains("[mcp_servers.other]"),
            "other content should be preserved"
        );
    }

    // --- JSON textual removal tests ---

    #[test]
    fn json_textual_removes_key_from_object() {
        let input = r#"{
  "mcpServers": {
    "other-tool": {
      "command": "other"
    },
    "sobre": {
      "command": "/usr/bin/sobre",
      "args": []
    }
  }
}
"#;
        let result = remove_sobre_from_json(input).expect("should find sobre");
        assert!(!result.contains("sobre"), "sobre should be removed");
        assert!(
            result.contains("other-tool"),
            "other-tool should be preserved"
        );
        // Verify valid JSON
        assert!(
            crate::core::jsonc::parse_jsonc(&result).is_ok(),
            "result should be valid JSON: {result}"
        );
    }

    #[test]
    fn json_textual_preserves_comments() {
        let input = r#"{
  // This is a user comment
  "mcpServers": {
    "sobre": {
      "command": "sobre"
    },
    "my-tool": {
      "command": "my-tool"
    }
  }
}
"#;
        let result = remove_sobre_from_json(input).expect("should find sobre");
        assert!(!result.contains("sobre"), "sobre should be removed");
        assert!(
            result.contains("// This is a user comment"),
            "comment should be preserved: {result}"
        );
        assert!(result.contains("my-tool"), "my-tool should be preserved");
    }

    #[test]
    fn json_textual_only_sobre() {
        let input = r#"{
  "mcpServers": {
    "sobre": {
      "command": "sobre"
    }
  }
}
"#;
        let result = remove_sobre_from_json(input).expect("should find sobre");
        assert!(!result.contains("sobre"), "sobre should be removed");
    }

    #[test]
    fn json_no_sobre_returns_none() {
        let input = r#"{"mcpServers": {"other": {"command": "other"}}}"#;
        assert!(remove_sobre_from_json(input).is_none());
    }

    // --- hooks.json tests ---

    #[test]
    fn hooks_json_entirely_sobre_no_other_keys() {
        let input = r#"{
  "hooks": {
    "preToolUse": [
      {
        "matcher": "Shell",
        "command": "sobre hook compress"
      }
    ]
  }
}"#;
        assert!(
            matches!(
                remove_sobre_from_hooks_json(input),
                HookCleanupResult::EntirelySobre
            ),
            "should return EntirelySobre when all hooks are sobre and no other keys"
        );
    }

    #[test]
    fn hooks_json_version_only_boilerplate_is_entirely_sobre() {
        // `version` / `$schema` are installer boilerplate: once the sobre
        // hooks are gone, `{"hooks": {}, "version": 1}` carries no user
        // content and must be deleted instead of left behind (GL #558).
        let input = r#"{
  "version": 1,
  "hooks": {
    "preToolUse": [
      {
        "matcher": "Shell",
        "command": "sobre hook compress"
      }
    ]
  }
}"#;
        assert!(
            matches!(
                remove_sobre_from_hooks_json(input),
                HookCleanupResult::EntirelySobre
            ),
            "version-only leftovers should be EntirelySobre"
        );
    }

    #[test]
    fn hooks_json_version_key_with_user_hooks_is_cleaned_not_deleted() {
        let input = r#"{
  "version": 1,
  "hooks": {
    "preToolUse": [
      {
        "matcher": "Shell",
        "command": "sobre hook compress"
      },
      {
        "matcher": "Shell",
        "command": "my-other-tool hook"
      }
    ]
  }
}"#;
        let result = match remove_sobre_from_hooks_json(input) {
            HookCleanupResult::Cleaned(s) => s,
            other => panic!("expected Cleaned, got {other:?}"),
        };
        assert!(
            result.contains("version"),
            "version key should be preserved alongside user hooks"
        );
        assert!(!result.contains("sobre"), "sobre should be removed");
        assert!(
            result.contains("my-other-tool"),
            "user hooks should survive"
        );
    }

    #[test]
    fn hooks_json_handles_nested_claude_format() {
        let input = r#"{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "sobre hook compress"
          }
        ]
      },
      {
        "matcher": "Other",
        "hooks": [
          {
            "type": "command",
            "command": "my-other-tool check"
          }
        ]
      }
    ]
  }
}"#;
        let result = match remove_sobre_from_hooks_json(input) {
            HookCleanupResult::Cleaned(s) => s,
            other => panic!("expected Cleaned, got {other:?}"),
        };
        assert!(!result.contains("sobre"), "sobre nested entry removed");
        assert!(
            result.contains("my-other-tool"),
            "non-sobre entries preserved"
        );
    }

    #[test]
    fn hooks_json_mixed_nested_group_preserves_user_hooks() {
        let input = r#"{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "sobre hook compress" },
          { "type": "command", "command": "my-custom-guard" }
        ]
      }
    ]
  }
}"#;
        let result = match remove_sobre_from_hooks_json(input) {
            HookCleanupResult::Cleaned(s) => s,
            other => panic!("expected Cleaned, got {other:?}"),
        };
        assert!(!result.contains("sobre"), "sobre sub-hook removed");
        assert!(
            result.contains("my-custom-guard"),
            "user sub-hook in same group preserved"
        );
    }

    #[test]
    fn hooks_json_copilot_bash_format() {
        let input = r#"{
  "hooks": {
    "preToolUse": [
      { "bash": "sobre hook compress" },
      { "bash": "my-other-hook" }
    ]
  }
}"#;
        let result = match remove_sobre_from_hooks_json(input) {
            HookCleanupResult::Cleaned(s) => s,
            other => panic!("expected Cleaned, got {other:?}"),
        };
        assert!(!result.contains("sobre"), "sobre bash entry removed");
        assert!(
            result.contains("my-other-hook"),
            "other bash hook preserved"
        );
    }

    #[test]
    fn hooks_json_permissions_are_left_alone() {
        // Permissions are the user's: uninstall never edits them, even entries
        // that name sobre.
        let input = r#"{
  "permissions": {
    "allow": ["mcp__sobre__ctx_read"],
    "deny": ["mcp__sobre__ctx_shell"]
  }
}"#;
        assert!(matches!(
            remove_sobre_from_hooks_json(input),
            HookCleanupResult::Unchanged
        ));
    }

    #[test]
    fn hooks_json_parse_error_does_not_delete() {
        let input = "{ this is not valid JSON at all !!!";
        assert!(
            matches!(
                remove_sobre_from_hooks_json(input),
                HookCleanupResult::ParseError
            ),
            "parse errors should return ParseError, not delete the file"
        );
    }

    #[test]
    fn hooks_json_no_sobre_returns_unchanged() {
        let input = r#"{
  "hooks": {
    "preToolUse": [
      { "command": "some-other-tool" }
    ]
  }
}"#;
        assert!(
            matches!(
                remove_sobre_from_hooks_json(input),
                HookCleanupResult::Unchanged
            ),
            "should return Unchanged when no sobre found"
        );
    }

    #[test]
    fn backup_before_modify_respects_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "hello").unwrap();

        backup_before_modify(&path, true);
        assert!(
            !bak_path_for(&path).exists(),
            "dry-run must not create backups"
        );

        backup_before_modify(&path, false);
        assert!(
            bak_path_for(&path).exists(),
            "non-dry-run should create backups"
        );
    }
}
