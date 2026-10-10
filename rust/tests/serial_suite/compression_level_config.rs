//! `compression_level`: how the file value parses, which band it selects, and
//! that no band changes the MCP instructions.

use sobre::core::config::{CompressionLevel, Config};
use sobre::instructions;

/// Points every sobre dir at a fresh temp dir holding `config.toml` with the
/// given contents, restoring the previous env on drop. The serial harness runs
/// one test at a time, so mutating the process env here is race-free.
struct ConfigGuard {
    _tmp: tempfile::TempDir,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

const ISOLATED_ENV_VARS: [&str; 2] = ["SOBRE_DATA_DIR", "SOBRE_CONFIG_DIR"];

fn with_config(toml: &str) -> ConfigGuard {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("config.toml"), toml).expect("write config.toml");
    let prev = ISOLATED_ENV_VARS
        .iter()
        .map(|k| (*k, std::env::var_os(k)))
        .collect();
    for k in ISOLATED_ENV_VARS {
        // SAFETY: the serial harness runs one test at a time.
        unsafe { std::env::set_var(k, tmp.path()) };
    }
    ConfigGuard { _tmp: tmp, prev }
}

impl Drop for ConfigGuard {
    fn drop(&mut self) {
        for (k, v) in &self.prev {
            // SAFETY: the serial harness runs one test at a time.
            match v {
                Some(v) => unsafe { std::env::set_var(k, v) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
    }
}

fn set_compression(value: f64) -> ConfigGuard {
    with_config(&format!("compression_level = {value}\n"))
}

// ── The instructions do not depend on the level ──

/// The instructions are one text for every configuration (invariant 3):
/// `compression_level` condenses tool output and descriptions, and never
/// asks the model to write its own replies differently.
#[test]
fn no_compression_level_changes_the_instructions() {
    let baseline = {
        let _g = set_compression(0.3);
        instructions::build_instructions()
    };
    for value in [0.0, 0.5, 0.65, 0.7, 1.0] {
        let _g = set_compression(value);
        let text = instructions::build_instructions();
        assert_eq!(text, baseline, "compression_level = {value}");
        assert!(!text.contains("OUTPUT STYLE"), "compression_level = {value}");
    }
}

// ── Config deserialization ──

#[test]
fn compression_level_reads_as_band() {
    let config: Config = toml::from_str("compression_level = 0.5").expect("number parses");
    assert_eq!(config.compression_level, 0.5);
    assert_eq!(
        CompressionLevel::effective(&config),
        CompressionLevel::Standard
    );
}

#[test]
fn compression_level_number_is_clamped() {
    let config: Config = toml::from_str("compression_level = 1.7").expect("number parses");
    assert_eq!(config.compression_level, 1.0);
}

#[test]
fn compression_level_name_is_a_parse_error() {
    assert!(toml::from_str::<Config>(r#"compression_level = "standard""#).is_err());
}

#[test]
fn compression_level_absent_means_the_default_lite() {
    let config: Config = toml::from_str("").expect("empty toml should use defaults");
    assert_eq!(config.compression_level, 0.3);
    assert_eq!(CompressionLevel::effective(&config), CompressionLevel::Lite);
}

#[test]
fn compression_level_bands() {
    for (value, level) in [
        (0.0, CompressionLevel::Off),
        (0.19, CompressionLevel::Off),
        (0.2, CompressionLevel::Lite),
        (0.45, CompressionLevel::Standard),
        (0.6, CompressionLevel::Max),
        (0.8, CompressionLevel::Raw),
        (1.0, CompressionLevel::Raw),
    ] {
        assert_eq!(CompressionLevel::from_value(value), level, "{value}");
    }
}
