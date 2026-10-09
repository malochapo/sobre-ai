pub(crate) fn format_tokens_cli(tokens: u64) -> String {
    if tokens >= 1_000_000_000_000 {
        format!("{:.2}T", tokens as f64 / 1_000_000_000_000.0)
    } else if tokens >= 1_000_000_000 {
        // Heavy users cross 1B; keep growing visibly instead of "1310.0M".
        format!("{:.2}B", tokens as f64 / 1_000_000_000.0)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{tokens}")
    }
}

/// The project a command works in: `explicit` (`--root`, `--project-root`)
/// when given, else the git root around the current directory.
pub(crate) fn project_root(explicit: Option<&str>) -> String {
    if let Some(root) = explicit.filter(|r| !r.trim().is_empty()) {
        return normalize_explicit_root(root);
    }
    let cwd = std::env::current_dir()
        .ok()
        .map_or_else(|| ".".to_string(), |p| p.to_string_lossy().to_string());
    promote_to_git_root(&cwd)
}

fn normalize_explicit_root(path: &str) -> String {
    let expanded = expand_home(path.trim());
    crate::core::index_paths::normalize_project_root(&expanded)
}

fn expand_home(path: &str) -> String {
    if path == "~" {
        return dirs::home_dir().map_or_else(
            || path.to_string(),
            |home| home.to_string_lossy().to_string(),
        );
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return dirs::home_dir().map_or_else(
            || path.to_string(),
            |home| home.join(rest).to_string_lossy().to_string(),
        );
    }
    path.to_string()
}

fn promote_to_git_root(path: &str) -> String {
    let mut p = std::path::Path::new(path);
    loop {
        if p.join(".git").exists() {
            return p.to_string_lossy().to_string();
        }
        match p.parent() {
            Some(parent) => p = parent,
            None => return path.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{format_tokens_cli, normalize_explicit_root, project_root};

    #[test]
    fn format_tokens_cli_scales_through_billions() {
        assert_eq!(format_tokens_cli(742), "742");
        assert_eq!(format_tokens_cli(2_500), "2.5K");
        assert_eq!(format_tokens_cli(3_400_000), "3.4M");
        // Must read as billions once a heavy user crosses 1B, not "1310.0M".
        assert_eq!(format_tokens_cli(1_310_000_000), "1.31B");
        assert_eq!(format_tokens_cli(1_500_000_000_000), "1.50T");
    }

    #[test]
    fn explicit_root_is_not_promoted_to_parent_git_root() {
        assert_eq!(
            project_root(Some("/home/example/travail")),
            "/home/example/travail"
        );
    }

    #[test]
    fn explicit_root_expands_home_prefix() {
        let Some(home) = dirs::home_dir() else {
            return;
        };

        assert_eq!(
            normalize_explicit_root("~/travail"),
            home.join("travail").to_string_lossy().to_string()
        );
    }
}
