//! GH #408 RO-config sandbox gate (brought forward from GL #607).
//!
//! With the config dir read-only and the data/state/cache categories split out
//! via `LEAN_CTX_*_DIR`, a full lean-ctx shell cycle must run without writing
//! anything into the config dir. This is the exact acceptance criterion from
//! #408 (`--ro $XDG_CONFIG_HOME/lean-ctx`).
//!
//! `config_dir()` resets its own perms to `0o700` on access, so `chmod 0o500` is
//! a best-effort RO simulation only — the real gate is the assertion that the
//! config dir's contents are byte-identical afterwards: any stray write (e.g. a
//! `stats.json` landing next to `config.toml`) fails the test. A captured agent
//! session id is additionally asserted to land in the state dir at `0o600`, while
//! a credential-shaped var (an API key) must never be persisted (finding 2).
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// Recursive content snapshot of `dir` keyed by path relative to it. Directories
/// are recorded as `"name/"` with empty content so new subdirs are detected too.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    collect(dir, dir, &mut out);
    out
}

fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        if path.is_dir() {
            out.insert(format!("{rel}/"), Vec::new());
            collect(root, &path, out);
        } else {
            out.insert(rel, std::fs::read(&path).unwrap_or_default());
        }
    }
}

#[test]
fn full_cycle_never_writes_into_readonly_config_dir() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config");
    let data = root.path().join("data");
    let state = root.path().join("state");
    let cache = root.path().join("cache");
    for d in [&config, &data, &state, &cache] {
        std::fs::create_dir_all(d).unwrap();
    }

    // The one legitimate config file. Everything else must stay out of here.
    std::fs::write(config.join("config.toml"), "ultra_compact = true\n").unwrap();
    let before = snapshot(&config);

    // Best-effort RO (the byte-identical assertion below is the real gate).
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o500)).unwrap();

    // A full shell cycle: reads config for compression, captures the forwardable
    // key into the state dir, runs the command, flushes stats into the data dir.
    let out = Command::new(env!("CARGO_BIN_EXE_lean-ctx"))
        .args(["-c", "echo lean-ctx-ro-sandbox-sentinel"])
        .env("LEAN_CTX_CONFIG_DIR", &config)
        .env("LEAN_CTX_DATA_DIR", &data)
        .env("LEAN_CTX_STATE_DIR", &state)
        .env("LEAN_CTX_CACHE_DIR", &cache)
        .env("HOME", root.path())
        // Forwardable session id → captured into the state dir (proof writes
        // land there). A credential-shaped var is also set and must NOT be
        // captured: forwarding API keys was the exfiltration risk fixed in
        // finding 2, so `is_forwardable` rejects anything credential-shaped.
        .env("CODEX_THREAD_ID", "ro-sandbox-session")
        .env("GEMINI_API_KEY", "ro-sandbox-secret")
        .output()
        .expect("spawn lean-ctx -c");

    // Restore perms so the tempdir can be cleaned up regardless of assertions.
    let _ = std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o700));

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "cycle failed: {}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("lean-ctx-ro-sandbox-sentinel"),
        "command output missing; got:\n{stdout}"
    );

    // The gate: config dir must be byte-identical — no new files, no rewrites.
    let after = snapshot(&config);
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>(),
        "a file leaked into the read-only config dir"
    );
    assert_eq!(
        before, after,
        "the config dir was modified during the cycle"
    );

    // Proof that writes landed in the split categories: the captured session id
    // lives in the state dir, owner-only — never the RO/shareable config dir.
    let key_file = state.join("agent_runtime_env.json");
    assert!(
        key_file.exists(),
        "captured session vars must be written to the state dir, not the config dir"
    );
    let captured = std::fs::read_to_string(&key_file).unwrap();
    assert!(
        captured.contains("CODEX_THREAD_ID"),
        "the forwardable session id must be captured; got:\n{captured}"
    );
    // Finding 2: credential-shaped vars must never be persisted, even when they
    // match a forwardable prefix (GEMINI_ here).
    assert!(
        !captured.contains("ro-sandbox-secret") && !captured.contains("GEMINI_API_KEY"),
        "credential-shaped vars must never be written to disk; got:\n{captured}"
    );
    let mode = std::fs::metadata(&key_file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "captured key file must be owner-only");
}
