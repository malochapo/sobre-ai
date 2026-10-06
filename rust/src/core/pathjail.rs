use std::path::{Path, PathBuf};

use crate::core::error::PathJailError;

/// `allow_paths` / `read_only_roots` come from `config.toml`, where no shell
/// ever runs — users writing `"$HOME/code"` or `"~/code"` got a literal,
/// never-matching prefix and concluded the whole option was broken (GH #392).
/// Only the home directory is expanded (`~`, `$HOME`, `${HOME}`): any other
/// `$VAR` would make the jail depend on the environment of whichever process
/// reads the file, so it stays literal.
pub fn expand_user_path(raw: &str) -> PathBuf {
    let rest = if matches!(raw, "~" | "$HOME" | "${HOME}") {
        Some("")
    } else {
        ["~/", "$HOME/", "${HOME}/"]
            .iter()
            .find_map(|prefix| raw.strip_prefix(prefix))
    };
    match (rest, dirs::home_dir()) {
        (Some(""), Some(home)) => home,
        (Some(r), Some(home)) => home.join(r),
        _ => PathBuf::from(raw),
    }
}

pub fn allow_paths_from_env_and_config() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let cfg = crate::core::config::Config::load();

    // The allow-list defines the jail boundary, so it must be canonicalized the
    // same (security, symlink-resolving) way as the candidate it is compared
    // against — otherwise a guarded (lexical) root vs a resolved candidate would
    // break `is_under_prefix`. These entries are data_dir / IDE-config dirs /
    // user-configured paths, virtually never under ~/Documents.
    //
    // This is also lean-ctx's own state dir (sessions, knowledge, …) — always
    // readable even while foreign editor dirs stay jailed.
    if let Ok(data_dir) = crate::core::data_dir::lean_ctx_data_dir() {
        out.push(canonicalize_secure(&data_dir));
    }

    for p in &cfg.allow_paths {
        out.push(canonicalize_secure(&expand_user_path(p)));
    }

    for path in crate::core::runtime_flags::allow_paths() {
        out.push(canonicalize_secure(&path));
    }
    // Read-only roots are *readable* (the whole point is read access to sibling
    // repos); writes into them are denied separately by `enforce_writable`
    // (#475). Add them to the read allow-list so reads resolve, exactly like
    // `extra_roots`, without granting write access.
    out.extend(canonicalized_roots(&cfg.read_only_roots));

    out
}

/// Canonicalize a set of config-supplied root entries, expanding `~`/`$VAR`
/// first. Shared by the read allow-list and the read-only-roots collector so
/// both tiers parse roots identically.
fn canonicalized_roots(config_entries: &[String]) -> Vec<PathBuf> {
    config_entries
        .iter()
        .map(|p| canonicalize_secure(&expand_user_path(p)))
        .collect()
}

/// A read-only root is a sibling subtree the agent may **read** but never
/// **write** — e.g. a reference repo mounted next to the project. Empty by
/// default, so [`is_read_only_path`]/[`enforce_writable`] are zero-cost no-ops
/// for everyone who hasn't opted in (#475).
pub fn read_only_roots_from_env_and_config() -> Vec<PathBuf> {
    let cfg = crate::core::config::Config::load();
    let mut roots = canonicalized_roots(&cfg.read_only_roots);
    // #899: session-scoped roots auto-detected from language caches. Consulted
    // by both the read allow-list (jail) and `is_read_only_path` (write-deny),
    // so a cache root is readable but never writable — exactly like a configured
    // read-only root, minus the config-file edit.
    roots.extend(session_read_only_roots());
    roots
}

static SESSION_READ_ONLY_ROOTS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> =
    std::sync::OnceLock::new();

fn session_read_only_roots_cell() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    SESSION_READ_ONLY_ROOTS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// The session-scoped read-only roots auto-registered this process (#899).
pub fn session_read_only_roots() -> Vec<PathBuf> {
    session_read_only_roots_cell()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Register a session-scoped read-only root (an auto-detected language cache).
/// Returns `true` when newly added. Idempotent on the canonicalized path.
///
/// ponytail: process-global set, not per-session — language caches
/// (`~/go/pkg/mod`, `~/.cargo/registry`, …) are machine-global read-only dirs,
/// so sharing read access across sessions grants nothing a session couldn't
/// already get by reading them; per-session isolation would be plumbing for no
/// security gain. Upgrade to a session-keyed map only if writable roots ever go
/// down this path.
pub fn register_session_read_only_root(root: &Path) -> bool {
    let canon = canonicalize_secure(root);
    let mut guard = match session_read_only_roots_cell().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.iter().any(|r| r == &canon) {
        return false;
    }
    guard.push(canon);
    true
}

/// A single active relaxation of the path jail. Each one widens or disables what
/// tools can reach beyond the project root, so it is surfaced loudly (GH security
/// audit, finding 3): the MCP/HTTP server inherits its process env from the
/// IDE/launchd, so `allow_paths` / `extra_roots` / `allow_ide_config_dirs`
/// (or `path_jail = false`) silently loosens the
/// boundary with no in-band signal otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JailRelaxation {
    /// The knob that activated it (env var name, config key, or build feature).
    pub source: &'static str,
    /// Human-readable effect of the relaxation.
    pub detail: &'static str,
}

/// Collect every currently-active path-jail relaxation. An empty result means
/// the jail is fully in force. This is the single source of truth shared by the
/// startup warning ([`warn_if_relaxed`]) and `lean-ctx doctor`.
#[must_use]
pub fn active_relaxations() -> Vec<JailRelaxation> {
    let mut out = Vec::new();

    if cfg!(feature = "no-jail") {
        out.push(JailRelaxation {
            source: "no-jail (build feature)",
            detail: "path jail compiled out — every tool path is allowed",
        });
    }

    if crate::core::config::Config::load().jail_open() {
        out.push(JailRelaxation {
            source: "allow_paths = [\"/\"] (config.toml)",
            detail: "path jail disabled — every tool path is allowed",
        });
    }

    if crate::core::runtime_flags::allow_path_enabled() {
        out.push(JailRelaxation {
            source: "multi-root workspace",
            detail: "child projects of the workspace root are allowed alongside it",
        });
    }

    out
}

/// Emit a loud `tracing::warn!` for every active path-jail relaxation. Called
/// once at MCP/HTTP server startup so a trusted-but-loosening env/config leaves
/// an in-band audit signal instead of silently defeating the jail (finding 3).
pub fn warn_if_relaxed() {
    for relaxation in active_relaxations() {
        tracing::warn!(
            "[SECURITY] path jail relaxed via {}: {} — intended for trusted local use only",
            relaxation.source,
            relaxation.detail
        );
    }
}

/// True when `candidate` resolves to a location inside a configured read-only
/// root. The candidate's nearest existing ancestor is canonicalized (so a
/// not-yet-existing file inherits the read-only status of the directory it
/// would be created in — closing the "create a new file in a read-only repo"
/// hole) and matched against the (symlink-resolved) read-only roots.
///
/// A `false` return is only authoritative when the roots list is empty or the
/// path provably sits outside every root; an unresolvable candidate (no
/// existing ancestor) is treated as *not* read-only here and is rejected later
/// by the ordinary write/jail error, never silently written.
pub fn is_read_only_path(candidate: &Path) -> bool {
    let roots = read_only_roots_from_env_and_config();
    if roots.is_empty() {
        return false;
    }

    // Compare the canonicalized nearest-existing-ancestor (resolves symlinks so
    // a symlink *into* a read-only root can't launder a write past the prefix
    // check), reconstructing the full path for the comparison.
    let base = match canonicalize_existing_ancestor(candidate) {
        Some((base, remainder)) => {
            let mut p = base;
            for part in remainder.iter().rev() {
                p.push(part);
            }
            p
        }
        None => canonicalize_or_self(candidate),
    };

    roots.iter().any(|r| is_under_prefix(&base, r))
}

/// Default-deny write guard for the read-only tier (#475): returns an error if
/// `candidate` is inside a configured read-only root, `Ok(())` otherwise.
///
/// This is the single read-only-aware choke point. Every filesystem write that
/// can target a caller-supplied path routes through it (the atomic writers in
/// `ctx_edit`/`edit_apply`, the handoff/session export bundle writers, the
/// in-place memory-compaction writer, and the refactor IDE pre-write gate), so
/// a "read-only" root cannot be written through any tool. Reads are unaffected.
pub fn enforce_writable(candidate: &Path) -> Result<(), String> {
    if is_read_only_path(candidate) {
        return Err(format!(
            "path is inside a read-only root — writes are denied (read_only_roots): {}",
            candidate.display()
        ));
    }
    Ok(())
}

fn is_under_prefix(path: &Path, prefix: &Path) -> bool {
    path.starts_with(prefix)
}

/// Heuristic canonicalize — honours the #356 TCC guard. Used by the
/// jail-disabled bypass and by external callers (session/startup/server roots)
/// that must not pop a privacy prompt on their own initiative.
pub fn canonicalize_or_self(path: &Path) -> PathBuf {
    super::pathutil::safe_canonicalize_bounded(path, 2000)
}

/// SECURITY canonicalize for the jail boundary itself (roots + candidate +
/// escape re-check). Deliberately bypasses the #356 TCC guard: the jail must
/// keep resolving symlinks to detect escapes, and it only ever runs on a path
/// the client explicitly asked to access, where a one-time prompt is legitimate.
fn canonicalize_secure(path: &Path) -> PathBuf {
    super::pathutil::canonicalize_secure_bounded(path, 2000)
}

fn canonicalize_existing_ancestor(path: &Path) -> Option<(PathBuf, Vec<std::ffi::OsString>)> {
    let mut cur = path.to_path_buf();
    let mut remainder: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if cur.exists() {
            return Some((canonicalize_secure(&cur), remainder));
        }
        let name = cur.file_name()?.to_os_string();
        remainder.push(name);
        if !cur.pop() {
            return None;
        }
    }
}

pub fn jail_path(candidate: &Path, jail_root: &Path) -> Result<PathBuf, PathJailError> {
    jail_path_with_roots(candidate, jail_root, &[])
}

/// Known language-cache markers (#899): (path substring, human label, config
/// example). Single source of truth shared by [`detected_cache_hint`] (the
/// jail-error suggestion) and [`detect_language_cache_root`] (session
/// auto-registration), so the two never drift.
const LANGUAGE_CACHE_PATTERNS: &[(&str, &str, &str)] = &[
    ("/go/pkg/mod/", "Go module cache", "~/go/pkg/mod"),
    (
        "/.cargo/registry/",
        "Rust crate registry",
        "~/.cargo/registry",
    ),
    (
        "/site-packages/",
        "Python site-packages",
        "<venv>/lib/pythonX.Y/site-packages",
    ),
    ("/node_modules/", "Node modules", "<project>/node_modules"),
    (
        "/.m2/repository/",
        "Maven local repository",
        "~/.m2/repository",
    ),
    ("/.gradle/caches/", "Gradle cache", "~/.gradle/caches"),
    (
        "/.nuget/packages/",
        "NuGet package cache",
        "~/.nuget/packages",
    ),
];

/// Detect well-known language cache paths and return a targeted hint. Used for
/// jail callers that don't auto-register (e.g. batch reads); the single-path
/// ctx_read flow instead auto-registers via [`detect_language_cache_root`].
fn detected_cache_hint(candidate: &std::path::Path) -> Option<String> {
    let s = candidate.to_string_lossy();
    for &(pattern, name, example) in LANGUAGE_CACHE_PATTERNS {
        if s.contains(pattern) {
            return Some(format!(
                ". Detected {name} — add read_only_roots = [\"{example}\"] to \
                 ~/.config/lean-ctx/config.toml for cached, compressed reads without write access"
            ));
        }
    }
    None
}

/// If `candidate` sits inside a known language cache, return `(label, root)`
/// where `root` is the path truncated at the end of the marker directory (no
/// trailing slash). resolve_path uses this to auto-register a session read-only
/// root so the retry resolves without a config edit or a subprocess (#899).
pub fn detect_language_cache_root(candidate: &Path) -> Option<(&'static str, PathBuf)> {
    let s = candidate.to_string_lossy().replace('\\', "/");
    for &(marker, label, _) in LANGUAGE_CACHE_PATTERNS {
        if let Some(idx) = s.find(marker) {
            let end = idx + marker.len() - 1; // keep the marker dir, drop trailing '/'
            return Some((label, PathBuf::from(&s[..end])));
        }
    }
    None
}

/// Like [`jail_path`], but also accepts paths under any of `extra_roots`.
///
/// `extra_roots` are session-scoped trusted roots (MCP `roots/list` and config
/// `extra_roots`, surfaced via `session.extra_roots`) — e.g. sibling git
/// worktrees the agent legitimately spans. They widen the allow-list for *this
/// call only*, so an explicit `path` under a worktree resolves instead of
/// failing with "path escapes project root", without loosening the global jail
/// (#403). `path_jail = false` still bypasses entirely and an empty slice is
/// byte-for-byte identical to the old single-root behaviour.
pub fn jail_path_with_roots(
    candidate: &Path,
    jail_root: &Path,
    extra_roots: &[String],
) -> Result<PathBuf, PathJailError> {
    if candidate.to_string_lossy().as_bytes().contains(&0) {
        return Err(PathJailError::NullByte);
    }

    #[cfg(feature = "no-jail")]
    {
        let _ = (jail_root, extra_roots);
        return Ok(canonicalize_or_self(candidate));
    }

    #[allow(unreachable_code)]
    {
        let cfg = crate::core::config::Config::load();
        if cfg.jail_open() {
            return Ok(canonicalize_or_self(candidate));
        }

        let root = canonicalize_secure(jail_root);

        // Resolve relative candidates against the (absolute) jail root — never the process
        // CWD. The daemon's CWD is not the project, so CWD-relative resolution made
        // graph-relative paths (e.g. auto-preload candidates like `rust/src/core/foo.rs`)
        // spuriously fail with "no existing ancestor". Absolute candidates are unchanged.
        let resolved: PathBuf;
        let candidate: &Path = if candidate.is_absolute() {
            candidate
        } else {
            resolved = root.join(candidate);
            resolved.as_path()
        };

        let mut allow = allow_paths_from_env_and_config();
        // Session-scoped roots widen the allow-list for this call only.
        allow.extend(
            extra_roots
                .iter()
                .filter(|r| !r.is_empty())
                .map(|r| canonicalize_secure(Path::new(r))),
        );

        // #820: lean-ctx's own state dir (tee files, artifacts, tool-results)
        // must be readable even when outside the project root. ctx_shell
        // tells agents to read tee-file paths, so the jail must allow them.
        if let Ok(state) = crate::core::paths::state_dir() {
            allow.push(canonicalize_secure(&state));
        }
        // Read-only roots are also allowed for reads (they only block writes
        // via enforce_writable, not reads via the jail).
        allow.extend(
            read_only_roots_from_env_and_config()
                .into_iter()
                .map(|p| canonicalize_secure(&p)),
        );

        let (base, remainder) = canonicalize_existing_ancestor(candidate).ok_or_else(|| {
            PathJailError::NoExistingAncestor {
                path: candidate.to_path_buf(),
            }
        })?;

        let allowed =
            is_under_prefix(&base, &root) || allow.iter().any(|p| is_under_prefix(&base, p));

        #[cfg(windows)]
        let allowed = allowed || is_under_prefix_windows(&base, &root);

        if !allowed {
            // Agents otherwise get a bare rejection and shell out to work around
            // the jail; name the config keys the same way the shell-allowlist
            // block message names its key (#540, #887).
            let mut hint = ". Fix (additive): add the directory to extra_roots or allow_paths in \
                 ~/.config/lean-ctx/config.toml — `lean-ctx doctor` shows the config in effect"
                .to_string();
            // The global config the runtime reads doesn't exist → on defaults, so
            // an `allow_paths` edit made to a config.toml elsewhere (a custom
            // config dir, or a sandboxed/container HOME) is never seen (#540).
            if let Some(missing) = crate::core::config::Config::missing_config_path() {
                hint.push_str(&format!(
                    ". ⚠ lean-ctx reads no config file at {} (running on defaults) — an \
                     allow_paths edit in a config.toml elsewhere is not read; \
                     `lean-ctx doctor` shows the path in effect",
                    missing.display()
                ));
            }
            if let Some(cache_hint) = detected_cache_hint(candidate) {
                hint.push_str(&cache_hint);
            }
            return Err(PathJailError::EscapesRoot {
                path: candidate.to_path_buf(),
                root,
                hint,
            });
        }

        #[cfg(windows)]
        reject_symlink_on_windows(candidate)?;

        let mut out = base;
        for part in remainder.iter().rev() {
            out.push(part);
        }

        // Re-validate after reconstruction: if the final path exists, canonicalize
        // and re-check to close TOCTOU window (symlink created between check and use).
        if out.exists() {
            let final_canon = canonicalize_secure(&out);
            let final_ok = is_under_prefix(&final_canon, &root)
                || allow.iter().any(|p| is_under_prefix(&final_canon, p));
            #[cfg(windows)]
            let final_ok = final_ok || is_under_prefix_windows(&final_canon, &root);
            if !final_ok {
                return Err(PathJailError::PostCanonicalizeEscape {
                    path: candidate.to_path_buf(),
                    resolved: final_canon,
                });
            }
        }

        Ok(out)
    }
}

#[cfg(windows)]
fn is_under_prefix_windows(path: &Path, prefix: &Path) -> bool {
    let path_str = normalize_windows_path(&path.to_string_lossy());
    let prefix_str = normalize_windows_path(&prefix.to_string_lossy());
    path_str.starts_with(&prefix_str)
}

#[cfg(windows)]
fn normalize_windows_path(s: &str) -> String {
    let stripped = super::pathutil::strip_verbatim_str(s).unwrap_or_else(|| s.to_string());
    stripped.to_lowercase().replace('/', "\\")
}

#[cfg(windows)]
fn reject_symlink_on_windows(path: &Path) -> Result<(), PathJailError> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        // Junctions and other reparse points redirect like symlinks but are
        // invisible to `is_symlink()` — reject them too (GL#442).
        if super::pathutil::is_symlink_or_reparse(&meta) {
            return Err(PathJailError::Symlink {
                path: path.to_path_buf(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn rejects_path_outside_root() {
        // Hermetic config (empty data dir => jail on) so a parallel test that
        // flips `path_jail` cannot leak into this enforcement check. The guard
        // holds the global test_env_lock, which also serializes against every
        // allow-path mutation (all of them go through that lock).
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(root.join("a.txt"), "ok").unwrap();
        std::fs::write(other.join("b.txt"), "no").unwrap();

        let ok = jail_path(&root.join("a.txt"), &root);
        assert!(ok.is_ok());

        let bad = jail_path(&other.join("b.txt"), &root);
        assert!(bad.is_err());
    }

    /// #475: a configured read-only root is readable but never writable. Reads
    /// resolve (the root joins the allow-list like an extra_root), while the
    /// single write choke point `enforce_writable` default-denies every write
    /// inside it — including a not-yet-existing file, which inherits the
    /// directory's read-only status. `isolated_data_dir` holds `test_env_lock`,
    /// serialising the `read_only_roots` config write against other tests.
    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn read_only_roots_deny_writes_but_allow_reads() {
        // `isolated_data_dir` already holds `test_env_lock` for its lifetime —
        // taking it again here would self-deadlock on a non-reentrant Mutex.
        let iso = crate::core::data_dir::isolated_data_dir();

        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        let refrepo = tmp.path().join("refrepo");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(refrepo.join("sub")).unwrap();
        std::fs::write(refrepo.join("lib.rs"), "pub fn x() {}\n").unwrap();

        // Canonicalize the configured root the same (symlink-resolving) way the
        // guard does, so macOS /var → /private/var can't defeat the prefix match.
        let ro_canon = canonicalize_secure(&refrepo);
        std::fs::write(
            iso.path().join("config.toml"),
            format!("read_only_roots = [{:?}]\n", ro_canon.to_string_lossy()),
        )
        .unwrap();

        let existing = refrepo.join("lib.rs");
        let new_file = refrepo.join("sub").join("new.rs");
        let proj_file = project.join("main.rs");

        // Capture every decision while the config is live.
        let read_existing = jail_path(&existing, &project);
        let deny_existing = enforce_writable(&existing);
        let deny_new = enforce_writable(&new_file);
        let allow_project = enforce_writable(&proj_file);
        let ro_existing = is_read_only_path(&existing);
        let ro_project = is_read_only_path(&proj_file);

        assert!(
            deny_existing.is_err(),
            "write to an existing file in a read-only root must be denied"
        );
        assert!(
            deny_new.is_err(),
            "creating a new file in a read-only root must be denied"
        );
        assert!(
            allow_project.is_ok(),
            "writes into the project root must stay allowed: {allow_project:?}"
        );
        assert!(
            read_existing.is_ok(),
            "reads inside a read-only root must resolve (read allow-list): {read_existing:?}"
        );
        assert!(ro_existing, "the file is inside the read-only root");
        assert!(!ro_project, "the project file is not read-only");
    }

    /// #406 regression: a long-lived process (the MCP server) must honor
    /// `path_jail = false` written to config after startup. The config cache is
    /// now keyed on content, so even an edit that preserves the file mtime takes
    /// effect — a path outside the jail root is accepted once the flag flips.
    /// (With the former mtime-only cache the stale `None` kept the jail on.)
    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn honors_path_jail_false_after_mtime_preserving_edit() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let cfg_path = crate::core::config::Config::path().unwrap();
        if let Some(parent) = cfg_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, "x").unwrap();

        // Warm the config cache with the jail on (no allow_paths key).
        std::fs::write(&cfg_path, "# jail on\n").unwrap();
        let mtime0 = std::fs::metadata(&cfg_path).unwrap().modified().unwrap();
        assert!(!crate::core::config::Config::load().jail_open());

        // Open the jail but restore the original mtime, so any mtime-only
        // cache would keep serving the stale jail-on value.
        std::fs::write(&cfg_path, "allow_paths = [\"/\"]\n").unwrap();
        filetime::set_file_mtime(&cfg_path, filetime::FileTime::from_system_time(mtime0)).unwrap();

        assert!(
            jail_path(&secret, &root).is_ok(),
            "allow_paths = [\"/\"] must take effect without a fresh process (#406)"
        );
    }

    #[test]
    fn allows_nonexistent_child_under_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), "ok").unwrap();

        let p = root.join("new").join("file.txt");
        let ok = jail_path(&p, &root).unwrap();
        assert!(ok.to_string_lossy().contains("file.txt"));
    }

    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn relative_candidate_resolves_against_root_not_cwd() {
        // Regression: in the daemon (CWD != project) a relative graph path like
        // `sub/file.rs` must resolve under the jail root, not the process CWD.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("file.rs"), "ok").unwrap();

        let jailed = jail_path(Path::new("sub/file.rs"), &root)
            .expect("relative candidate should resolve under the jail root");
        assert!(jailed.ends_with("sub/file.rs"));
        assert!(
            is_under_prefix(&canonicalize_or_self(&jailed), &canonicalize_or_self(&root)),
            "resolved path must live under the jail root: {jailed:?}"
        );
    }

    #[test]
    fn canonicalize_or_self_strips_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("project");
        std::fs::create_dir_all(&dir).unwrap();

        let result = canonicalize_or_self(&dir);
        let s = result.to_string_lossy();
        assert!(
            !s.starts_with(r"\\?\"),
            "canonicalize_or_self should strip verbatim prefix, got: {s}"
        );
    }

    #[test]
    fn jail_path_accepts_same_dir_different_format() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("file.rs"), "ok").unwrap();

        let result = jail_path(&root.join("file.rs"), &root);
        assert!(result.is_ok(), "same dir should be accepted: {result:?}");
    }

    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn error_message_contains_escape_info() {
        // isolated_data_dir holds the global test_env_lock, serializing this
        // against any parallel allow-path mutation.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("b.txt"), "no").unwrap();

        let err = jail_path(&other.join("b.txt"), &root).unwrap_err();
        assert!(
            err.to_string().contains("path escapes project root"),
            "error should mention escape: {err}"
        );
    }

    // GH #887: over MCP (meta hints gated off) the rejection was bare, so
    // agents shelled out to work around the jail instead of widening it via
    // config. The agent-visible error must name the sanctioned config keys.
    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn escape_error_names_config_keys_without_meta() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("b.txt"), "no").unwrap();

        let err = jail_path(&other.join("b.txt"), &root).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("extra_roots") && msg.contains("allow_paths"),
            "agent-visible escape error should name the config keys: {msg}"
        );
    }

    // GH #392: config entries like "$HOME/code" or "~/code" were taken
    // literally and never matched.
    #[test]
    fn expand_user_path_expands_home_only() {
        let home = dirs::home_dir().expect("home dir");

        assert_eq!(expand_user_path("~"), home);
        assert_eq!(expand_user_path("~/code"), home.join("code"));
        assert_eq!(expand_user_path("$HOME/code"), home.join("code"));
        assert_eq!(expand_user_path("${HOME}/code"), home.join("code"));
        // Absolute paths pass through untouched.
        assert_eq!(expand_user_path("/etc"), PathBuf::from("/etc"));
    }

    #[test]
    fn expand_user_path_leaves_other_vars_literal() {
        // Any variable but HOME would make the jail depend on the reader's env.
        crate::test_env::set_var("LEAN_CTX_TEST_SUB", "sub");
        assert_eq!(
            expand_user_path("$LEAN_CTX_TEST_SUB/code"),
            PathBuf::from("$LEAN_CTX_TEST_SUB/code")
        );
        crate::test_env::remove_var("LEAN_CTX_TEST_SUB");
    }

    // GH #392: `allow_paths = ["/"]` must grant access to any absolute path —
    // "/" is a prefix of everything.
    // Config-mutating tests here hold the process-global
    // `data_dir::test_env_lock()` (via `isolated_data_dir()`, which wraps it
    // and gives them a private config.toml) — NOT a module-local mutex, so no
    // other test can observe this test's `allow_paths = ["/"]` (#695).
    #[cfg(unix)]
    #[test]
    fn allow_path_root_slash_permits_everything() {
        let iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("b.txt"), "allowed").unwrap();

        std::fs::write(iso.path().join("config.toml"), "allow_paths = [\"/\"]\n").unwrap();
        let result = jail_path(&other.join("b.txt"), &root);

        assert!(result.is_ok(), "allow path '/' must permit all: {result:?}");
    }

    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn active_relaxations_empty_when_jail_intact() {
        let _iso = crate::core::data_dir::isolated_data_dir();
        assert!(
            active_relaxations().is_empty(),
            "an intact jail (clean config) must report no relaxations: {:?}",
            active_relaxations()
        );
    }

    #[test]
    fn allow_paths_config_permits_outside_root() {
        let iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("b.txt"), "allowed").unwrap();

        let canon = canonicalize_or_self(&other);
        std::fs::write(
            iso.path().join("config.toml"),
            format!("allow_paths = [{:?}]\n", canon.to_string_lossy()),
        )
        .unwrap();
        let result = jail_path(&other.join("b.txt"), &root);

        assert!(
            result.is_ok(),
            "allow_paths should permit access: {result:?}"
        );
    }

    #[cfg(all(unix, not(feature = "no-jail")))]
    #[test]
    fn rejects_symlink_escape_on_unix() {
        use std::os::unix::fs::symlink;

        // isolated_data_dir holds the global test_env_lock — no parallel test
        // can configure `allow_paths = ["/"]` and let this escape resolve.
        let _iso = crate::core::data_dir::isolated_data_dir();
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("secret.txt"), "no").unwrap();

        let link = root.join("link.txt");
        symlink(other.join("secret.txt"), &link).unwrap();

        let bad = jail_path(&link, &root);
        assert!(bad.is_err(), "symlink escape must be rejected: {bad:?}");
    }

    #[test]
    fn rejects_null_byte_in_path() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir_all(&root).unwrap();

        let bad_path = PathBuf::from("file\0.txt");
        let result = jail_path(&bad_path, &root);
        assert!(result.is_err(), "null byte in path must be rejected");
        assert!(
            result.unwrap_err().to_string().contains("null byte"),
            "error must mention null byte"
        );
    }

    /// #403 Bug 1: an explicit path under a session-scoped `extra_root` (e.g. a
    /// sibling git worktree from MCP `roots/list`) must resolve, while the same
    /// path is rejected without it — and a path under *no* root is rejected even
    /// when extra roots are present. Holds both env locks so neither a parallel
    /// `path_jail` flip nor an allow-path mutation can leak in.
    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn extra_roots_permit_paths_outside_jail() {
        let _iso = crate::core::data_dir::isolated_data_dir();

        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        let worktree = tmp.path().join("worktree");
        let elsewhere = tmp.path().join("elsewhere");
        for d in [&root, &worktree, &elsewhere] {
            std::fs::create_dir_all(d).unwrap();
        }
        let in_worktree = worktree.join("a.txt");
        std::fs::write(&in_worktree, "x").unwrap();
        let outside = elsewhere.join("b.txt");
        std::fs::write(&outside, "y").unwrap();

        // Parity: with no extra roots, the worktree path escapes the jail.
        assert!(jail_path(&in_worktree, &root).is_err());
        assert!(jail_path_with_roots(&in_worktree, &root, &[]).is_err());

        // The session-scoped extra root permits it — via the slice alone, with
        // nothing in env/config.
        let extra = vec![worktree.to_string_lossy().to_string()];
        assert!(
            jail_path_with_roots(&in_worktree, &root, &extra).is_ok(),
            "path under a session extra_root must resolve (#403)"
        );

        // A path under neither the jail nor any extra root is still rejected.
        assert!(
            jail_path_with_roots(&outside, &root, &extra).is_err(),
            "paths outside ALL roots must still be rejected"
        );

        // Empty entries are ignored (no accidental allow-all).
        assert!(jail_path_with_roots(&outside, &root, &[String::new()]).is_err());
    }

    /// #820: lean-ctx state dir (tee files) is implicitly allowed by the jail.
    #[test]
    fn state_dir_tee_files_pass_jail() {
        let _lock = crate::core::data_dir::test_env_lock();
        let state = crate::core::paths::state_dir().expect("state_dir must be available");
        let tee_path = state.join("tee").join("some_command_deadbeef.log");
        // Use a root that is clearly NOT the state dir's parent
        let fake_root = std::env::temp_dir().join("pathjail_test_820_root");
        std::fs::create_dir_all(&fake_root).ok();
        // The tee path is outside the fake root, but the state dir allowance
        // should make it pass (the state dir itself exists on disk).
        let result = jail_path_with_roots(&tee_path, &fake_root, &[]);
        // If state_dir exists on disk (it does in dev), the path should be allowed.
        // If the tee file itself doesn't exist, canonicalize_existing_ancestor
        // resolves to the state_dir (which does exist) + remainder.
        if state.exists() {
            assert!(
                result.is_ok(),
                "tee-file path under lean-ctx state dir must be auto-allowed: {result:?}"
            );
        }
        std::fs::remove_dir_all(&fake_root).ok();
    }

    #[test]
    fn detected_cache_hint_recognizes_go_cargo_python() {
        use std::path::Path;
        let go = detected_cache_hint(Path::new("/Users/x/go/pkg/mod/github.com/foo/bar/main.go"));
        assert!(go.is_some(), "Go module cache should be detected");
        assert!(go.unwrap().contains("Go module cache"));

        let cargo = detected_cache_hint(Path::new(
            "/home/x/.cargo/registry/src/crates.io/serde-1.0/lib.rs",
        ));
        assert!(cargo.is_some(), "Rust cargo registry should be detected");
        assert!(cargo.unwrap().contains("Rust crate registry"));

        let py = detected_cache_hint(Path::new(
            "/usr/lib/python3.12/site-packages/requests/api.py",
        ));
        assert!(py.is_some(), "Python site-packages should be detected");

        let normal = detected_cache_hint(Path::new("/home/x/projects/myapp/src/main.rs"));
        assert!(normal.is_none(), "Normal project path should not match");
    }

    #[test]
    fn detect_cache_root_extracts_marker_dir() {
        let cases = [
            (
                "/Users/x/go/pkg/mod/github.com/foo/bar@v1.2.3/baz.go",
                "Go module cache",
                "/Users/x/go/pkg/mod",
            ),
            (
                "/home/u/.cargo/registry/src/index-abc/serde-1.0/src/lib.rs",
                "Rust crate registry",
                "/home/u/.cargo/registry",
            ),
            (
                "/opt/venv/lib/python3.12/site-packages/requests/api.py",
                "Python site-packages",
                "/opt/venv/lib/python3.12/site-packages",
            ),
            (
                "/w/app/node_modules/react/index.js",
                "Node modules",
                "/w/app/node_modules",
            ),
        ];
        for (path, want_label, want_root) in cases {
            let (label, root) = detect_language_cache_root(Path::new(path))
                .unwrap_or_else(|| panic!("expected cache match for {path}"));
            assert_eq!(label, want_label, "label for {path}");
            assert_eq!(root, PathBuf::from(want_root), "root for {path}");
        }
        assert!(
            detect_language_cache_root(Path::new("/home/u/proj/src/main.rs")).is_none(),
            "a normal project path is not a cache"
        );
    }

    /// The core #899 guarantee: once a detected cache root is registered, a path
    /// under it *reads* (jail resolves) but never *writes* (enforce_writable
    /// denies), and registration is idempotent.
    #[cfg(not(feature = "no-jail"))]
    #[test]
    fn registered_cache_root_reads_allow_writes_deny() {
        let _iso = crate::core::data_dir::isolated_data_dir();

        let tmp = tempfile::tempdir().unwrap();
        // A fake Go module cache so detect_language_cache_root matches the path.
        let dep = tmp.path().join("go/pkg/mod/example.com/lib@v1");
        std::fs::create_dir_all(&dep).unwrap();
        let file = dep.join("lib.go");
        std::fs::write(&file, "package lib").unwrap();

        // A project jail that does NOT contain the cache.
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();

        // Before registration: the read escapes the jail.
        assert!(jail_path_with_roots(&file, &project, &[]).is_err());

        // Register the detected root; the second call is a no-op.
        let (_, root) = detect_language_cache_root(&file).expect("cache match");
        assert!(
            register_session_read_only_root(&root),
            "first register is new"
        );
        assert!(
            !register_session_read_only_root(&root),
            "re-register is a no-op"
        );

        // After: the read resolves, but writes are denied (read-only tier).
        assert!(
            jail_path_with_roots(&file, &project, &[]).is_ok(),
            "registered cache root must be readable"
        );
        assert!(is_read_only_path(&file), "cache file is read-only");
        assert!(
            enforce_writable(&file).is_err(),
            "writes into the cache root must be denied"
        );
    }
}
