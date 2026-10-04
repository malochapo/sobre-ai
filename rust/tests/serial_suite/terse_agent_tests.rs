use lean_ctx::core::config::{CompressionLevel, Config, TerseAgent};
use lean_ctx::instructions;
use lean_ctx::tools::CrpMode;

/// Points every lean-ctx dir at a fresh temp dir holding `config.toml` with the
/// given contents, restoring the previous env on drop. The serial harness runs
/// one test at a time, so mutating the process env here is race-free.
struct ConfigGuard {
    _tmp: tempfile::TempDir,
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

const ISOLATED_ENV_VARS: [&str; 2] = ["LEAN_CTX_DATA_DIR", "LEAN_CTX_CONFIG_DIR"];

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

fn set_compression(compression: &str) -> ConfigGuard {
    with_config(&format!("compression_level = \"{compression}\"\n"))
}

fn set_legacy_terse(terse: &str) -> ConfigGuard {
    with_config(&format!(
        "compression_level = \"off\"\nterse_agent = \"{terse}\"\n"
    ))
}

// ── Legacy `terse_agent` config routes through CompressionLevel ──

#[test]
fn legacy_terse_lite_routes_to_compression_lite() {
    let _g = set_legacy_terse("lite");
    let text = instructions::build_instructions_for_test(CrpMode::Off);
    assert!(
        text.contains("OUTPUT STYLE: concise"),
        "legacy terse lite should route to compression lite prompt"
    );
}

#[test]
fn legacy_terse_full_routes_to_compression_standard() {
    let _g = set_legacy_terse("full");
    let text = instructions::build_instructions_for_test(CrpMode::Off);
    assert!(
        text.contains("OUTPUT STYLE: dense"),
        "legacy terse full should route to compression standard prompt"
    );
}

#[test]
fn legacy_terse_ultra_routes_to_compression_max() {
    let _g = set_legacy_terse("ultra");
    let text = instructions::build_instructions_for_test(CrpMode::Off);
    assert!(
        text.contains("OUTPUT STYLE: expert-terse"),
        "legacy terse ultra should route to compression max prompt"
    );
}

#[test]
fn legacy_terse_off_no_output_style() {
    let _g = set_legacy_terse("off");
    let text = instructions::build_instructions_for_test(CrpMode::Off);
    assert!(
        !text.contains("OUTPUT STYLE"),
        "terse off should not inject OUTPUT STYLE block"
    );
}

// ── Unified `compression_level` config key ──

#[test]
fn compression_level_overrides_legacy_terse_agent() {
    let _g = with_config("compression_level = \"max\"\nterse_agent = \"lite\"\n");
    let text = instructions::build_instructions_for_test(CrpMode::Off);
    assert!(
        text.contains("expert-terse"),
        "compression_level should override legacy terse_agent"
    );
}

#[test]
fn compression_level_lite_injects_concise() {
    let _g = set_compression("lite");
    let text = instructions::build_instructions_for_test(CrpMode::Off);
    assert!(
        text.contains("OUTPUT STYLE: concise"),
        "compression lite should inject concise prompt"
    );
}

#[test]
fn compression_level_standard_injects_dense() {
    let _g = set_compression("standard");
    let text = instructions::build_instructions_for_test(CrpMode::Off);
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
    let _g = set_compression("max");
    let text = instructions::build_instructions_for_test(CrpMode::Off);
    assert!(
        text.contains("OUTPUT STYLE: expert-terse"),
        "compression max should inject expert-terse prompt"
    );
}

#[test]
fn compression_level_off_no_output_style() {
    let _g = set_compression("off");
    let text = instructions::build_instructions_for_test(CrpMode::Off);
    assert!(
        !text.contains("OUTPUT STYLE"),
        "compression off should not inject any OUTPUT STYLE block"
    );
}

// ── Config deserialization ──

#[test]
fn terse_agent_config_deserializes() {
    let toml = r#"
terse_agent = "full"
"#;
    let config: Config = toml::from_str(toml).expect("should parse terse_agent from toml");
    assert!(matches!(config.terse_agent, TerseAgent::Full));
}

#[test]
fn terse_agent_config_default_off() {
    let toml = "";
    let config: Config = toml::from_str(toml).expect("empty toml should use defaults");
    assert!(matches!(config.terse_agent, TerseAgent::Off));
}

#[test]
fn compression_level_config_deserializes() {
    let toml = r#"compression_level = "standard""#;
    let config: Config = toml::from_str(toml).expect("should parse compression_level");
    assert!(matches!(
        config.compression_level,
        CompressionLevel::Standard
    ));
}

#[test]
fn compression_level_config_default_lite() {
    let toml = "";
    let config: Config = toml::from_str(toml).expect("empty toml should use defaults");
    assert!(matches!(config.compression_level, CompressionLevel::Lite));
}
