//! `compression_level`: how the file value parses, which band it selects, and
//! the output style the MCP instructions then ask of the model.

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

// ── The output style the instructions ask for ──

#[test]
fn compression_level_lite_injects_concise() {
    let _g = set_compression(0.3);
    let text = instructions::build_instructions();
    assert!(
        text.contains("OUTPUT STYLE: concise"),
        "compression lite should inject concise prompt"
    );
}

#[test]
fn compression_level_standard_injects_dense() {
    let _g = set_compression(0.5);
    let text = instructions::build_instructions();
    assert!(
        text.contains("OUTPUT STYLE: dense"),
        "compression standard should inject dense prompt"
    );
    assert!(
        text.contains("fn, cfg, impl"),
        "compression standard should mention abbreviations"
    );
}

#[test]
fn compression_level_max_injects_expert_terse() {
    let _g = set_compression(0.7);
    let text = instructions::build_instructions();
    assert!(
        text.contains("OUTPUT STYLE: expert-terse"),
        "compression max should inject expert-terse prompt"
    );
}

#[test]
fn compression_level_off_no_output_style() {
    let _g = set_compression(0.0);
    let text = instructions::build_instructions();
    assert!(
        !text.contains("OUTPUT STYLE"),
        "compression off should not inject any OUTPUT STYLE block"
    );
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

#[test]
fn compression_level_number_selects_prose_band() {
    let _g = with_config("compression_level = 0.65\n");
    let text = instructions::build_instructions();
    assert!(
        text.contains("OUTPUT STYLE: expert-terse"),
        "0.65 falls in the max band"
    );
}
