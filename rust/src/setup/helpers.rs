//! Setup helper routines: TOML key upserts, path shortening. Split out of
//! `setup/mod.rs` for focus.

pub(crate) fn shorten_path(path: &str, home: &str) -> String {
    if let Some(stripped) = path.strip_prefix(home) {
        format!("~{stripped}")
    } else {
        path.to_string()
    }
}
