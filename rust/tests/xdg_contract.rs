//! Contract: with no `SOBRE_*_DIR` override, data / config / state / cache
//! each resolve to their typed `$XDG_*` directory.
//!
//! This runs as an INTEGRATION test on purpose: the library is then compiled
//! WITHOUT `#[cfg(test)]`, so the real (non-sandbox) resolvers execute exactly as
//! they do in production — the unit-test sandbox would otherwise short-circuit
//! `config_dir`/`state_dir`/`cache_dir` to a temp dir and hide a wrong one.

#![cfg(unix)]

#[path = "support/user_dirs.rs"]
mod user_dirs;

#[test]
fn harness_sandboxes_user_dirs() {
    user_dirs::assert_sandboxed();
}

use std::ffi::OsString;

fn restore(key: &str, val: Option<OsString>) {
    match val {
        // SAFETY: serialized by `test_env_lock()`.
        Some(v) => unsafe { std::env::set_var(key, v) },
        // SAFETY: serialized by `test_env_lock()`.
        None => unsafe { std::env::remove_var(key) },
    }
}

#[test]
fn every_category_resolves_to_its_xdg_dir() {
    // Serialize against any other env-mutating test in this crate.
    let _lock = sobre::core::data_dir::test_env_lock();

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let xc = tmp.path().join("config");
    let xd = tmp.path().join("data");
    let xs = tmp.path().join("state");
    let xk = tmp.path().join("cache");

    std::fs::create_dir_all(&home).unwrap();

    let keys = [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "SOBRE_DATA_DIR",
        "SOBRE_CONFIG_DIR",
        "SOBRE_STATE_DIR",
        "SOBRE_CACHE_DIR",
    ];
    let saved: Vec<(&str, Option<OsString>)> =
        keys.iter().map(|k| (*k, std::env::var_os(k))).collect();

    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("HOME", &home) };
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("XDG_CONFIG_HOME", &xc) };
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("XDG_DATA_HOME", &xd) };
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("XDG_STATE_HOME", &xs) };
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("XDG_CACHE_HOME", &xk) };
    for k in [
        "SOBRE_DATA_DIR",
        "SOBRE_CONFIG_DIR",
        "SOBRE_STATE_DIR",
        "SOBRE_CACHE_DIR",
    ] {
        // SAFETY: serialized by `test_env_lock()`.
        unsafe { std::env::remove_var(k) };
    }

    let data = sobre::core::paths::data_dir();
    let config = sobre::core::paths::config_dir();
    let state = sobre::core::paths::state_dir();
    let cache = sobre::core::paths::cache_dir();

    // Restore the environment before asserting so a failure can't leak into
    // sibling tests sharing this process.
    for (k, v) in saved {
        restore(k, v);
    }

    let data = data.expect("data dir resolves");
    let config = config.expect("config dir resolves");
    let state = state.expect("state dir resolves");
    let cache = cache.expect("cache dir resolves");

    assert_eq!(data, xd.join("sobre"), "data must split to $XDG_DATA_HOME");
    assert_eq!(
        config,
        xc.join("sobre"),
        "config must split to $XDG_CONFIG_HOME"
    );
    assert_eq!(
        state,
        xs.join("sobre"),
        "state must split to $XDG_STATE_HOME"
    );
    assert_eq!(
        cache,
        xk.join("sobre"),
        "cache must split to $XDG_CACHE_HOME"
    );
}
