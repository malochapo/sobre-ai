//! Tests for config parsing and serialization.

#[allow(unused_imports)]
use super::*;

#[cfg(test)]
mod compression_level_tests {
    use super::super::*;

    #[test]
    fn default_is_lite() {
        // Friendly default: plain-English concise guidance, not the symbolic
        // dense/expert-terse styles (those are opt-in power modes).
        assert_eq!(CompressionLevel::default(), CompressionLevel::Lite);
    }

    #[test]
    fn to_components_off() {
        let (crp, tm) = CompressionLevel::Off.to_components();
        assert_eq!(crp, "off");
        assert!(!tm);
    }

    #[test]
    fn to_components_lite() {
        let (crp, tm) = CompressionLevel::Lite.to_components();
        assert_eq!(crp, "off");
        assert!(tm);
    }

    #[test]
    fn to_components_standard() {
        let (crp, tm) = CompressionLevel::Standard.to_components();
        assert_eq!(crp, "compact");
        assert!(tm);
    }

    #[test]
    fn to_components_max() {
        let (crp, tm) = CompressionLevel::Max.to_components();
        assert_eq!(crp, "tdd");
        assert!(tm);
    }

    #[test]
    fn labels_match() {
        assert_eq!(CompressionLevel::Off.label(), "off");
        assert_eq!(CompressionLevel::Lite.label(), "lite");
        assert_eq!(CompressionLevel::Standard.label(), "standard");
        assert_eq!(CompressionLevel::Max.label(), "max");
    }

    #[test]
    fn is_active_false_for_off() {
        assert!(!CompressionLevel::Off.is_active());
    }

    #[test]
    fn is_active_true_for_all_others() {
        assert!(CompressionLevel::Lite.is_active());
        assert!(CompressionLevel::Standard.is_active());
        assert!(CompressionLevel::Max.is_active());
    }

    #[test]
    fn deserialization_defaults_to_lite() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.compression_level, 0.3);
        assert_eq!(CompressionLevel::effective(&cfg), CompressionLevel::Lite);
    }

    #[test]
    fn deserialization_from_toml() {
        let cfg: Config = toml::from_str("compression_level = 0.5").unwrap();
        assert_eq!(cfg.compression_level, 0.5);
    }
}

#[cfg(test)]
mod root_key_tests {
    use super::super::*;

    /// Regression: a stray root scalar once landed under [secret_detection]
    /// because whole-struct serialization placed it after a table. Any
    /// `Option<String>` root key is exposed to that bug; `tool_profile` and
    /// then `profile` carried the guard and are gone, so `hook_binary` does.
    #[test]
    fn option_root_scalar_serializes_above_the_first_table() {
        let original = Config {
            hook_binary: Some("$HOME/.local/bin/sobre".to_string()),
            ..Config::default()
        };
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let pos = toml_str
            .find("hook_binary")
            .expect("hook_binary should be serialized");
        let first_table = toml_str
            .find(
                "
[",
            )
            .unwrap_or(toml_str.len());
        assert!(
            pos < first_table,
            "hook_binary must be a root key, not nested under a [table]:
{toml_str}"
        );
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            parsed.hook_binary.as_deref(),
            Some("$HOME/.local/bin/sobre")
        );
    }
}
