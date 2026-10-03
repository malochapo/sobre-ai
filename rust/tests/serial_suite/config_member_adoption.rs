//! `config_dir_member` adopts a member that older builds wrote under the
//! OS-native config dir — but never into a config dir the environment fixed.
//! Integration tests run with such a dir, and on Windows the OS-native dir
//! ignores the environment, so adopting would move a developer's real files.
//! Linux only: the OS-native dir follows `XDG_CONFIG_HOME` there, so the test
//! can place the legacy member in a temp dir.
#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::path::Path;

/// Sets `vars` for the guard's lifetime and restores the previous values.
struct EnvVars(Vec<(&'static str, Option<OsString>)>);

impl EnvVars {
    fn set(vars: &[(&'static str, &Path)]) -> Self {
        let saved = vars
            .iter()
            .map(|(k, v)| {
                let old = std::env::var_os(k);
                // SAFETY: the serial binary runs one test at a time.
                unsafe { std::env::set_var(k, v) };
                (*k, old)
            })
            .collect();
        Self(saved)
    }
}

impl Drop for EnvVars {
    fn drop(&mut self) {
        for (k, old) in &self.0 {
            // SAFETY: the serial binary runs one test at a time.
            match old {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

#[test]
fn explicit_data_dir_leaves_os_native_member_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    // Non-standard, so it holds config as well as data.
    let data = tmp.path().join("pinned");
    let xdg_config = tmp.path().join("xdg-config");
    let legacy = xdg_config.join("lean-ctx").join("multi-repo.toml");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "# legacy member\n").unwrap();

    let _env = EnvVars::set(&[
        ("LEAN_CTX_DATA_DIR", &data),
        ("XDG_CONFIG_HOME", &xdg_config),
    ]);
    let canonical = lean_ctx::core::paths::config_dir_member("multi-repo.toml").unwrap();

    assert_eq!(canonical, data.join("multi-repo.toml"));
    assert!(
        legacy.exists(),
        "the OS-native member must stay where it is when the config dir is explicit"
    );
    assert!(
        !canonical.exists(),
        "nothing may be moved into the explicit config dir"
    );
}
