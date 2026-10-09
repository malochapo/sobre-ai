use std::path::PathBuf;

use super::super::{resolve_hook_command_binary, to_bash_compatible_path};

fn copilot_hook_entry(binary: &str, action: &str, timeout_sec: u64) -> serde_json::Value {
    let bash_binary = to_bash_compatible_path(binary);
    serde_json::json!({
        "type": "command",
        "bash": format!("\"{bash_binary}\" hook {action}"),
        "powershell": format!("& \"{binary}\" hook {action}"),
        "timeoutSec": timeout_sec
    })
}

/// User-level Copilot hooks live under `~/.copilot/hooks`, or under
/// `$COPILOT_HOME/hooks`.
pub(crate) fn copilot_hooks_dir(home: &std::path::Path) -> PathBuf {
    if let Ok(copilot_home) = std::env::var("COPILOT_HOME") {
        let trimmed = copilot_home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("hooks");
        }
    }
    home.join(".copilot").join("hooks")
}

fn copilot_user_hooks_dir() -> Option<PathBuf> {
    crate::core::home::resolve_home_dir().map(|h| copilot_hooks_dir(&h))
}

/// Copilot's `postToolUse` hooks: shell output compression and observe.
pub(crate) fn install_copilot_hooks() -> Result<(), String> {
    let binary = resolve_hook_command_binary();
    let mut compress = copilot_hook_entry(&binary, "compress", 15);
    compress["matcher"] = serde_json::json!("bash|powershell");
    let observe = copilot_hook_entry(&binary, "observe", 5);

    let dir = copilot_user_hooks_dir()
        .ok_or_else(|| "cannot resolve the Copilot hooks directory".to_string())?;
    crate::config_io::edit_json(&dir.join("hooks.json"), |root| {
        root.entry("version".to_string())
            .or_insert_with(|| serde_json::json!(1));
        let hooks = root
            .entry("hooks".to_string())
            .or_insert_with(|| serde_json::json!({}));
        let Some(hooks_obj) = hooks.as_object_mut() else {
            return;
        };
        let post = hooks_obj
            .entry("postToolUse".to_string())
            .or_insert_with(|| serde_json::json!([]));
        let Some(post_arr) = post.as_array_mut() else {
            return;
        };
        post_arr.retain(|e| !is_sobre_entry(e));
        post_arr.push(compress);
        post_arr.push(observe);
    })
}

fn is_sobre_entry(entry: &serde_json::Value) -> bool {
    ["bash", "powershell"].iter().any(|k| {
        entry
            .get(*k)
            .and_then(|c| c.as_str())
            .is_some_and(crate::hooks::is_sobre_hook_command)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_entry_carries_bash_and_powershell_with_quoting() {
        // Windows-style install path with a space — the failure shape.
        let entry =
            copilot_hook_entry(r"C:\Users\Jane Doe\AppData\Local\sobre.exe", "compress", 15);
        assert_eq!(
            entry["bash"], "\"/c/Users/Jane Doe/AppData/Local/sobre.exe\" hook compress",
            "bash field must use a quoted MSYS-style path"
        );
        assert_eq!(
            entry["powershell"],
            "& \"C:\\Users\\Jane Doe\\AppData\\Local\\sobre.exe\" hook compress",
            "powershell field must invoke the quoted native path via the call operator"
        );
        assert_eq!(entry["timeoutSec"], 15);
        assert_eq!(entry["type"], "command");

        // Unix paths stay untouched (modulo quoting).
        let unix = copilot_hook_entry("/usr/local/bin/sobre", "observe", 5);
        assert_eq!(unix["bash"], "\"/usr/local/bin/sobre\" hook observe");
        assert_eq!(
            unix["powershell"],
            "& \"/usr/local/bin/sobre\" hook observe"
        );
    }
}
