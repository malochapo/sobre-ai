//! Re-entrancy markers that stop nested `lean-ctx` invocations from
//! double-compressing — decoupled from the user-facing activation flag.
//!
//! Two distinct signals were historically conflated in `LEAN_CTX_ACTIVE`
//! (GH #533), which silently disabled compression for any agent that
//! *inherited* it:
//!
//! - `LEAN_CTX_ACTIVE`: shell-hook re-entry guard. Stops the *shell* hook from
//!   re-firing inside a command lean-ctx already spawned. It is a plain env var
//!   an agent's top-level process can legitimately inherit.
//! - [`WRAP_MARKER`] (`LEAN_CTX_WRAPPED`): process-level ownership. Set ONLY by
//!   lean-ctx on the children it spawns, so its presence reliably means "a
//!   parent lean-ctx already owns compression of this command tree" and a
//!   nested `lean-ctx -c` must pass through. Because lean-ctx is the only
//!   writer, an agent cannot trigger it by leaking an env var.

use std::process::Command;

/// Env var lean-ctx stamps on every child it spawns to mark the command tree as
/// already owned (compression handled by the parent lean-ctx).
pub(crate) const WRAP_MARKER: &str = "LEAN_CTX_WRAPPED";

/// Legacy shell-hook re-entry guard. Still stamped on children so the shell
/// hook (which tests `-z "$LEAN_CTX_ACTIVE"`) does not re-fire inside a wrapped
/// command, but it no longer gates compression on its own.
pub(crate) const ACTIVE_MARKER: &str = "LEAN_CTX_ACTIVE";

/// True when a parent lean-ctx already owns this command tree. Nested explicit
/// `lean-ctx -c` should defer to the ambient shell defaults, not force raw
/// command bytes, so shell-hook compression can still apply when configured.
#[must_use]
pub(crate) fn is_wrapped() -> bool {
    std::env::var(WRAP_MARKER).is_ok()
}

/// True when `lean-ctx -c` / `-t` must avoid owning compression itself: a
/// parent lean-ctx already owns this command tree ([`WRAP_MARKER`]).
///
/// Deliberately does **not** consult `LEAN_CTX_ACTIVE`: that flag is only a
/// shell-hook re-entry guard, and an agent's top-level process can inherit it,
/// which previously suppressed compression for every command it ran (#533).
#[must_use]
pub(crate) fn should_pass_through() -> bool {
    is_wrapped()
}

/// Stamp a child command so nested lean-ctx invocations pass through and the
/// shell hook does not re-fire: sets both the ownership marker
/// ([`WRAP_MARKER`]) and the legacy hook guard (`LEAN_CTX_ACTIVE`).
pub(crate) fn mark_child(cmd: &mut Command) {
    cmd.env(ACTIVE_MARKER, "1").env(WRAP_MARKER, "1");
}

/// Clear lean-ctx ownership markers before delegating to the user's shell
/// defaults. This lets a nested explicit wrapper behave like typing the command
/// in that shell: configured hooks may compress; unconfigured shells run raw.
pub(crate) fn clear_shell_default_markers(cmd: &mut Command) {
    cmd.env_remove(ACTIVE_MARKER).env_remove(WRAP_MARKER);
}

#[cfg(test)]
mod tests {
    use super::{
        ACTIVE_MARKER, WRAP_MARKER, clear_shell_default_markers, is_wrapped, mark_child,
        should_pass_through,
    };

    #[test]
    fn wrap_marker_triggers_passthrough() {
        let _lock = crate::core::data_dir::test_env_lock();
        crate::test_env::remove_var(ACTIVE_MARKER);
        crate::test_env::set_var(WRAP_MARKER, "1");
        assert!(is_wrapped());
        assert!(should_pass_through());
        crate::test_env::remove_var(WRAP_MARKER);
    }

    /// #533: an inherited/leaked `LEAN_CTX_ACTIVE` must NOT suppress compression.
    #[test]
    fn inherited_active_does_not_trigger_passthrough() {
        let _lock = crate::core::data_dir::test_env_lock();
        crate::test_env::remove_var(WRAP_MARKER);
        crate::test_env::set_var(ACTIVE_MARKER, "1");
        assert!(
            !should_pass_through(),
            "inherited LEAN_CTX_ACTIVE must not disable compression (#533)"
        );
        crate::test_env::remove_var(ACTIVE_MARKER);
    }

    #[test]
    fn mark_child_sets_both_markers() {
        let mut cmd = std::process::Command::new("true");
        mark_child(&mut cmd);
        let envs: std::collections::HashMap<String, String> = cmd
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();
        assert_eq!(envs.get(WRAP_MARKER).map(String::as_str), Some("1"));
        assert_eq!(envs.get(ACTIVE_MARKER).map(String::as_str), Some("1"));
    }

    #[test]
    fn clear_shell_default_markers_removes_wrapping_state() {
        let mut cmd = std::process::Command::new("true");
        mark_child(&mut cmd);

        clear_shell_default_markers(&mut cmd);

        let envs: std::collections::HashMap<String, Option<String>> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_str()?.to_string(),
                    v.and_then(|v| v.to_str().map(str::to_string)),
                ))
            })
            .collect();

        assert_eq!(envs.get(WRAP_MARKER), Some(&None));
        assert_eq!(envs.get(ACTIVE_MARKER), Some(&None));
    }
}
