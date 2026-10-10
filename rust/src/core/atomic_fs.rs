//! Shared, policy-free atomic-write mechanics.
//!
//! Both the config installer ([`crate::config_io`]) and the edit tools
//! (`crate::tools::edit_io`) need the same durable write dance: a
//! same-directory temp file + `rename`, with an in-place-overwrite fallback when
//! the directory is read-only but the file inode is writable. Only the
//! *mechanism* lives here — one audited implementation. The differing *policy*
//! stays in each caller:
//!
//! * `edit_io` rejects symlinks (`reject_symlink` + `O_NOFOLLOW`) and guards
//!   TOCTOU before calling in;
//! * `config_io` resolves a user-managed symlink to its real target (within
//!   `$HOME`) and then writes through to it.
//!
//! Not `tempfile::NamedTempFile::persist`: it has none of the read-only-directory
//! fallback, the `O_NOFOLLOW` overwrite and the sweep of temps left by a crash.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

fn invalid_input(msg: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, msg)
}

/// Durable, crash-atomic write: a temp file in the **same directory** as `path`
/// followed by `rename` over the target. Requires write permission on the parent
/// directory; the read-only-directory fallback is handled by
/// [`write_bytes_with_fallback`].
pub(crate) fn try_atomic_write(
    path: &Path,
    bytes: &[u8],
    permissions: Option<&std::fs::Permissions>,
) -> std::io::Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| invalid_input("invalid path (no parent directory)"))?;
    let filename = path
        .file_name()
        .ok_or_else(|| invalid_input("invalid path (no filename)"))?
        .to_string_lossy();

    // Sweep this directory for temps orphaned by a crash between temp-file
    // creation and rename. Nothing else removes them, so every atomic write
    // into a directory reaps what an earlier crashed write left there.
    cleanup_orphaned_temps(parent, &filename);

    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = parent.join(format!(".{filename}.sobre.tmp.{pid}.{nanos}"));

    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        let _ = f.flush();
        let _ = f.sync_all();
    }

    if let Some(perms) = permissions {
        let _ = std::fs::set_permissions(&tmp, perms.clone());
    }

    // `rename` replaces an existing `path` in one step on every platform
    // (`MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` on Windows).
    if let Err(e) = std::fs::rename(&tmp, path) {
        // Don't leave a half-written temp behind before the caller decides
        // whether to fall back.
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    #[cfg(unix)]
    fsync_dir(parent);

    Ok(())
}

/// Best-effort `fsync` of a directory's inode so a preceding `rename`/`create`
/// into it survives a crash (Unix only).
#[cfg(unix)]
fn fsync_dir(dir: &Path) {
    if let Ok(f) = std::fs::File::open(dir) {
        let _ = f.sync_all();
    }
}

/// Removes `.{filename}.sobre.tmp.{pid}.{nanos}` leftovers in `parent` from
/// a crash between temp-file creation and rename.
fn cleanup_orphaned_temps(parent: &Path, filename: &str) {
    const STALE_AGE: std::time::Duration = std::time::Duration::from_hours(1);
    let prefix = format!(".{filename}.sobre.tmp.");

    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name
            .to_string_lossy()
            .strip_prefix(&prefix)
            .map(str::to_string)
        else {
            continue;
        };
        let pid: Option<u32> = rest.split('.').next().and_then(|s| s.parse().ok());
        let pid_dead = pid.is_some_and(|p| !crate::ipc::process::is_alive(p));
        let stale_by_age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > STALE_AGE);
        if pid_dead || stale_by_age {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// In-place overwrite of an existing file inode (`O_WRONLY|O_TRUNC`, plus
/// `O_NOFOLLOW` on Unix). Works when the parent directory is read-only but the
/// file itself is writable. Not crash-atomic — used only as a fallback when the
/// atomic path is impossible.
pub(crate) fn in_place_overwrite(
    path: &Path,
    bytes: &[u8],
    permissions: Option<&std::fs::Permissions>,
) -> std::io::Result<()> {
    use std::io::Write;

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // O_NOFOLLOW: a symlink swapped in after the caller's checks must never
        // be followed here (mirrors the read-side O_NOFOLLOW boundary).
        opts.custom_flags(libc::O_NOFOLLOW);
    }

    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    let _ = f.flush();
    let _ = f.sync_all();

    if let Some(perms) = permissions {
        let _ = std::fs::set_permissions(path, perms.clone());
    }
    Ok(())
}

/// True for errors that mean "this directory won't accept create/rename" even
/// though the target file may be writable: `EROFS` (read-only fs) plus
/// `EACCES`/`EPERM` (directory write denied).
pub(crate) fn is_readonly_dir_error(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        return true;
    }
    #[cfg(unix)]
    {
        matches!(
            e.raw_os_error(),
            Some(libc::EROFS | libc::EACCES | libc::EPERM)
        )
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Atomic write with the read-only-directory in-place fallback. Tries the
/// crash-atomic temp+rename first; if that fails because the *directory* is
/// read-only/permission-denied but an existing file inode is writable, overwrite
/// it in place. `permissions`, when given, is applied to the written file.
pub(crate) fn write_bytes_with_fallback(
    path: &Path,
    bytes: &[u8],
    permissions: Option<&std::fs::Permissions>,
) -> Result<(), String> {
    match try_atomic_write(path, bytes, permissions) {
        Ok(()) => Ok(()),
        Err(e) if is_readonly_dir_error(&e) && path.is_file() => {
            in_place_overwrite(path, bytes, permissions).map_err(|fallback_err| {
                format!(
                    "atomic write failed ({e}); in-place fallback also failed: {fallback_err} ({})",
                    path.display()
                )
            })
        }
        Err(e) => Err(format!("atomic write failed: {e} ({})", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_dir_error_classification() {
        assert!(is_readonly_dir_error(&std::io::Error::from(
            std::io::ErrorKind::PermissionDenied
        )));
        assert!(!is_readonly_dir_error(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
        #[cfg(unix)]
        {
            assert!(is_readonly_dir_error(&std::io::Error::from_raw_os_error(
                libc::EROFS
            )));
            assert!(is_readonly_dir_error(&std::io::Error::from_raw_os_error(
                libc::EACCES
            )));
            assert!(is_readonly_dir_error(&std::io::Error::from_raw_os_error(
                libc::EPERM
            )));
        }
    }

    // --- orphaned-temp sweep ---

    #[test]
    fn cleanup_orphaned_temps_removes_dead_pid_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let filename = "cfg.toml";
        let dead_pid = 999_999_999u32;
        let orphan = dir
            .path()
            .join(format!(".{filename}.sobre.tmp.{dead_pid}.123"));
        std::fs::write(&orphan, b"stale").unwrap();

        cleanup_orphaned_temps(dir.path(), filename);

        assert!(
            !orphan.exists(),
            "orphaned temp with a dead PID must be removed"
        );
    }

    #[test]
    fn cleanup_orphaned_temps_keeps_fresh_temp_from_a_live_pid() {
        let dir = tempfile::tempdir().unwrap();
        let filename = "cfg.toml";
        let my_pid = std::process::id();
        let in_progress = dir
            .path()
            .join(format!(".{filename}.sobre.tmp.{my_pid}.123"));
        std::fs::write(&in_progress, b"still writing").unwrap();

        cleanup_orphaned_temps(dir.path(), filename);

        assert!(
            in_progress.exists(),
            "a live writer's own in-progress temp must survive"
        );
    }

    #[test]
    fn cleanup_orphaned_temps_removes_old_temp_even_with_a_live_pid() {
        let dir = tempfile::tempdir().unwrap();
        let filename = "cfg.toml";
        let my_pid = std::process::id();
        let ancient = dir
            .path()
            .join(format!(".{filename}.sobre.tmp.{my_pid}.123"));
        std::fs::write(&ancient, b"ancient").unwrap();
        filetime::set_file_mtime(&ancient, filetime::FileTime::from_unix_time(0, 0)).unwrap();

        cleanup_orphaned_temps(dir.path(), filename);

        assert!(
            !ancient.exists(),
            "ancient temp must be removed regardless of PID liveness"
        );
    }

    #[test]
    fn cleanup_orphaned_temps_ignores_unrelated_files() {
        let dir = tempfile::tempdir().unwrap();
        let filename = "cfg.toml";
        let unrelated = dir.path().join("other-file.txt");
        std::fs::write(&unrelated, b"keep me").unwrap();

        cleanup_orphaned_temps(dir.path(), filename);

        assert!(unrelated.exists(), "non-matching files must be left alone");
    }
    #[test]
    fn try_atomic_write_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        try_atomic_write(&path, b"first", None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        // No leftover temp files.
        let strays: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".sobre.tmp."))
            .collect();
        assert!(strays.is_empty(), "temp file must not linger");
        try_atomic_write(&path, b"second", None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
    }

    #[cfg(unix)]
    #[test]
    fn in_place_overwrite_truncates_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.jsonc");
        std::fs::write(&path, b"longer original content").unwrap();
        in_place_overwrite(&path, b"short", None).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"short");
    }

    #[cfg(unix)]
    #[test]
    fn fallback_overwrites_when_parent_dir_is_readonly() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg.toml");
        std::fs::write(&path, b"original").unwrap();
        // Read-only parent dir: temp+rename is impossible, but the file inode
        // stays writable, so the in-place fallback must succeed.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let res = write_bytes_with_fallback(&path, b"updated", None);
        let _ = std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700));
        res.expect("read-only-dir fallback must succeed");
        assert_eq!(std::fs::read(&path).unwrap(), b"updated");
    }
}
