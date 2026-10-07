use std::path::Path;

// ── Shared types moved here from tools/ to break reverse-dependency ──

/// Context Reduction Protocol mode controlling output verbosity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrpMode {
    Off,
    Compact,
    Tdd,
}

impl CrpMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "off" => Some(Self::Off),
            "compact" => Some(Self::Compact),
            "tdd" => Some(Self::Tdd),
            _ => None,
        }
    }
}

/// Finds the outermost project root by walking up from `file_path`.
/// For monorepos with nested `.git` dirs (e.g. `mono/backend/.git` + `mono/frontend/.git`),
/// returns the outermost ancestor containing `.git`, a workspace marker, or a known
/// monorepo config file — so the whole monorepo is treated as one project.
pub fn detect_project_root(file_path: &str) -> Option<String> {
    let start = Path::new(file_path);
    let mut dir = if start.is_dir() {
        start
    } else {
        start.parent()?
    };
    let mut best: Option<String> = None;

    loop {
        if is_project_root_marker(dir) {
            best = Some(dir.to_string_lossy().to_string());
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent,
            _ => break,
        }
    }
    best
}

/// Checks if a directory looks like a project root (has `.git`, workspace config, etc.).
fn is_project_root_marker(dir: &Path) -> bool {
    const MARKERS: &[&str] = &[
        ".git",
        "Cargo.toml",
        "package.json",
        "go.work",
        "pnpm-workspace.yaml",
        "lerna.json",
        "nx.json",
        "turbo.json",
        ".projectile",
        "pyproject.toml",
        "setup.py",
        "Makefile",
        "CMakeLists.txt",
        "BUILD.bazel",
    ];
    MARKERS.iter().any(|m| dir.join(m).exists())
}

/// Returns the project root for `file_path`, falling back to cwd if none found.
/// Logs a warning when the fallback is a broad directory (home, root).
pub fn detect_project_root_or_cwd(file_path: &str) -> String {
    if let Some(root) = detect_project_root(file_path) {
        return root;
    }

    let fallback = {
        let p = Path::new(file_path);
        if p.exists() {
            if p.is_dir() {
                file_path.to_string()
            } else {
                p.parent().map_or_else(
                    || file_path.to_string(),
                    |pp| pp.to_string_lossy().to_string(),
                )
            }
        } else {
            std::env::current_dir()
                .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string())
        }
    };

    if is_broad_directory(&fallback) {
        use std::sync::Once;
        static WARN_ONCE: Once = Once::new();
        WARN_ONCE.call_once(|| {
            tracing::warn!(
                "[protocol: no project detected — current directory is {fallback} which is not a project root.\n  \
                 To fix: run from inside a project (with .git, Cargo.toml, package.json, etc.)]"
            );
        });
    }

    fallback
}

fn is_broad_directory(path: &str) -> bool {
    if path == "/" || path == "\\" || path == "." {
        return true;
    }
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path == home_str.as_ref() || path == format!("{home_str}/") {
            return true;
        }
    }
    false
}

/// Returns the file name component of a path for compact display.
/// Normalize a path for display by converting Windows backslashes to forward
/// slashes. Forward slashes are valid path separators on Windows, and unlike
/// backslashes they are never misinterpreted as escape sequences by the JSON,
/// markdown, or terminal layers of MCP clients — which corrupted Windows paths
/// in tool output (e.g. `C:\Users\…` rendered as `CUsers…`). See issue #324.
pub fn display_path(path: &str) -> String {
    path.replace('\\', "/")
}

pub fn shorten_path(path: &str) -> String {
    let normalized = display_path(path);
    let p = Path::new(&normalized);
    if let Some(name) = p.file_name() {
        return name.to_string_lossy().to_string();
    }
    normalized
}

/// Returns a path relative to `root` for disambiguated display, always with
/// forward slashes. Falls back to the basename if stripping fails.
///
/// Relativization is done on slash-normalized strings so it works regardless of
/// the separator style the client sent (Windows backslashes, mixed separators).
/// A component boundary is required so that root `a/b` never matches `a/bc`.
pub fn shorten_path_relative(path: &str, root: &str) -> String {
    let norm_path = display_path(path);
    let norm_root = display_path(root);
    let norm_root = norm_root.strip_suffix('/').unwrap_or(&norm_root);
    if let Some(rest) = norm_path.strip_prefix(norm_root)
        && let Some(rel) = rest.strip_prefix('/')
        && !rel.is_empty()
    {
        return rel.to_string();
    }
    shorten_path(&norm_path)
}

/// Removes a single trailing savings footer line, if present.
///
/// The compression funnel appends at most one footer as the final line — either
/// the box-drawing form (`─── 4,200 → 840 tok (↓80%) ───`) or the verbatim
/// truncation form (`[lean-ctx: 4200→840 tok, verbatim truncated]`). The
/// `/v1/compress` contract surfaces savings in a structured `stats` field, so
/// message bodies must stay footer-free and byte-stable for prompt caching
/// (#498). This strips that trailing line regardless of the ambient
/// `savings_footer` setting; content without a footer is returned untouched.
pub fn strip_trailing_savings_footer(output: &str) -> &str {
    let body = output.trim_end_matches('\n');
    let (head, last_line) = match body.rfind('\n') {
        Some(nl) => (&body[..nl], &body[nl + 1..]),
        None => ("", body),
    };
    if is_savings_footer_line(last_line) {
        head
    } else {
        output
    }
}

fn is_savings_footer_line(line: &str) -> bool {
    let l = line.trim();
    (l.starts_with("\u{2500}\u{2500}\u{2500} ") && l.ends_with(" \u{2500}\u{2500}\u{2500}"))
        || (l.starts_with("[lean-ctx: ") && l.ends_with(']'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_trailing_savings_footer_handles_both_styles() {
        // Box-drawing footer.
        let boxed = "body line one\nbody line two\n\u{2500}\u{2500}\u{2500} 4,200 \u{2192} 840 tok (\u{2193}80%) \u{2500}\u{2500}\u{2500}";
        assert_eq!(
            strip_trailing_savings_footer(boxed),
            "body line one\nbody line two"
        );
        // Verbatim-truncation footer.
        let verbatim = "out\n[lean-ctx: 4200\u{2192}840 tok, verbatim truncated]";
        assert_eq!(strip_trailing_savings_footer(verbatim), "out");
        // No footer → untouched (including trailing newline).
        assert_eq!(
            strip_trailing_savings_footer("plain body\n"),
            "plain body\n"
        );
        // A footer-only string collapses to empty.
        assert_eq!(
            strip_trailing_savings_footer("[lean-ctx: 10\u{2192}5 tok, verbatim truncated]"),
            ""
        );
        // A body line that merely mentions the marker mid-text is preserved.
        assert_eq!(
            strip_trailing_savings_footer("see [lean-ctx: docs] for details"),
            "see [lean-ctx: docs] for details"
        );
    }

    #[test]
    fn display_path_normalizes_windows_separators() {
        // Issue #324: backslashes were dropped/escaped by client render layers.
        assert_eq!(
            display_path(r"C:\Users\zir\AppData\Local\Temp\win-build-log.txt"),
            "C:/Users/zir/AppData/Local/Temp/win-build-log.txt"
        );
        assert_eq!(display_path("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn shorten_path_basename_for_windows_abs_path() {
        assert_eq!(
            shorten_path(r"D:\Temp\win-build-raw.log"),
            "win-build-raw.log"
        );
        assert_eq!(shorten_path("a/b/c.txt"), "c.txt");
    }

    #[test]
    fn shorten_path_relative_handles_windows_separators() {
        // Relative display keeps forward slashes regardless of input style.
        assert_eq!(
            shorten_path_relative(r"C:\proj\src\app\main.rs", r"C:\proj"),
            "src/app/main.rs"
        );
        // Mixed separators between path and root still relativize.
        assert_eq!(
            shorten_path_relative(r"C:\proj\src\main.rs", "C:/proj/"),
            "src/main.rs"
        );
        // A non-prefix abs path falls back to a clean basename, never a
        // separator-stripped blob like "CUserszir…".
        assert_eq!(
            shorten_path_relative(r"C:\Users\zir\Temp\build.log", r"D:\proj"),
            "build.log"
        );
    }

    #[test]
    fn shorten_path_relative_requires_component_boundary() {
        // Root "a/b" must not match sibling "a/bc".
        assert_eq!(shorten_path_relative("a/bc/d.rs", "a/b"), "d.rs");
        assert_eq!(shorten_path_relative("a/b/d.rs", "a/b"), "d.rs");
    }

    #[test]
    fn is_project_root_marker_detects_git() {
        let tmp = std::env::temp_dir().join("lean-ctx-test-root-marker");
        let _ = std::fs::create_dir_all(&tmp);
        let git_dir = tmp.join(".git");
        let _ = std::fs::create_dir_all(&git_dir);
        assert!(is_project_root_marker(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn is_project_root_marker_detects_cargo_toml() {
        let tmp = std::env::temp_dir().join("lean-ctx-test-cargo-marker");
        let _ = std::fs::create_dir_all(&tmp);
        let _ = std::fs::write(tmp.join("Cargo.toml"), "[package]");
        assert!(is_project_root_marker(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_project_root_finds_outermost() {
        let base = std::env::temp_dir().join("lean-ctx-test-monorepo");
        let inner = base.join("packages").join("app");
        let _ = std::fs::create_dir_all(&inner);
        let _ = std::fs::create_dir_all(base.join(".git"));
        let _ = std::fs::create_dir_all(inner.join(".git"));

        let test_file = inner.join("main.rs");
        let _ = std::fs::write(&test_file, "fn main() {}");

        let root = detect_project_root(test_file.to_str().unwrap());
        assert!(root.is_some(), "should find a project root for nested .git");
        let root_path = std::path::PathBuf::from(root.unwrap());
        assert_eq!(
            crate::core::pathutil::safe_canonicalize(&root_path).ok(),
            crate::core::pathutil::safe_canonicalize(&base).ok(),
            "should return outermost .git, not inner"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
