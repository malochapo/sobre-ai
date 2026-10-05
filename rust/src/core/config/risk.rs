//! Risk classification for `config set` governance (#852).
//!
//! Most config keys are routine (cache sizes, density, theme) and are written
//! without friction. A small set, however, changes lean-ctx's *security posture*
//! (containment, secret redaction) or *network routing / data egress* (upstream
//! redirects). Flipping one of those silently can weaken the user's machine or
//! leak credentials to a provider, so `config set` shows a before→after review
//! and requires confirmation (or `--yes`) before applying — mirroring the
//! existing `yolo` / `secure` confirmation pattern.
//!
//! This is a deterministic, local-only lookup: no telemetry, no heuristics.

/// A consequential config key and the one-line note explaining what changing it
/// does. Returned by [`classify`]; `None` ⇒ a routine key, written directly.
pub struct ConfigRisk {
    /// Human-readable consequence of changing this key, shown in the review.
    pub note: &'static str,
}

/// Classifies a fully-qualified config key (dot-path, e.g. `proxy.upstreams.openai`).
///
/// Returns a [`ConfigRisk`] for keys whose change is security- or
/// egress-relevant; `None` for everything else.
#[must_use]
pub fn classify(key: &str) -> Option<ConfigRisk> {
    let note = match key {
        "allow_paths" => {
            "allow_paths widens the path jail: tools may read and write these paths too, and [\"/\"] opens it entirely."
        }
        key if key.starts_with("proxy.upstreams.") => {
            "Redirects provider traffic to a custom upstream — every request and API key for this provider will flow through it."
        }
        _ => return None,
    };
    Some(ConfigRisk { note })
}

/// True if changing `key` is consequential enough to require a review.
#[must_use]
#[cfg(test)]
pub fn is_consequential(key: &str) -> bool {
    classify(key).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_keys_are_consequential() {
        for key in [
            "allow_paths",
            "proxy.upstreams.openai",
            "proxy.upstreams.anthropic",
            "proxy.upstreams.gemini",
        ] {
            assert!(is_consequential(key), "{key} should be consequential");
            assert!(!classify(key).unwrap().note.is_empty());
        }
    }

    #[test]
    fn routine_keys_are_not_consequential() {
        for key in ["compression_level", "proxy.port"] {
            assert!(!is_consequential(key), "{key} should be routine");
        }
    }
}
