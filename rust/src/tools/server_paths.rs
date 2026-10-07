use super::server::LeanCtxServer;
use super::startup::{
    has_project_marker, is_suspicious_root, maybe_derive_project_root_from_absolute,
};

impl LeanCtxServer {
    /// Resolves a (possibly relative) tool path against the session's project_root.
    /// Absolute paths and "." are returned as-is. Relative paths like "src/main.rs"
    /// are joined with project_root so tools work regardless of the server's cwd.
    pub async fn resolve_path(&self, path: &str) -> Result<String, String> {
        let normalized = crate::core::pathutil::normalize_tool_path(path);
        if normalized.is_empty() || normalized == "." {
            return Ok(normalized);
        }
        let p = std::path::Path::new(&normalized);

        let (resolved, root) = {
            let session = self.session.read().await;
            let root = session
                .project_root
                .as_deref()
                .or(session.shell_cwd.as_deref())
                .unwrap_or(".")
                .to_string();

            // #707: a shell_cwd tracking a mid-session worktree switch
            // (different git checkout) outranks the stale project_root — same
            // precedence as core::path_resolve. Checked BEFORE the `p.exists()`
            // probe below: that probe runs against the *process* CWD, which
            // IDEs routinely set to the original project root, so it would
            // short-circuit every relative path back to the stale checkout and
            // the divergence rule could never apply.
            let worktree_cwd = if p.is_absolute() {
                None
            } else {
                session
                    .project_root
                    .as_deref()
                    .zip(session.shell_cwd.as_deref())
                    .filter(|(root, cwd)| {
                        crate::core::path_resolve::shell_cwd_is_divergent_checkout(root, cwd)
                    })
                    .map(|(_, cwd)| std::path::Path::new(cwd).join(&normalized))
            };

            let resolved = if let Some(overridden) = worktree_cwd {
                overridden
            } else if p.is_absolute() || p.exists() {
                std::path::PathBuf::from(&normalized)
            } else if let Some(ref root) = session.project_root {
                let joined = std::path::Path::new(root).join(&normalized);
                if joined.exists() {
                    joined
                } else if let Some(ref cwd) = session.shell_cwd {
                    std::path::Path::new(cwd).join(&normalized)
                } else {
                    std::path::Path::new(&root).join(&normalized)
                }
            } else if let Some(ref cwd) = session.shell_cwd {
                std::path::Path::new(cwd).join(&normalized)
            } else {
                std::path::Path::new(&root).join(&normalized)
            };

            (resolved, root)
        };

        // sobre does not confine paths: the agent host decides what may be
        // read or written (invariant 6). An absolute path outside a root that
        // is not a real project (an agent config dir, a markerless client cwd)
        // re-roots the session on the project that path belongs to (#580/#649).
        let root_path = std::path::Path::new(&root);
        if p.is_absolute()
            && !resolved.starts_with(root_path)
            && (is_suspicious_root(root_path)
                || (self.startup_project_root.is_none() && !has_project_marker(root_path)))
            && let Some(new_root) = maybe_derive_project_root_from_absolute(&resolved)
        {
            let mut session = self.session.write().await;
            let new_root_str = new_root.to_string_lossy().to_string();
            session.project_root = Some(new_root_str.clone());
            session.shell_cwd = self
                .startup_shell_cwd
                .as_ref()
                .filter(|cwd| std::path::Path::new(cwd).starts_with(&new_root))
                .cloned()
                .or_else(|| Some(new_root_str.clone()));
            let _ = session.save();
        }
        let resolved = crate::core::pathutil::canonicalize_or_self(&resolved);

        Ok(crate::core::pathutil::normalize_tool_path(
            &resolved.to_string_lossy().replace('\\', "/"),
        ))
    }

    /// Like `resolve_path`, but returns the original path on failure instead of an error.
    pub async fn resolve_path_or_passthrough(&self, path: &str) -> String {
        self.resolve_path(path)
            .await
            .unwrap_or_else(|_| path.to_string())
    }
}
