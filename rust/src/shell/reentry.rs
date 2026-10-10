//! Re-entrancy marker that stops nested `sobre` invocations from
//! double-compressing.
//!
//! sobre sets [`WRAP_MARKER`] (`SOBRE_WRAPPED`) on the children it spawns,
//! so a nested `sobre -c` sees that a parent sobre already owns compression
//! of this command tree and passes through. Any process can set the variable
//! too: a command run with it set is passed through uncompressed.

use std::process::Command;

/// Env var sobre stamps on every child it spawns to mark the command tree as
/// already owned (compression handled by the parent sobre).
pub(crate) const WRAP_MARKER: &str = "SOBRE_WRAPPED";

/// True when a parent sobre already owns this command tree.
#[must_use]
pub(crate) fn is_wrapped() -> bool {
    std::env::var(WRAP_MARKER).is_ok()
}

/// True when `sobre -c` must avoid owning compression itself: a parent
/// sobre already owns this command tree ([`WRAP_MARKER`]).
#[must_use]
pub(crate) fn should_pass_through() -> bool {
    is_wrapped()
}

/// Stamp a child command so nested sobre invocations pass through.
pub(crate) fn mark_child(cmd: &mut Command) {
    cmd.env(WRAP_MARKER, "1");
}

#[cfg(test)]
mod tests {
    use super::{WRAP_MARKER, is_wrapped, mark_child, should_pass_through};

    #[test]
    fn wrap_marker_triggers_passthrough() {
        let _lock = crate::core::data_dir::test_env_lock();
        crate::test_env::set_var(WRAP_MARKER, "1");
        assert!(is_wrapped());
        assert!(should_pass_through());
        crate::test_env::remove_var(WRAP_MARKER);
    }

    #[test]
    fn mark_child_sets_the_marker() {
        let mut cmd = std::process::Command::new("true");
        mark_child(&mut cmd);
        let set: Option<String> = cmd
            .get_envs()
            .find(|(k, _)| *k == WRAP_MARKER)
            .and_then(|(_, v)| v.and_then(|v| v.to_str().map(str::to_string)));
        assert_eq!(set.as_deref(), Some("1"));
    }
}
