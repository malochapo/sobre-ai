use super::server::SobreServer;
use super::startup::{
    has_project_marker, is_suspicious_root, maybe_derive_project_root_from_absolute,
};

impl SobreServer {
    /// Resolves a tool path through [`crate::core::path_resolve::resolve_tool_path`]
    /// with the session's project root and shell cwd: an empty path or `.` is
    /// the project root, and a relative path never resolves against the
    /// server's own cwd.
    pub async fn resolve_path(&self, path: &str) -> Result<String, String> {
        let normalized = crate::core::pathutil::normalize_tool_path(path);
        let p = std::path::Path::new(&normalized);
        let (project_root, shell_cwd) = {
            let session = self.session.read().await;
            (session.project_root.clone(), session.shell_cwd.clone())
        };
        // A few `exists()` probes: cheap enough to run on the async thread.
        let resolved = crate::core::path_resolve::resolve_tool_path(
            project_root.as_deref(),
            shell_cwd.as_deref(),
            path,
        )?;
        let resolved = std::path::PathBuf::from(resolved);
        let root = project_root
            .or(shell_cwd)
            .unwrap_or_else(|| ".".to_string());

        // sobre does not confine paths: the agent host decides what may be
        // read or written (invariant 5). An absolute path outside a root that
        // is not a real project (an agent config dir, a markerless client cwd)
        // re-roots the session on the project that path belongs to. The
        // dispatcher says so in that call's output.
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
        }
        Ok(resolved.to_string_lossy().to_string())
    }
}
