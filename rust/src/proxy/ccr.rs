//! Content-addressed recovery (CCR) for the proxy's lossy rewrites.
//!
//! When the proxy prunes an old `tool_result` from conversation history, the
//! lossy stub used to say *"re-read the file"* — which is stale-unsafe by
//! construction: in an agent session files are edited or deleted between turns,
//! so a re-read returns the *current* bytes (or fails), not the historical
//! version the conversation actually showed. The model could then silently
//! reason about the wrong content.
//!
//! CCR fixes this by persisting the **verbatim original** to the shared,
//! content-addressed tee store (`{state}/tee/`, reused from the shell path) and
//! embedding a **retrieval handle** — the absolute path of that file — in the
//! stub. Retrieval is MCP-independent: the agent reads the path with its native
//! file read; no sobre tool has to be attached.
//!
//! ## Cache-safety
//! The handle is the file path, and the path is a pure function of the content
//! hash ([`crate::core::hasher::hash_short`]). For a fixed pruned message the
//! handle is therefore byte-identical on every later turn, so the provider
//! prompt-cache prefix is never invalidated. The on-disk *write* is best-effort
//! and never affects the returned handle — only retrievability degrades if the
//! write (or the 24h TTL cleanup) loses the file, so a stub can never become
//! non-deterministic based on filesystem state.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Originals smaller than this are not worth a tee file + handle; the caller
/// keeps its plain stub. Matches the spirit of the prune length thresholds.
pub(crate) const MIN_TEE_BYTES: usize = 512;

/// Throttle the O(dir) TTL cleanup so the prune hot path does at most one
/// directory scan per this interval (the write itself is content-addressed and
/// idempotent, so steady-state cost is a single `stat`).
const CLEANUP_INTERVAL_SECS: u64 = 600;

/// Length of the content hash in a proxy tee name ([`hash_short`]).
const TEE_HASH_LEN: usize = 16;
/// Length of the hash in a shell tee name (`shell::redact::save_tee`).
const SHELL_TEE_HASH_LEN: usize = 8;

/// Deterministic tee path for `content`:
/// `{state}/tee/proxy_{blake3(content)[..16]}.log`. Pure (no I/O) so a stub
/// embedding it stays byte-stable regardless of filesystem state. The `proxy_`
/// prefix keeps these apart from the shell tee files in the same store, which
/// one resolver ([`resolve_tee`]) serves.
fn tee_path(content: &str) -> Option<PathBuf> {
    let dir = crate::core::paths::state_dir().ok()?.join("tee");
    let hash = crate::core::hasher::hash_short(content);
    Some(dir.join(format!("proxy_{hash}.log")))
}

/// Run the shared 24h TTL cleanup at most once per [`CLEANUP_INTERVAL_SECS`].
fn maybe_cleanup(tee_dir: &Path) {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else {
        return;
    };
    let now = now.as_secs();
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) < CLEANUP_INTERVAL_SECS {
        return;
    }
    // Only one thread wins the slot; the rest skip until the next interval.
    if LAST
        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
        .is_ok()
    {
        crate::shell::cleanup_old_tee_logs(tee_dir);
    }
}

/// Persist `content` verbatim (best-effort, secret-redacted) to the
/// content-addressed tee store and return its retrieval handle (the absolute
/// path). Returns `None` only when `content` is below [`MIN_TEE_BYTES`] or the
/// state dir can't be resolved — never because the *write* failed, so the
/// returned handle is a pure function of the content and the embedding stub
/// stays deterministic. Re-persisting identical content is idempotent: same
/// content → same path → the existing file is left untouched.
pub(crate) fn persist(content: &str) -> Option<String> {
    if content.len() < MIN_TEE_BYTES {
        return None;
    }
    let path = tee_path(content)?;
    let handle = path.to_string_lossy().to_string();

    if !path.exists() {
        if let Some(dir) = path.parent()
            && std::fs::create_dir_all(dir).is_ok()
        {
            maybe_cleanup(dir);
        }
        // Same redaction the shell tee applies, so a recovered original can never
        // re-introduce a secret the live turn would also have masked.
        let masked = crate::core::redaction::redact_text(content);
        let (redacted, _) = crate::core::secret_detection::scan_and_redact(&masked);
        if std::fs::write(&path, redacted).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }
    Some(handle)
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Canonical `proxy_{16hex}.log` name for a `proxy_{16hex}` id, with or
/// without the `.log`, or `None`.
fn canonical_tee_name(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".log").unwrap_or(name);
    let hash = stem.strip_prefix("proxy_")?;
    is_hex(hash, TEE_HASH_LEN).then(|| format!("proxy_{hash}.log"))
}

/// True for a shell tee basename `<slug>_<8hex>.log` (`shell::redact::save_tee`):
/// ends in `.log`, the whole basename is safe (`[A-Za-z0-9_-]`), and the **last**
/// `_`-segment is exactly 8 hex. The slug itself may contain `_`, so the hash is
/// matched as the suffix — never the first segment (the documented parsing trap).
fn is_shell_tee_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".log") else {
        return false;
    };
    if !stem
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return false;
    }
    match stem.rsplit_once('_') {
        Some((slug, hash)) => !slug.is_empty() && is_hex(hash, SHELL_TEE_HASH_LEN),
        None => false,
    }
}

/// Resolve a retrieval `id` back to a file in the shared `{state}/tee/` store.
/// Accepts every handle form a stub or footer can carry, with a fixed precedence
/// so the forms can never collide:
///
/// 1. **Proxy form** — `proxy_<16hex>(.log)`: the proxy history-prune and live
///    compression stubs.
/// 2. **Shell-tee form** — `<slug>_<8hex>.log` (`save_tee`), so every compressed
///    shell command's already-teed verbatim output is surgically retrievable.
///
/// The 16-vs-8 hex length already disambiguates the two classes; the explicit
/// order documents intent. Security: only the *file name* is trusted — the path
/// is always rebuilt under `{state}/tee/`, so a crafted `id` can never escape the
/// store (no path traversal) and a non-tee id resolves to `None`.
pub(crate) fn resolve_tee(id: &str) -> Option<PathBuf> {
    let name = Path::new(id)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(id);
    let canon =
        canonical_tee_name(name).or_else(|| is_shell_tee_name(name).then(|| name.to_string()))?;
    let path = crate::core::paths::state_dir()
        .ok()?
        .join("tee")
        .join(canon);
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn big(seed: &str) -> String {
        format!("{seed}\n").repeat(40)
    }

    #[test]
    fn handle_is_content_addressed_and_deterministic() {
        let _lock = crate::core::data_dir::test_env_lock();
        let content = big("file body line");
        let a = persist(&content).expect("persisted");
        let b = persist(&content).expect("persisted again");
        assert_eq!(
            a, b,
            "same content must map to the same handle (cache-safe)"
        );
        assert!(a.contains("proxy_"), "handle is a proxy tee path: {a}");

        let other = persist(&big("different body")).expect("persisted");
        assert_ne!(a, other, "different content must get a different handle");
    }

    #[test]
    fn persisted_original_is_recoverable() {
        let _lock = crate::core::data_dir::test_env_lock();
        let content = big("recoverable verbatim line");
        let handle = persist(&content).expect("persisted");
        let on_disk = std::fs::read_to_string(&handle).expect("tee file readable");
        assert!(
            on_disk.contains("recoverable verbatim line"),
            "the verbatim original must be retrievable from the handle"
        );
    }

    #[test]
    fn resolve_tee_accepts_every_stub_form() {
        let _lock = crate::core::data_dir::test_env_lock();
        let content = big("resolvable tee body");
        let handle = persist(&content).expect("persisted");
        let hash = crate::core::hasher::hash_short(&content);

        // Full path, bare file name and proxy_<hash> all resolve to the same
        // on-disk file — whatever the agent copied out of the stub.
        for form in [
            handle.clone(),
            format!("proxy_{hash}.log"),
            format!("proxy_{hash}"),
        ] {
            let resolved = resolve_tee(&form).unwrap_or_else(|| panic!("must resolve {form}"));
            assert_eq!(
                resolved.to_string_lossy(),
                handle,
                "form {form} -> {handle}"
            );
        }
    }

    #[test]
    fn resolve_tee_rejects_nontee_and_traversal_ids() {
        let _lock = crate::core::data_dir::test_env_lock();
        // No FS escape: a crafted path is reduced to its file name, which is not a
        // valid proxy tee name, so it resolves to None instead of reading it.
        assert!(resolve_tee("/etc/passwd").is_none());
        assert!(resolve_tee("../../secret").is_none());
        assert!(resolve_tee("proxy_nothex0000000.log").is_none());
        // Right shape but no such file in the store.
        assert!(resolve_tee("proxy_deadbeefdeadbeef").is_none());
        // A bare hash is not a handle any stub carries.
        assert!(resolve_tee("deadbeefdeadbeef").is_none());
    }

    #[test]
    fn resolve_tee_resolves_shell_tee_with_underscored_slug() {
        let _lock = crate::core::data_dir::test_env_lock();
        // A real shell command whose slug contains underscores: the hash is the
        // last `_`-segment (8 hex), never the first — the parsing trap. The full
        // verbatim output the shell already teed is now surgically retrievable.
        let path = crate::shell::save_tee("gh api /repos/foo/bar", &big("api row"))
            .expect("shell tee saved");
        let name = std::path::Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .to_string();
        assert!(
            is_shell_tee_name(&name),
            "save_tee name must be recognized as a shell tee: {name}"
        );
        for form in [path.clone(), name] {
            assert_eq!(
                resolve_tee(&form)
                    .expect("shell tee form resolves")
                    .to_string_lossy(),
                path,
                "shell tee form -> {path}"
            );
        }
    }

    #[test]
    fn resolve_tee_does_not_capture_reference_ids() {
        let _lock = crate::core::data_dir::test_env_lock();
        // A reference-store id (`ref_<16hex>`, no `.log`) must fall through the
        // tee resolver so sobre_expand routes it to the reference store, not the
        // tee store — the precedence guard for the unified retrieve ladder.
        assert!(resolve_tee("ref_deadbeefcafef00d").is_none());
        // A bare 16-hex archive id with no backing tee file also stays None.
        assert!(resolve_tee("0123456789abcdef").is_none());
    }
}
