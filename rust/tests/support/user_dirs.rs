//! Keeps an integration-test binary out of the developer's real directories.
//! Included by every tests/*.rs harness with `#[path]`.
//!
//! Integration tests link the library without `cfg(test)`, so its per-process
//! test sandbox does not apply and every store write would land in the real
//! `~/.local/share/lean-ctx`, `~/.local/state/lean-ctx` and `~/.cache/lean-ctx`.
//!
//! The static below runs before `main` and points HOME and the XDG bases at a
//! fresh per-process temp dir, plus a non-standard `LEAN_CTX_DATA_DIR`, which
//! holds config, state and cache as well (the same isolation
//! `.config/nextest-tmpdir.sh` gives nextest runs). A test that sets its own
//! `LEAN_CTX_DATA_DIR` replaces it; one that removes it still resolves into
//! the temp dir through HOME and the XDG bases.

const SANDBOXED_VARS: &[(&str, &str)] = &[
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "home/.config"),
    ("XDG_DATA_HOME", "home/.local/share"),
    ("XDG_STATE_HOME", "home/.local/state"),
    ("XDG_CACHE_HOME", "home/.cache"),
    ("LEAN_CTX_DATA_DIR", ".lean-ctx"),
];

extern "C" fn sandbox_user_dirs() {
    let root = std::env::temp_dir().join(format!("lean-ctx-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (var, rel) in SANDBOXED_VARS {
        let dir = root.join(rel);
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: runs from the platform's initializer list before `main`,
        // while the process has a single thread.
        unsafe { std::env::set_var(var, &dir) };
    }
}

#[used]
#[cfg_attr(
    any(target_os = "linux", target_os = "android", target_os = "freebsd"),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
static SANDBOX_USER_DIRS: extern "C" fn() = sandbox_user_dirs;

/// The initializer ran: HOME points into this process's sandbox.
pub(crate) fn assert_sandboxed() {
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(
        home.contains(&format!("lean-ctx-it-{}", std::process::id())),
        "the pre-main sandbox did not run: HOME is {home:?}, so tests would write to real user dirs"
    );
}
