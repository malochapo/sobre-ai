use std::path::{Path, PathBuf};

/// The copy of an agent config as it was before sobre first edited it,
/// empty when the file did not exist: `uninstall` then deletes a file that
/// holds nothing but sobre's entries. `uninstall` deletes the copy once
/// sobre's entries are gone from the original.
pub fn backup_path_for(path: &Path) -> PathBuf {
    let filename = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{filename}.sobre.bak"))
}

/// Writes an agent config file that `init` (or `proxy enable`) edits.
///
/// - Content identical to what is on disk writes nothing.
/// - The first edit of an existing file keeps a copy of it as
///   [`backup_path_for`]; later edits leave that copy alone, so it stays the
///   file the user had before sobre. Creating the file leaves an empty copy:
///   the next `init` then knows sobre made it, and does not keep sobre's own
///   entries as the user's.
/// - The write is recorded in the [`journal`]; while the journal previews
///   (`init --check`), nothing is written.
pub fn write_atomic_with_backup(path: &Path, content: &str) -> Result<(), String> {
    let existed = path.exists();
    if existed && std::fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(());
    }
    journal::record(
        path,
        if existed {
            journal::Change::Updated
        } else {
            journal::Change::Created
        },
    );
    if journal::dry_run() {
        return Ok(());
    }
    let bak = backup_path_for(path);
    if existed {
        if !bak.exists() {
            let _ = std::fs::copy(path, &bak);
        }
        return write_atomic(path, content);
    }
    // After the write, which creates the parent directories.
    write_atomic(path, content)?;
    let _ = std::fs::write(&bak, "");
    Ok(())
}

/// Writes a file sobre owns entirely (the skill, the OpenCode plugin): journaled
/// like [`write_atomic_with_backup`], with no backup since nothing of the
/// user's is in it.
pub fn write_owned(path: &Path, content: &str) -> Result<(), String> {
    let existed = path.exists();
    if existed && std::fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(());
    }
    journal::record(
        path,
        if existed {
            journal::Change::Updated
        } else {
            journal::Change::Created
        },
    );
    if journal::dry_run() {
        return Ok(());
    }
    write_atomic(path, content)
}

/// Writes `value` as an edit of `original`, the text it was parsed from: the
/// user's comments, key order and formatting are kept (see
/// [`crate::core::jsonc::rewrite_preserving`]).
pub fn write_json_edit(
    path: &Path,
    original: &str,
    value: &serde_json::Value,
) -> Result<(), String> {
    let rendered = if original.trim().is_empty() {
        crate::core::jsonc::pretty(value)
    } else {
        crate::core::jsonc::rewrite_preserving(original, value)
    };
    write_atomic_with_backup(path, &rendered)
}

/// Like [`write_json_edit`], for a write that takes sobre's settings out of a
/// file (`proxy disable`, `uninstall`): no backup is made, so the copy `init`
/// kept from before sobre stays the one `uninstall` compares against.
pub fn write_json_cleanup(
    path: &Path,
    original: &str,
    value: &serde_json::Value,
) -> Result<(), String> {
    write_atomic(
        path,
        &crate::core::jsonc::rewrite_preserving(original, value),
    )
}

/// Edits the JSON(C) agent config at `path` in place. `edit` gets the root
/// object (empty when the file is missing or blank); the result is written
/// back with the user's comments, key order and formatting kept, through
/// [`write_atomic_with_backup`]. Nothing is written when `edit` changed
/// nothing, and a missing file is not created for an empty object. A file
/// that does not parse, or whose root is not an object, is left alone.
pub fn edit_json(
    path: &Path,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) -> Result<(), String> {
    let original = std::fs::read_to_string(path).unwrap_or_default();
    let before = if original.trim().is_empty() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        crate::core::jsonc::parse_jsonc(&original)
            .map_err(|e| format!("{} is not valid JSON ({e}); left unchanged", path.display()))?
    };
    let mut after = before.clone();
    let obj = after.as_object_mut().ok_or_else(|| {
        format!(
            "{} does not hold a JSON object; left unchanged",
            path.display()
        )
    })?;
    edit(obj);
    if after == before {
        return Ok(());
    }
    let rendered = if original.trim().is_empty() {
        crate::core::jsonc::pretty(&after)
    } else {
        crate::core::jsonc::rewrite_preserving(&original, &after)
    };
    write_atomic_with_backup(path, &rendered)
}

/// What `init` writes, recorded as it goes: the per-agent summary is built from
/// it, and `init --check` runs the same code with writes turned off.
pub mod journal {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    /// What a write did to one file.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Change {
        Created,
        Updated,
    }

    struct Journal {
        dry_run: bool,
        entries: Vec<(PathBuf, Change)>,
    }

    thread_local! {
        static JOURNAL: RefCell<Option<Journal>> = const { RefCell::new(None) };
    }

    /// Starts recording on this thread. With `dry_run`, writes that go through
    /// [`super::write_atomic_with_backup`] and [`super::ensure_dir`] are
    /// recorded and not performed.
    pub fn begin(dry_run: bool) {
        JOURNAL.with(|j| {
            *j.borrow_mut() = Some(Journal {
                dry_run,
                entries: Vec::new(),
            });
        });
    }

    /// Stops recording.
    pub fn end() {
        JOURNAL.with(|j| *j.borrow_mut() = None);
    }

    /// The files written since the last call, each once and in first-write
    /// order. A file written several times counts as created if any write
    /// created it.
    pub fn take() -> Vec<(PathBuf, Change)> {
        let entries = JOURNAL.with(|j| {
            j.borrow_mut()
                .as_mut()
                .map(|j| std::mem::take(&mut j.entries))
                .unwrap_or_default()
        });
        let mut merged: Vec<(PathBuf, Change)> = Vec::new();
        for (path, change) in entries {
            match merged.iter_mut().find(|(p, _)| *p == path) {
                Some(existing) if change == Change::Created => existing.1 = Change::Created,
                Some(_) => {}
                None => merged.push((path, change)),
            }
        }
        merged
    }

    /// True while `init --check` previews: nothing may be written.
    pub fn dry_run() -> bool {
        JOURNAL.with(|j| j.borrow().as_ref().is_some_and(|j| j.dry_run))
    }

    /// Records a change about to be made.
    pub(super) fn record(path: &Path, change: Change) {
        JOURNAL.with(|j| {
            if let Some(j) = j.borrow_mut().as_mut() {
                j.entries.push((path.to_path_buf(), change));
            }
        });
    }
}

/// Merges `new_content` into the TOML file at `path`, keeping its comments,
/// formatting and unknown keys, and keeping the config minimal: keys whose value
/// equals the type's default AND are not already present on disk are skipped,
/// so a hand-written config is not bloated with every default key. Existing
/// keys are always updated (preserving comments), and non-default values are
/// always written. `default_content` is `toml::to_string_pretty(&T::default())`.
pub fn write_toml_preserving_minimal(
    path: &Path,
    new_content: &str,
    default_content: &str,
) -> Result<(), String> {
    let merged = match std::fs::read_to_string(path) {
        Ok(existing) if !existing.trim().is_empty() => {
            // Refuse to overwrite a non-empty file we cannot parse. `new_content`
            // and `default_content` come from our own serializer (always valid),
            // so a merge failure means the on-disk config is corrupt — clobbering
            // it with defaults would silently wipe customizations. We
            // propagate the error and leave the file untouched instead.
            merge_toml_inner(&existing, new_content, Some(default_content)).map_err(|e| {
                format!(
                    "refusing to overwrite an unparseable config at {}: {e}",
                    path.display()
                )
            })?
        }
        // No existing file: write a fresh minimal document (drop defaults).
        _ => merge_toml_inner("", new_content, Some(default_content))
            .unwrap_or_else(|_| new_content.to_string()),
    };
    write_atomic(path, &merged)
}

/// Merges `incoming` TOML values onto the `existing` document, retaining the
/// existing document's comments, whitespace, and unknown keys.
#[cfg(test)]
fn merge_toml(existing: &str, incoming: &str) -> Result<String, String> {
    merge_toml_inner(existing, incoming, None)
}

fn merge_toml_inner(
    existing: &str,
    incoming: &str,
    defaults: Option<&str>,
) -> Result<String, String> {
    let mut existing_doc = existing
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    let incoming_doc = incoming
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    let default_doc = match defaults {
        Some(d) => Some(
            d.parse::<toml_edit::DocumentMut>()
                .map_err(|e| e.to_string())?,
        ),
        None => None,
    };
    merge_table(
        existing_doc.as_table_mut(),
        incoming_doc.as_table(),
        default_doc.as_ref().map(toml_edit::DocumentMut::as_table),
    );
    Ok(existing_doc.to_string())
}

/// Recursively merges `source` keys into `target`, updating values in place so
/// surrounding comments (key decor) survive, recursing into nested tables, and
/// preserving inline value decor (trailing comments) on updated leaves.
///
/// When `defaults` is `Some`, a key that is absent from `target` and whose value
/// equals the corresponding default is skipped (minimal-config mode).
fn merge_table(
    target: &mut toml_edit::Table,
    source: &toml_edit::Table,
    defaults: Option<&toml_edit::Table>,
) {
    use toml_edit::Item;
    for (key, source_item) in source {
        let default_item = defaults.and_then(|d| d.get(key));
        match (source_item, target.get_mut(key)) {
            (Item::Table(source_tbl), Some(Item::Table(target_tbl))) => {
                merge_table(
                    target_tbl,
                    source_tbl,
                    default_item.and_then(Item::as_table),
                );
            }
            (Item::Value(source_val), Some(Item::Value(target_val))) => {
                let prefix = target_val.decor().prefix().cloned();
                let suffix = target_val.decor().suffix().cloned();
                let mut new_val = source_val.clone();
                if let Some(p) = prefix {
                    new_val.decor_mut().set_prefix(p);
                }
                if let Some(s) = suffix {
                    new_val.decor_mut().set_suffix(s);
                }
                *target_val = new_val;
            }
            (_, Some(target_item)) => {
                *target_item = source_item.clone();
            }
            (Item::Table(source_tbl), None) if defaults.is_some() => {
                // New table in minimal mode: build it from non-default leaves
                // only and skip it entirely if nothing meaningful remains.
                let mut fresh = toml_edit::Table::new();
                merge_table(
                    &mut fresh,
                    source_tbl,
                    default_item.and_then(Item::as_table),
                );
                if !fresh.is_empty() {
                    target.insert(key, Item::Table(fresh));
                }
            }
            (_, None) => {
                if defaults.is_none() || !item_equals_default(source_item, default_item) {
                    target.insert(key, source_item.clone());
                }
            }
        }
    }
}

/// Compares a serialized item against its default, ignoring decor. Both sides
/// originate from the same serializer, so their normalized string form matches
/// exactly when the underlying values are equal.
fn item_equals_default(item: &toml_edit::Item, default: Option<&toml_edit::Item>) -> bool {
    match default {
        Some(d) => item.to_string().trim() == d.to_string().trim(),
        None => false,
    }
}

/// Writes `content` to `path` atomically (temp file + rename).
pub fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    // A user may symlink agent config (`~/.claude.json`,
    // `~/.codex/config.toml`, …) into a managed dotfiles repo. The write goes
    // THROUGH the symlink to its target, wherever that is, and the symlink
    // stays.
    let target = resolve_write_target(path)?;

    if let Some(parent) = target.parent() {
        ensure_dir(parent)?;
    }

    // An existing file keeps its mode; a new one is owner-only. Windows ACLs
    // are left untouched. The temp+rename mechanics and the read-only-directory
    // in-place fallback are shared with the edit tools via
    // `core::atomic_fs`.
    #[cfg(unix)]
    let perms = {
        use std::os::unix::fs::PermissionsExt;
        Some(std::fs::metadata(&target).map_or_else(
            |_| std::fs::Permissions::from_mode(0o600),
            |m| m.permissions(),
        ))
    };
    #[cfg(not(unix))]
    let perms: Option<std::fs::Permissions> = None;

    crate::core::atomic_fs::write_bytes_with_fallback(&target, content.as_bytes(), perms.as_ref())
}

/// Resolve the real file to write, honoring a user-managed symlink.
///
/// * not a symlink (or missing) → `path` unchanged.
/// * symlink → its resolved target (write THROUGH, preserving the symlink) —
///   the dotfiles pattern. The link is the user's; sobre follows it.
fn resolve_write_target(path: &Path) -> Result<PathBuf, String> {
    let Ok(meta) = path.symlink_metadata() else {
        return Ok(path.to_path_buf());
    };
    if !crate::core::pathutil::is_symlink_or_reparse(&meta) {
        return Ok(path.to_path_buf());
    }

    resolve_symlink_target(path)
}

/// Read a symlink and resolve its target to an absolute path, resolving symlinks
/// in the existing-ancestor portion (so a symlinked *parent* is followed too)
/// while tolerating a not-yet-created target file/dir.
fn resolve_symlink_target(link_path: &Path) -> Result<PathBuf, String> {
    let link = std::fs::read_link(link_path)
        .map_err(|e| format!("cannot read symlink {}: {e}", link_path.display()))?;
    let raw_target = if link.is_absolute() {
        link
    } else {
        link_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(link)
    };
    canonicalize_existing_prefix(&raw_target)
}

/// Canonicalize `path` by resolving its deepest *existing* ancestor (following
/// symlinks) and re-appending the not-yet-created tail, so a target that does
/// not exist yet still resolves to a real path.
fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, String> {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(real) = crate::core::pathutil::canonicalize_secure(cur) {
            let mut out = real;
            for comp in tail.iter().rev() {
                out.push(comp);
            }
            return Ok(out);
        }
        match cur.parent() {
            Some(parent) if parent != cur => {
                if let Some(name) = cur.file_name() {
                    tail.push(name.to_os_string());
                }
                cur = parent;
            }
            _ => {
                return Err(format!(
                    "cannot resolve any existing ancestor of {}",
                    path.display()
                ));
            }
        }
    }
}

/// `create_dir_all` that tolerates a user-managed symlinked directory:
///
/// * regular dir / missing path → `create_dir_all`.
/// * symlink to an existing directory → ok (no-op).
/// * dangling symlink → create the real target.
/// * symlink to a non-directory → clear error.
pub fn ensure_dir(dir: &Path) -> Result<(), String> {
    if journal::dry_run() {
        return Ok(());
    }
    match dir.symlink_metadata() {
        Ok(meta) if crate::core::pathutil::is_symlink_or_reparse(&meta) => {
            match std::fs::metadata(dir) {
                Ok(m) if m.is_dir() => Ok(()),
                Ok(_) => Err(format!(
                    "{} is a symlink to a non-directory; fix or remove the symlink",
                    dir.display()
                )),
                Err(_) => {
                    // Dangling symlink: create the intended target.
                    let real_target = resolve_symlink_target(dir)?;
                    std::fs::create_dir_all(&real_target).map_err(|e| {
                        format!(
                            "cannot create symlink target dir {}: {e}",
                            real_target.display()
                        )
                    })
                }
            }
        }
        _ => std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create directory {}: {e}", dir.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_comments_and_unknown_keys() {
        let existing = "\
# My custom config — do not delete!
debug_log = true  # inline note

# Section about the proxy
[proxy]
enabled = false
custom_user_key = \"keep-me\"
";
        let incoming = "\
debug_log = false

[proxy]
enabled = true
";
        let merged = merge_toml(existing, incoming).unwrap();

        // Comments survive.
        assert!(merged.contains("# My custom config — do not delete!"));
        assert!(merged.contains("# inline note"));
        assert!(merged.contains("# Section about the proxy"));
        // Unknown / user keys survive.
        assert!(merged.contains("custom_user_key = \"keep-me\""));
        // Values are updated.
        assert!(merged.contains("debug_log = false"));
        assert!(merged.contains("enabled = true"));
        assert!(!merged.contains("enabled = false"));
    }

    #[test]
    fn minimal_mode_skips_unset_defaults_but_keeps_existing() {
        // On-disk: only debug_log is explicitly set, with a comment.
        let existing = "# my config\ndebug_log = true\n";
        // Incoming: full serialization (all fields present).
        let incoming = "debug_log = false\nshell_timeout_secs = 15\nhook_binary = \"sobre\"\n";
        // Defaults: what an untouched config would serialize to.
        let defaults = "debug_log = false\nshell_timeout_secs = 15\nhook_binary = \"sobre\"\n";

        let merged = merge_toml_inner(existing, incoming, Some(defaults)).unwrap();

        // Existing key updated + comment preserved.
        assert!(merged.contains("# my config"));
        assert!(merged.contains("debug_log = false"));
        // Default-valued keys that were never on disk are NOT added (stay minimal).
        assert!(!merged.contains("shell_timeout_secs"));
        assert!(!merged.contains("hook_binary"));
    }

    #[test]
    fn minimal_mode_writes_non_default_values() {
        let existing = "";
        let incoming = "debug_log = false\nshell_timeout_secs = 42\n";
        let defaults = "debug_log = false\nshell_timeout_secs = 15\n";

        let merged = merge_toml_inner(existing, incoming, Some(defaults)).unwrap();

        // Non-default value is written, default value is skipped.
        assert!(merged.contains("shell_timeout_secs = 42"));
        assert!(!merged.contains("debug_log"));
    }

    #[test]
    fn minimal_mode_drops_empty_default_tables() {
        let existing = "";
        let incoming = "[proxy]\nenabled = false\n\n[lsp]\n";
        let defaults = "[proxy]\nenabled = false\n\n[lsp]\n";

        let merged = merge_toml_inner(existing, incoming, Some(defaults)).unwrap();

        // Everything equals default and nothing exists on disk → empty output.
        assert!(!merged.contains("[lsp]"));
        assert!(!merged.contains("[proxy]"));
    }

    #[test]
    fn merge_adds_new_keys_and_sections() {
        let existing = "debug_log = true\n";
        let incoming = "debug_log = true\nnew_key = 42\n\n[proxy]\nenabled = true\n";
        let merged = merge_toml(existing, incoming).unwrap();
        assert!(merged.contains("new_key = 42"));
        assert!(merged.contains("[proxy]"));
        assert!(merged.contains("enabled = true"));
    }

    fn unique_tmp(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("sobre_{tag}_{}_{nanos}", std::process::id()))
    }

    #[test]
    fn minimal_mode_refuses_to_clobber_unparseable_existing() {
        // A corrupt config must never be silently replaced with defaults.
        let tmp = unique_tmp("cfg_corrupt");
        let _ = std::fs::create_dir_all(&tmp);
        let path = tmp.join("config.toml");
        let corrupt = "broken = = =\n";
        std::fs::write(&path, corrupt).unwrap();

        let result =
            write_toml_preserving_minimal(&path, "debug_log = false\n", "debug_log = false\n");

        assert!(
            result.is_err(),
            "must refuse to overwrite an unparseable config"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt,
            "the corrupt file must be left exactly as-is"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

/// Write THROUGH a user-managed symlink to its real target, and make
/// `ensure_dir` tolerant of symlinked directories. Unix-only (POSIX symlinks + `$HOME` override).
#[cfg(all(test, unix))]
mod symlink_tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// RAII override of `$HOME` that restores the previous value on drop (even on
    /// panic). Pair with `test_env_lock()` so env access stays serialized.
    struct HomeGuard(Option<std::ffi::OsString>);
    impl HomeGuard {
        fn set(home: &Path) -> Self {
            let prev = std::env::var_os("HOME");
            crate::test_env::set_var("HOME", home);
            HomeGuard(prev)
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => crate::test_env::set_var("HOME", v),
                None => crate::test_env::remove_var("HOME"),
            }
        }
    }

    #[test]
    fn write_through_symlink_in_home_updates_target_and_keeps_link() {
        let _lock = crate::core::data_dir::test_env_lock();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(home.path());

        let dotfiles = home.path().join("dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap();
        let target = dotfiles.join("agent.json");
        std::fs::write(&target, "{}\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let link = home.path().join(".agent.json");
        symlink(&target, &link).unwrap();

        write_atomic(&link, "{\"k\":1}\n").unwrap();

        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the user symlink must be preserved (write-through, not replace)"
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"k\":1}\n");

        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644,
            "the real config file keeps its mode"
        );

        let fresh = home.path().join(".new.json");
        write_atomic(&fresh, "{}\n").unwrap();
        assert_eq!(
            std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o600,
            "a file sobre creates is owner-only"
        );
    }

    #[test]
    fn writes_through_symlink_whose_target_is_outside_home() {
        // The link is the user's: sobre follows it wherever it points.
        let _lock = crate::core::data_dir::test_env_lock();
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(home.path());

        // Canonical root (macOS tempdirs live under /var → /private/var).
        let real_outside = std::fs::canonicalize(outside.path()).unwrap();
        let target = real_outside.join("agent.json");
        std::fs::write(&target, "{}\n").unwrap();
        let link = home.path().join(".agent.json");
        symlink(&target, &link).unwrap();

        write_atomic(&link, "{\"k\":1}\n").unwrap();

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"k\":1}\n");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the user symlink must be preserved (write-through, not replace)"
        );
    }

    #[test]
    fn ensure_dir_accepts_symlink_to_dir_rejects_symlink_to_file() {
        let _lock = crate::core::data_dir::test_env_lock();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(home.path());

        let real_dir = home.path().join("real_dir");
        std::fs::create_dir_all(&real_dir).unwrap();
        let dir_link = home.path().join(".agentdir");
        symlink(&real_dir, &dir_link).unwrap();
        assert!(
            ensure_dir(&dir_link).is_ok(),
            "a healthy dir symlink must be accepted"
        );

        let real_file = home.path().join("real_file");
        std::fs::write(&real_file, "x").unwrap();
        let file_link = home.path().join(".agentfile");
        symlink(&real_file, &file_link).unwrap();
        let err = ensure_dir(&file_link).unwrap_err();
        assert!(err.contains("non-directory"), "got: {err}");
    }

    #[test]
    fn ensure_dir_creates_dangling_symlink_target_in_home() {
        let _lock = crate::core::data_dir::test_env_lock();
        let home = tempfile::tempdir().unwrap();
        let _home = HomeGuard::set(home.path());

        // Dangling: link → home/dotfiles/.codex, neither exists yet.
        let target = home.path().join("dotfiles/.codex");
        let link = home.path().join(".codex");
        symlink(&target, &link).unwrap();

        ensure_dir(&link).unwrap();

        assert!(target.is_dir(), "dangling symlink target must be created");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the symlink itself must remain"
        );
        assert!(std::fs::metadata(&link).unwrap().is_dir());
    }
}
