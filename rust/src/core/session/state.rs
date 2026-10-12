use super::paths::extract_cd_target;
use super::types::SessionState;

impl SessionState {
    /// Creates an empty session: no root, no shell directory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the effective working directory for shell commands.
    /// Priority: explicit cwd arg (relative to the project root) > session
    /// shell_cwd > project_root > process cwd. sobre does not confine it: the
    /// agent host decides where commands may run (invariant 5).
    pub fn effective_cwd(&self, explicit_cwd: Option<&str>) -> String {
        if let Some(cwd) = explicit_cwd
            && !cwd.is_empty()
            && cwd != "."
        {
            let p = std::path::Path::new(cwd);
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::path::Path::new(self.project_root.as_deref().unwrap_or(".")).join(p)
            };
            return crate::core::pathutil::safe_canonicalize_or_self(&abs)
                .to_string_lossy()
                .to_string();
        }
        if let Some(ref cwd) = self.shell_cwd {
            return cwd.clone();
        }
        if let Some(ref r) = self.project_root {
            return r.clone();
        }
        std::env::current_dir()
            .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string())
    }

    /// Persist an explicit `cwd` argument as the live shell
    /// cwd. `update_shell_cwd` only tracks `cd` inside the command
    /// text, but clients that switch checkouts mid-session (Claude Code
    /// after `EnterWorktree`) pass the new directory as the `cwd` *param* of
    /// every subsequent call — without persisting it, the worktree-divergence
    /// detection in path resolution never sees the switch.
    pub fn note_explicit_cwd(&mut self, cwd: &str) {
        let path = std::path::Path::new(cwd);
        if path.is_absolute() && path.is_dir() {
            self.shell_cwd = Some(
                crate::core::pathutil::safe_canonicalize_or_self(path)
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }

    /// Updates shell_cwd by detecting `cd` in the command.
    /// Handles: `cd /abs/path`, `cd rel/path` (relative to current cwd),
    /// `cd ..`, and chained commands like `cd foo && ...`.
    pub fn update_shell_cwd(&mut self, command: &str) {
        let base = self.effective_cwd(None);
        if let Some(new_cwd) = extract_cd_target(command, &base) {
            let path = std::path::Path::new(&new_cwd);
            if path.exists() && path.is_dir() {
                self.shell_cwd = Some(
                    crate::core::pathutil::safe_canonicalize_or_self(path)
                        .to_string_lossy()
                        .to_string(),
                );
            }
        }
    }
}
