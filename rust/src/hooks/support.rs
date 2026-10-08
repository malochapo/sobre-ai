//! Codex's `[features] hooks` flag. Codex runs hooks only while it is on;
//! `init` turns it on only when the user never set it, and marks the line so
//! `uninstall` removes exactly what `init` added.

/// The comment `init` leaves on the flag it adds.
const ADDED_MARK: &str = "added by sobre init";

/// Codex's names for the flag, current first.
const FLAG_KEYS: [&str; 2] = ["hooks", "codex_hooks"];

/// What `init` found, or did, about Codex's hooks flag.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CodexHooksFlag {
    /// The user's config already turns hooks on.
    On,
    /// The flag was missing; `init` added it. Holds the new config.
    Added(String),
    /// The user turned hooks off; `init` leaves that alone.
    Off,
}

/// Turns Codex hooks on when the user never set the flag. An explicit
/// `false` is the user's choice and stays.
pub(crate) fn ensure_codex_hooks_enabled(content: &str) -> Result<CodexHooksFlag, String> {
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| e.to_string())?;
    let features = doc["features"].or_insert(toml_edit::table());
    let features = features
        .as_table_like_mut()
        .ok_or("[features] is not a table")?;
    for key in FLAG_KEYS {
        if let Some(value) = features.get(key).and_then(toml_edit::Item::as_bool) {
            return Ok(if value {
                CodexHooksFlag::On
            } else {
                CodexHooksFlag::Off
            });
        }
    }
    let mut flag = toml_edit::value(true);
    if let Some(v) = flag.as_value_mut() {
        v.decor_mut().set_suffix(format!("  # {ADDED_MARK}"));
    }
    features.insert("hooks", flag);
    Ok(CodexHooksFlag::Added(doc.to_string()))
}

/// Removes the flag `init` added, and the `[features]` table if nothing else
/// is in it. A flag the user set is left as it is. `None` when nothing
/// changes or the file is not TOML.
pub(crate) fn remove_codex_hooks_flag(content: &str) -> Option<String> {
    let mut doc = content.parse::<toml_edit::DocumentMut>().ok()?;
    let features = doc.get_mut("features")?.as_table_like_mut()?;
    let ours = FLAG_KEYS.into_iter().find(|key| {
        features
            .get(key)
            .and_then(toml_edit::Item::as_value)
            .and_then(|v| v.decor().suffix())
            .and_then(toml_edit::RawString::as_str)
            .is_some_and(|suffix| suffix.contains(ADDED_MARK))
    })?;
    features.remove(ours);
    if features.is_empty() {
        doc.remove("features");
    }
    Some(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_a_marked_flag_when_missing_and_uninstall_removes_it() {
        let original = "model = \"o3\"\n";
        let CodexHooksFlag::Added(with_flag) = ensure_codex_hooks_enabled(original).unwrap() else {
            panic!("flag should be added");
        };
        assert!(with_flag.contains("[features]"), "{with_flag}");
        assert!(with_flag.contains(ADDED_MARK), "{with_flag}");
        assert_eq!(
            ensure_codex_hooks_enabled(&with_flag).unwrap(),
            CodexHooksFlag::On
        );
        assert_eq!(remove_codex_hooks_flag(&with_flag).unwrap(), original);
    }

    #[test]
    fn a_flag_in_an_existing_table_goes_and_the_table_stays() {
        let original = "[features]\nweb_search = true\n";
        let CodexHooksFlag::Added(with_flag) = ensure_codex_hooks_enabled(original).unwrap() else {
            panic!("flag should be added");
        };
        assert_eq!(remove_codex_hooks_flag(&with_flag).unwrap(), original);
    }

    #[test]
    fn the_users_own_flag_is_never_changed() {
        for (input, expected) in [
            ("[features]\nhooks = true\n", CodexHooksFlag::On),
            ("[features]\ncodex_hooks = true\n", CodexHooksFlag::On),
            ("[features]\nhooks = false\n", CodexHooksFlag::Off),
        ] {
            assert_eq!(ensure_codex_hooks_enabled(input).unwrap(), expected);
            assert_eq!(remove_codex_hooks_flag(input), None, "{input}");
        }
    }

    #[test]
    fn hooks_keys_in_other_tables_are_not_the_flag() {
        let input = "[profiles.x]\nhooks = false\n";
        assert!(matches!(
            ensure_codex_hooks_enabled(input).unwrap(),
            CodexHooksFlag::Added(_)
        ));
    }
}
