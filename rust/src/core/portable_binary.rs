/// Verbatim hook-binary override (#708), or `None` when unset/blank.
///
/// Users who sync agent settings (`~/.claude/settings.json`, …) across
/// machines with different usernames set `hook_binary` (config.toml) to an env-based form like
/// `$HOME/.local/bin/sobre`. Hook hosts execute commands through a shell,
/// which expands the variable at run time. The value is emitted verbatim by
/// every hook writer and accepted verbatim by `doctor`'s staleness check —
/// scoped to hook/agent artifacts only, never to launchd/systemd units
/// (which do not expand shell variables).
pub fn hook_binary_override() -> Option<String> {
    crate::core::config::Config::load()
        .hook_binary
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn resolve_portable_binary() -> String {
    let current = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());

    let which_cmd = if cfg!(windows) { "where" } else { "which" };
    let which_raw = std::process::Command::new(which_cmd)
        .arg("sobre")
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());

    choose_binary_path(current.as_deref(), which_raw.as_deref())
}

/// Decide which `sobre` path to bake into generated artifacts (autostart
/// plists, MCP server command, agent/shell hooks). The chosen path must be the
/// *exact build the user is running*, so every artifact agrees and a single
/// `init` can never leave the proxy on a different build than the MCP config.
///
/// Preference order:
/// 1. `current_exe` when absolute and not inside a transient Cargo build dir —
///    by construction the build currently in use.
/// 2. `which sobre` — the installed copy on PATH; used when the running binary
///    lives in `target/{debug,release}` (`cargo run -- setup`), where the
///    installed copy is the intended target.
/// 3. an absolute `current_exe` even from a build dir — still better than a bare
///    name (keeps generated hooks absolute, see #367).
/// 4. bare `sobre`.
fn choose_binary_path(current_exe: Option<&str>, which_raw: Option<&str>) -> String {
    let is_build_artifact = |p: &str| {
        p.contains("/target/debug/")
            || p.contains("/target/release/")
            || p.contains("\\target\\debug\\")
            || p.contains("\\target\\release\\")
    };

    // 1. Prefer the running binary when it lives in a stable install location.
    if let Some(exe) = current_exe
        && std::path::Path::new(exe).is_absolute()
        && !is_build_artifact(exe)
    {
        return sanitize_exe_path(exe);
    }

    // 2. Otherwise fall back to the installed copy on PATH.
    if let Some(raw) = which_raw {
        let path = pick_best_binary_line(raw);
        if std::path::Path::new(&path).is_absolute() {
            return sanitize_exe_path(&path);
        }
    }

    // 3. An absolute build-artifact path still beats a bare name.
    if let Some(exe) = current_exe
        && std::path::Path::new(exe).is_absolute()
    {
        return sanitize_exe_path(exe);
    }

    // 4. Last resort.
    "sobre".to_string()
}

/// On Windows, `where sobre` can return multiple lines. Pick the `.exe`
/// variant if available, otherwise the first line.
fn pick_best_binary_line(raw: &str) -> String {
    let lines: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.len() <= 1 {
        return lines.first().unwrap_or(&"sobre").to_string();
    }
    if cfg!(windows)
        && let Some(exe) = lines.iter().find(|l| {
            std::path::Path::new(*l)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        })
    {
        return exe.to_string();
    }
    lines[0].to_string()
}

fn sanitize_exe_path(path: &str) -> String {
    let cleaned = path.trim_end_matches(" (deleted)");
    if cfg!(windows) {
        super::pathutil::normalize_tool_path(cleaned)
    } else {
        cleaned.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_line_returns_as_is() {
        assert_eq!(pick_best_binary_line("/usr/bin/sobre"), "/usr/bin/sobre");
    }

    #[test]
    fn multiline_returns_first_line() {
        let raw = "/usr/bin/sobre\n/usr/local/bin/sobre";
        let result = pick_best_binary_line(raw);
        assert_eq!(result, "/usr/bin/sobre");
    }

    #[test]
    fn empty_returns_fallback() {
        assert_eq!(pick_best_binary_line(""), "sobre");
    }

    #[test]
    fn sanitize_removes_deleted_suffix() {
        assert_eq!(
            sanitize_exe_path("/usr/bin/sobre (deleted)"),
            "/usr/bin/sobre"
        );
    }

    #[test]
    fn whitespace_lines_are_filtered() {
        let raw = "  /usr/bin/sobre  \n  \n  /usr/local/bin/sobre  ";
        assert_eq!(pick_best_binary_line(raw), "/usr/bin/sobre");
    }

    #[cfg(windows)]
    #[test]
    fn sanitize_normalizes_msys_path_on_windows() {
        assert_eq!(
            sanitize_exe_path("/c/Users/ABC/.local/bin/sobre"),
            "C:/Users/ABC/.local/bin/sobre"
        );
    }

    #[cfg(windows)]
    #[test]
    fn sanitize_keeps_native_windows_path() {
        assert_eq!(
            sanitize_exe_path(r"C:\Users\ABC\sobre.exe"),
            "C:/Users/ABC/sobre.exe"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn sanitize_unix_path_unchanged() {
        assert_eq!(
            sanitize_exe_path("/usr/local/bin/sobre"),
            "/usr/local/bin/sobre"
        );
    }

    /// #708: the configured override is emitted verbatim — no absolutization,
    /// no rewriting — so synced settings keep `$HOME/...` forms; blank means
    /// unset so an empty value cannot wedge hook generation.
    #[test]
    fn hook_binary_override_is_verbatim_and_blank_is_none() {
        let iso = crate::core::data_dir::isolated_data_dir();
        let cfg = iso.path().join("config.toml");
        std::fs::write(&cfg, "hook_binary = \"$HOME/.local/bin/sobre\"\n").unwrap();
        assert_eq!(
            hook_binary_override().as_deref(),
            Some("$HOME/.local/bin/sobre")
        );
        std::fs::write(&cfg, "hook_binary = \"   \"\n").unwrap();
        assert_eq!(hook_binary_override(), None);
    }

    #[test]
    fn resolve_portable_binary_is_absolute() {
        // #367: generated hook commands must use an absolute binary path, never
        // a bare `sobre`, because agents run hooks under non-login shells
        // without the install dir on PATH. `which`/`current_exe()` both yield
        // an absolute path in any normal environment (incl. the test harness).
        let resolved = resolve_portable_binary();
        assert!(
            std::path::Path::new(&resolved).is_absolute(),
            "resolve_portable_binary must return an absolute path, got: {resolved}"
        );
    }

    #[test]
    fn nothing_resolvable_returns_bare_name() {
        // #2444: neither a usable running binary nor a PATH hit -> bare name.
        assert_eq!(choose_binary_path(None, None), "sobre");
        // A relative current_exe is not a usable absolute path.
        assert_eq!(choose_binary_path(Some("sobre"), None), "sobre");
    }

    // Unix absolute paths (the `/...` form is not absolute on Windows).
    #[cfg(not(windows))]
    mod unix_paths {
        use super::*;

        #[test]
        fn current_exe_beats_path_lookup() {
            // The core of #2444: the *running* build wins over a divergent PATH
            // entry (e.g. a stale Homebrew copy shadowing ~/.local/bin).
            let chosen = choose_binary_path(
                Some("/Users/dev/.local/bin/sobre"),
                Some("/opt/homebrew/bin/sobre"),
            );
            assert_eq!(chosen, "/Users/dev/.local/bin/sobre");
        }

        #[test]
        fn release_build_artifact_falls_back_to_path() {
            // `cargo run --release -- setup`: bake the installed copy, not the
            // transient build output.
            let chosen = choose_binary_path(
                Some("/work/sobre/rust/target/release/sobre"),
                Some("/Users/dev/.local/bin/sobre"),
            );
            assert_eq!(chosen, "/Users/dev/.local/bin/sobre");
        }

        #[test]
        fn debug_build_artifact_falls_back_to_path() {
            let chosen = choose_binary_path(
                Some("/work/sobre/rust/target/debug/deps/sobre-abc123"),
                Some("/usr/local/bin/sobre"),
            );
            assert_eq!(chosen, "/usr/local/bin/sobre");
        }

        #[test]
        fn build_artifact_without_path_keeps_absolute_current_exe() {
            // No installed copy on PATH -> an absolute build path still beats the
            // bare name, so generated hooks stay absolute (#367).
            let chosen = choose_binary_path(Some("/work/sobre/rust/target/release/sobre"), None);
            assert_eq!(chosen, "/work/sobre/rust/target/release/sobre");
        }

        #[test]
        fn relative_current_exe_falls_back_to_path() {
            let chosen = choose_binary_path(Some("sobre"), Some("/usr/bin/sobre"));
            assert_eq!(chosen, "/usr/bin/sobre");
        }

        #[test]
        fn path_lookup_multiline_picks_first() {
            let chosen = choose_binary_path(
                None,
                Some("/Users/dev/.local/bin/sobre\n/opt/homebrew/bin/sobre"),
            );
            assert_eq!(chosen, "/Users/dev/.local/bin/sobre");
        }
    }
}
