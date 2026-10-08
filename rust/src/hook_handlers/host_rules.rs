//! The host's own command rules, read so a rewrite never changes how the host
//! treats a command (INVARIANTS.md §5).
//!
//! Cursor and Gemini CLI get `cmd` rewritten to `sobre -c 'cmd'` before it
//! runs. That is only acceptable when the host would treat both strings the
//! same way: a deny or ask rule that catches `git push` must not be dodged by
//! the wrapper, and an allow rule must not start covering it either. The check
//! is stricter than comparing verdicts: every rule sobre can read has to match
//! both commands or neither. When a rules file exists but cannot be read, the
//! answer is "not the same" and the command runs as written.
//!
//! Rules read:
//! - Cursor: `permissions.allow` / `permissions.deny` `Shell(...)` entries in
//!   `~/.cursor/cli-config.json` and `<project>/.cursor/cli.json`. Rules kept in
//!   the Cursor app's own settings are not on disk in a readable form.
//! - Gemini CLI: `run_shell_command(...)` entries in the `tools` lists and the
//!   legacy top-level lists of `~/.gemini/settings.json` and
//!   `<project>/.gemini/settings.json`, and every `run_shell_command` rule of the
//!   policy engine (`~/.gemini/policies/*.toml` and the system policy dir).

use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Host {
    Cursor,
    Gemini,
}

/// One way a host rule can select a shell command.
#[derive(Debug)]
pub(super) enum Rule {
    /// `Shell(git status)` / `run_shell_command(git)`: the command equals the
    /// pattern or starts with it plus a space; `*` is a wildcard.
    Pattern(String),
    /// Gemini policy `commandPrefix`.
    Prefix(String),
    /// Gemini policy `commandRegex`, tested against the command.
    CommandRegex(Regex),
    /// Gemini policy `argsPattern`, tested against the JSON tool arguments.
    ArgsRegex(Regex),
}

impl Rule {
    fn matches(&self, cmd: &str) -> bool {
        match self {
            Rule::Pattern(p) => pattern_matches(cmd, p),
            Rule::Prefix(p) => cmd.starts_with(p.as_str()),
            Rule::CommandRegex(re) => re.is_match(cmd),
            Rule::ArgsRegex(re) => re.is_match(&serde_json::json!({ "command": cmd }).to_string()),
        }
    }
}

/// True when every readable rule of `host` treats `original` and `rewritten`
/// alike. `false` when a rules file is present but unreadable.
pub(super) fn same_treatment(host: Host, original: &str, rewritten: &str, cwd: &Path) -> bool {
    let Some(rules) = load(host, cwd) else {
        return false;
    };
    rules_agree(&rules, original, rewritten)
}

pub(super) fn rules_agree(rules: &[Rule], original: &str, rewritten: &str) -> bool {
    rules
        .iter()
        .all(|r| r.matches(original) == r.matches(rewritten))
}

fn load(host: Host, cwd: &Path) -> Option<Vec<Rule>> {
    let home = crate::core::home::resolve_home_dir()?;
    let mut rules = Vec::new();
    match host {
        Host::Cursor => {
            for path in [
                home.join(".cursor/cli-config.json"),
                cwd.join(".cursor/cli.json"),
            ] {
                let json = read_json(&path).ok()?;
                if let Some(perms) = json.as_ref().and_then(|j| j.get("permissions")) {
                    for key in ["allow", "deny", "ask"] {
                        collect_wrapped(perms.get(key), &["Shell("], &mut rules);
                    }
                }
            }
        }
        Host::Gemini => {
            for path in [
                home.join(".gemini/settings.json"),
                cwd.join(".gemini/settings.json"),
            ] {
                let json = read_json(&path).ok()?;
                if let Some(json) = json {
                    collect_gemini_settings(&json, &mut rules);
                }
            }
            for dir in gemini_policy_dirs(&home) {
                collect_gemini_policies(&dir, &mut rules)?;
            }
        }
    }
    Some(rules)
}

/// `Ok(None)` → no file; `Err` → a file that cannot be read or parsed.
fn read_json(path: &Path) -> Result<Option<Value>, ()> {
    match std::fs::read_to_string(path) {
        Ok(text) => crate::core::jsonc::parse_jsonc(&text)
            .map(Some)
            .map_err(|_| ()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

fn collect_wrapped(list: Option<&Value>, prefixes: &[&str], rules: &mut Vec<Rule>) {
    let Some(items) = list.and_then(Value::as_array) else {
        return;
    };
    for item in items.iter().filter_map(Value::as_str) {
        for prefix in prefixes {
            if let Some(inner) = item.trim().strip_prefix(prefix)
                && let Some(pattern) = inner.strip_suffix(')')
            {
                rules.push(Rule::Pattern(pattern.trim().to_string()));
            }
        }
    }
}

const GEMINI_SHELL: &[&str] = &["run_shell_command(", "ShellTool("];

fn collect_gemini_settings(json: &Value, rules: &mut Vec<Rule>) {
    let tools = json.get("tools");
    for key in ["allowed", "exclude", "core", "confirmationRequired"] {
        collect_wrapped(tools.and_then(|t| t.get(key)), GEMINI_SHELL, rules);
    }
    for key in ["allowedTools", "excludeTools", "coreTools"] {
        collect_wrapped(json.get(key), GEMINI_SHELL, rules);
    }
}

fn gemini_policy_dirs(home: &Path) -> Vec<PathBuf> {
    let system = if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/GeminiCli/policies")
    } else if cfg!(windows) {
        PathBuf::from(r"C:\ProgramData\gemini-cli\policies")
    } else {
        PathBuf::from("/etc/gemini-cli/policies")
    };
    vec![home.join(".gemini/policies"), system]
}

/// Adds every shell rule of the `*.toml` policies in `dir`. `None` when a
/// policy file cannot be read or parsed.
fn collect_gemini_policies(dir: &Path, rules: &mut Vec<Rule>) -> Option<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Some(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        let doc: toml::Table = toml::from_str(&text).ok()?;
        let Some(list) = doc.get("rule").and_then(toml::Value::as_array) else {
            continue;
        };
        for rule in list.iter().filter_map(toml::Value::as_table) {
            collect_gemini_policy_rule(rule, rules)?;
        }
    }
    Some(())
}

fn collect_gemini_policy_rule(rule: &toml::Table, rules: &mut Vec<Rule>) -> Option<()> {
    let applies = match rule.get("toolName") {
        None => true,
        Some(toml::Value::String(name)) => name == "run_shell_command" || name == "*",
        Some(toml::Value::Array(names)) => names
            .iter()
            .any(|n| matches!(n.as_str(), Some("run_shell_command" | "*"))),
        Some(_) => return None,
    };
    if !applies {
        return Some(());
    }
    match rule.get("commandPrefix") {
        Some(toml::Value::String(p)) => rules.push(Rule::Prefix(p.clone())),
        Some(toml::Value::Array(ps)) => {
            for p in ps {
                rules.push(Rule::Prefix(p.as_str()?.to_string()));
            }
        }
        Some(_) => return None,
        None => {}
    }
    if let Some(v) = rule.get("commandRegex") {
        rules.push(Rule::CommandRegex(Regex::new(v.as_str()?).ok()?));
    }
    if let Some(v) = rule.get("argsPattern") {
        rules.push(Rule::ArgsRegex(Regex::new(v.as_str()?).ok()?));
    }
    Some(())
}

/// `*` matches everything; a pattern ending in `*` (or `:*`) is a word-bounded
/// prefix; any other `*` is a wildcard; otherwise the command equals the
/// pattern or starts with it plus a space.
fn pattern_matches(cmd: &str, pattern: &str) -> bool {
    let cmd = normalize(cmd);
    let pattern = normalize(pattern);
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        let prefix = prefix.trim_end_matches(':').trim_end();
        if prefix.is_empty() {
            return true;
        }
        if !prefix.contains('*') {
            return cmd == prefix || cmd.starts_with(&format!("{prefix} "));
        }
    }
    if pattern.contains('*') {
        return wildcard_matches(&cmd, &pattern.replace(":*", " *"));
    }
    cmd == pattern || cmd.starts_with(&format!("{pattern} "))
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn wildcard_matches(text: &str, pattern: &str) -> bool {
    let regex = format!(
        "^{}$",
        pattern
            .split('*')
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join(".*")
    );
    Regex::new(&regex).is_ok_and(|re| re.is_match(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WRAPPED: &str = "sobre -c 'git push origin main'";
    const ORIGINAL: &str = "git push origin main";

    #[test]
    fn no_rules_means_same_treatment() {
        assert!(rules_agree(&[], ORIGINAL, WRAPPED));
    }

    #[test]
    fn a_rule_naming_the_command_blocks_the_rewrite() {
        for p in ["git push", "git push *", "git:*", "git"] {
            let rules = [Rule::Pattern(p.to_string())];
            assert!(!rules_agree(&rules, ORIGINAL, WRAPPED), "pattern {p}");
        }
        let rules = [Rule::Prefix("git push".into())];
        assert!(!rules_agree(&rules, ORIGINAL, WRAPPED));
        let rules = [Rule::CommandRegex(Regex::new("^git ").unwrap())];
        assert!(!rules_agree(&rules, ORIGINAL, WRAPPED));
        let rules = [Rule::ArgsRegex(
            Regex::new(r#""command":"git push"#).unwrap(),
        )];
        assert!(!rules_agree(&rules, ORIGINAL, WRAPPED));
    }

    #[test]
    fn rules_matching_both_or_neither_allow_the_rewrite() {
        let rules = [
            Rule::Pattern("*".into()),
            Rule::Pattern("npm test".into()),
            Rule::Prefix("rm -rf".into()),
            Rule::CommandRegex(Regex::new("origin").unwrap()),
        ];
        assert!(rules_agree(&rules, ORIGINAL, WRAPPED));
    }

    #[test]
    fn a_rule_naming_the_wrapper_blocks_the_rewrite() {
        let rules = [Rule::Pattern("sobre".into())];
        assert!(!rules_agree(&rules, ORIGINAL, WRAPPED));
    }

    #[test]
    fn gemini_policy_rules_are_read() {
        let doc: toml::Table = toml::from_str(
            r#"
            [[rule]]
            toolName = "run_shell_command"
            commandPrefix = ["git push", "rm"]
            decision = "deny"
            priority = 100

            [[rule]]
            toolName = "read_file"
            decision = "allow"
            priority = 100
            "#,
        )
        .unwrap();
        let mut rules = Vec::new();
        for rule in doc["rule"].as_array().unwrap() {
            collect_gemini_policy_rule(rule.as_table().unwrap(), &mut rules).unwrap();
        }
        assert_eq!(rules.len(), 2);
        assert!(!rules_agree(&rules, ORIGINAL, WRAPPED));
    }

    #[test]
    fn unreadable_policy_regex_refuses_the_rewrite() {
        let doc: toml::Table =
            toml::from_str("[[rule]]\ntoolName = \"run_shell_command\"\ncommandRegex = \"(\"\n")
                .unwrap();
        let rule = doc["rule"].as_array().unwrap()[0].as_table().unwrap();
        assert!(collect_gemini_policy_rule(rule, &mut Vec::new()).is_none());
    }

    #[test]
    fn settings_lists_are_read() {
        let json = serde_json::json!({
            "tools": { "allowed": ["run_shell_command(git status)", "read_file"] },
            "excludeTools": ["run_shell_command(git push)"]
        });
        let mut rules = Vec::new();
        collect_gemini_settings(&json, &mut rules);
        assert_eq!(rules.len(), 2);
    }
}
