use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// Computes a composite hash from the project root path and any detected
/// project identity markers (git remote, manifest file, etc.).
///
/// This prevents hash collisions when different projects share the same
/// mount path (e.g. Docker volumes at `/workspace`).
pub(crate) fn hash_project_root(root: &str) -> String {
    // Normalize the path separator/casing first so the SAME directory always
    // produces the SAME hash regardless of which interface resolved it. On
    // Windows the MCP server reports forward slashes (`D:/repo`) while the CLI
    // reports backslashes (`D:\repo`); without normalization these hash to two
    // different project stores and facts written via one are invisible to the
    // other (issue #325). `normalize_tool_path` is a no-op for clean POSIX
    // paths, so non-Windows hashes are unaffected.
    let root = crate::core::pathutil::normalize_tool_path(root);
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);

    if let Some(identity) = project_identity(&root) {
        identity.hash(&mut hasher);
    }

    format!("{:016x}", hasher.finish())
}

/// Path-only hash (no identity markers), the baseline the composite hash
/// must differ from when an identity exists.
#[cfg(test)]
fn hash_path_only(root: &str) -> String {
    let root = crate::core::pathutil::normalize_tool_path(root);
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Extracts a stable project identity string from well-known config files.
///
/// Checks (in priority order):
///   1. `.git/config`   → remote "origin" URL
///   2. `Cargo.toml`    → `[package] name`
///   3. `package.json`  → `"name"` field
///   4. `pyproject.toml`→ `[project] name`
///   5. `go.mod`        → `module` path
///   6. `composer.json` → `"name"` field
///   7. `build.gradle`  / `build.gradle.kts` → existence as a marker
///   8. `*.sln`         → first `.sln` filename
///
/// Returns `None` when no identity marker is found, in which case
/// the hash falls back to path-only.
pub(crate) fn project_identity(root: &str) -> Option<String> {
    let root = Path::new(root);

    if let Some(url) = git_remote_url(root) {
        return Some(format!("git:{url}"));
    }
    if let Some(name) = cargo_package_name(root) {
        return Some(format!("cargo:{name}"));
    }
    if let Some(name) = npm_package_name(root) {
        return Some(format!("npm:{name}"));
    }
    if let Some(name) = pyproject_name(root) {
        return Some(format!("python:{name}"));
    }
    if let Some(module) = go_module(root) {
        return Some(format!("go:{module}"));
    }
    if let Some(name) = composer_name(root) {
        return Some(format!("composer:{name}"));
    }
    if let Some(name) = gradle_project(root) {
        return Some(format!("gradle:{name}"));
    }
    if let Some(name) = dotnet_solution(root) {
        return Some(format!("dotnet:{name}"));
    }

    None
}

// ---------------------------------------------------------------------------
// Identity detectors
// ---------------------------------------------------------------------------

fn git_remote_url(root: &Path) -> Option<String> {
    let config = root.join(".git").join("config");
    let content = std::fs::read_to_string(config).ok()?;

    let mut in_origin = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed == r#"[remote "origin"]"#;
            continue;
        }
        if in_origin && let Some(url) = trimmed.strip_prefix("url") {
            let url = url.trim_start_matches([' ', '=']);
            let url = url.trim();
            if !url.is_empty() {
                return Some(normalize_git_url(url));
            }
        }
    }
    None
}

fn normalize_git_url(url: &str) -> String {
    let url = url.trim_end_matches(".git");
    let url = url
        .strip_prefix("git@")
        .map_or_else(|| url.to_string(), |s| s.replacen(':', "/", 1));
    url.to_lowercase()
}

fn cargo_package_name(root: &Path) -> Option<String> {
    extract_toml_value(&root.join("Cargo.toml"), "name", Some("[package]"))
}

fn npm_package_name(root: &Path) -> Option<String> {
    extract_json_string_field(&root.join("package.json"), "name")
}

fn pyproject_name(root: &Path) -> Option<String> {
    extract_toml_value(&root.join("pyproject.toml"), "name", Some("[project]"))
        .or_else(|| extract_toml_value(&root.join("pyproject.toml"), "name", Some("[tool.poetry]")))
}

fn go_module(root: &Path) -> Option<String> {
    let content = std::fs::read_to_string(root.join("go.mod")).ok()?;
    let first = content.lines().next()?;
    first.strip_prefix("module").map(|s| s.trim().to_string())
}

fn composer_name(root: &Path) -> Option<String> {
    extract_json_string_field(&root.join("composer.json"), "name")
}

fn gradle_project(root: &Path) -> Option<String> {
    let settings = root.join("settings.gradle");
    let settings_kts = root.join("settings.gradle.kts");

    let path = if settings.exists() {
        settings
    } else if settings_kts.exists() {
        settings_kts
    } else {
        return None;
    };

    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("rootProject.name") {
            let rest = rest.trim_start_matches([' ', '=']);
            let name = rest.trim().trim_matches(['\'', '"']);
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn dotnet_solution(root: &Path) -> Option<String> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        if let Some(ext) = entry.path().extension()
            && ext == "sln"
        {
            return entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(String::from);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// TOML / JSON helpers (lightweight, no extra deps)
// ---------------------------------------------------------------------------

fn extract_toml_value(path: &Path, key: &str, section: Option<&str>) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut in_section = section.is_none();
    let target_section = section.unwrap_or("");

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') {
            in_section = trimmed == target_section;
            continue;
        }

        if in_section && let Some(rest) = trimmed.strip_prefix(key) {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let val = rest.trim().trim_matches('"');
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

fn extract_json_string_field(path: &Path, field: &str) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let needle = format!("\"{field}\"");
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(&needle) {
            let rest = rest.trim_start_matches([' ', ':']);
            let val = rest.trim().trim_start_matches('"');
            if let Some(end) = val.find('"') {
                let name = &val[..end];
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn windows_slash_and_backslash_hash_identically() {
        // Issue #325: the MCP server reports forward slashes while the CLI
        // reports backslashes for the same Windows directory. Both must resolve
        // to the same project hash so the knowledge store is not split.
        assert_eq!(
            hash_project_root(r"D:\repos\oref-examples"),
            hash_project_root("D:/repos/oref-examples"),
        );
        assert_eq!(
            hash_path_only(r"D:\repos\oref-examples"),
            hash_path_only("D:/repos/oref-examples"),
        );
    }

    #[test]
    fn trailing_slash_does_not_split_hash() {
        assert_eq!(
            hash_project_root("/home/user/project/"),
            hash_project_root("/home/user/project"),
        );
    }

    #[test]
    fn composite_differs_when_identity_present() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();

        let old = hash_path_only(root);
        let no_identity = hash_project_root(root);
        assert_eq!(old, no_identity, "without identity, hashes must match");

        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(
            dir.path().join(".git").join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:user/my-repo.git\n",
        )
        .unwrap();

        let with_identity = hash_project_root(root);
        assert_ne!(old, with_identity, "identity must change hash");
    }

    #[test]
    fn docker_collision_avoided() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();

        let shared_path = "/workspace";

        fs::create_dir_all(dir_a.path().join(".git")).unwrap();
        fs::write(
            dir_a.path().join(".git").join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:user/repo-a.git\n",
        )
        .unwrap();

        fs::create_dir_all(dir_b.path().join(".git")).unwrap();
        fs::write(
            dir_b.path().join(".git").join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:user/repo-b.git\n",
        )
        .unwrap();

        let hash_a = {
            let mut hasher = DefaultHasher::new();
            shared_path.hash(&mut hasher);
            let id = project_identity(dir_a.path().to_str().unwrap()).unwrap();
            id.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };
        let hash_b = {
            let mut hasher = DefaultHasher::new();
            shared_path.hash(&mut hasher);
            let id = project_identity(dir_b.path().to_str().unwrap()).unwrap();
            id.hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        };

        assert_ne!(
            hash_a, hash_b,
            "different repos at same path must produce different hashes"
        );
    }

    #[test]
    fn git_url_normalization() {
        assert_eq!(
            normalize_git_url("git@github.com:User/Repo.git"),
            "github.com/user/repo"
        );
        assert_eq!(
            normalize_git_url("https://github.com/User/Repo.git"),
            "https://github.com/user/repo"
        );
        assert_eq!(
            normalize_git_url("git@gitlab.com:org/sub/project.git"),
            "gitlab.com/org/sub/project"
        );
    }

    #[test]
    fn identity_from_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let id = project_identity(dir.path().to_str().unwrap());
        assert_eq!(id, Some("cargo:my-crate".into()));
    }

    #[test]
    fn identity_from_package_json() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("package.json"),
            "{\n  \"name\": \"@scope/my-app\",\n  \"version\": \"1.0.0\"\n}\n",
        )
        .unwrap();

        let id = project_identity(dir.path().to_str().unwrap());
        assert_eq!(id, Some("npm:@scope/my-app".into()));
    }

    #[test]
    fn identity_from_pyproject() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"my-python-lib\"\nversion = \"2.0\"\n",
        )
        .unwrap();

        let id = project_identity(dir.path().to_str().unwrap());
        assert_eq!(id, Some("python:my-python-lib".into()));
    }

    #[test]
    fn identity_from_poetry_pyproject() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.poetry]\nname = \"poetry-app\"\nversion = \"1.0\"\n",
        )
        .unwrap();

        let id = project_identity(dir.path().to_str().unwrap());
        assert_eq!(id, Some("python:poetry-app".into()));
    }

    #[test]
    fn identity_from_go_mod() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("go.mod"),
            "module github.com/user/myservice\n\ngo 1.21\n",
        )
        .unwrap();

        let id = project_identity(dir.path().to_str().unwrap());
        assert_eq!(id, Some("go:github.com/user/myservice".into()));
    }

    #[test]
    fn identity_from_composer() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("composer.json"),
            "{\n  \"name\": \"vendor/my-php-lib\"\n}\n",
        )
        .unwrap();

        let id = project_identity(dir.path().to_str().unwrap());
        assert_eq!(id, Some("composer:vendor/my-php-lib".into()));
    }

    #[test]
    fn identity_from_gradle() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("settings.gradle"),
            "rootProject.name = 'my-java-app'\n",
        )
        .unwrap();

        let id = project_identity(dir.path().to_str().unwrap());
        assert_eq!(id, Some("gradle:my-java-app".into()));
    }

    #[test]
    fn identity_from_dotnet_sln() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("MyApp.sln"), "").unwrap();

        let id = project_identity(dir.path().to_str().unwrap());
        assert_eq!(id, Some("dotnet:MyApp".into()));
    }

    #[test]
    fn identity_git_takes_priority_over_cargo() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::write(
            dir.path().join(".git").join("config"),
            "[remote \"origin\"]\n\turl = git@github.com:user/repo.git\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\n",
        )
        .unwrap();

        let id = project_identity(dir.path().to_str().unwrap());
        assert_eq!(id, Some("git:github.com/user/repo".into()));
    }

    #[test]
    fn no_identity_for_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let id = project_identity(dir.path().to_str().unwrap());
        assert!(id.is_none());
    }
}
