//! Secret detection: which secret shapes a text holds, and the same text with
//! them masked.
//!
//! [`scan_and_redact`] masks everything [`crate::core::redaction::redact_text`]
//! masks, plus quoted key/value secrets of 20 characters or more; what it
//! returns alongside names each shape it found.

use std::sync::OnceLock;

use regex::Regex;

macro_rules! static_regex {
    ($pattern:expr_2021) => {{
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new($pattern).expect(concat!("invalid regex: ", $pattern)))
    }};
}

#[derive(Debug, Clone)]
pub struct SecretMatch {
    pub pattern_name: &'static str,
}

fn aws_key_re() -> &'static Regex {
    static_regex!(r"\bAKIA[0-9A-Z]{16}")
}

/// The BEGIN line of any PEM or PGP private key.
pub(crate) fn private_key_re() -> &'static Regex {
    static_regex!(r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----")
}

fn github_token_re() -> &'static Regex {
    static_regex!(r"\bgh[ps]_[A-Za-z0-9_]{36,}")
}

fn anthropic_key_re() -> &'static Regex {
    static_regex!(r"\bsk-ant-[A-Za-z0-9_\-]{20,}")
}

fn openai_key_re() -> &'static Regex {
    static_regex!(r"\bsk-[A-Za-z0-9]{20,}")
}

// Word boundaries for the generic key/value patterns: the keyword must
// not be a camelCase subword (`superuserPassword`), while SNAKE_CASE env
// names (`MY_TOKEN=…`) keep matching — `_` stays a permitted predecessor.
// Group 1 = predecessor + key prefix (kept on redaction), group 2 = value
// (guarded: identifier references and placeholders are benign).
fn generic_api_key_re() -> &'static Regex {
    static_regex!(
        r#"(?im)((?:^|[^a-z0-9])(?:api[_-]?key|secret[_-]?key|token|password|passwd|access[_-]?token|client[_-]?secret)\s*[=:]\s*)(['"]?[a-zA-Z0-9_\-]{20,})"#
    )
}

fn high_entropy_b64_re() -> &'static Regex {
    static_regex!(
        r#"(?im)((?:^|[^a-z0-9])(?:key|token|secret|password|credential|auth)\s*[=:]\s*)(['"]?[A-Za-z0-9+/=\-_]{40,})"#
    )
}

fn gitlab_pat_re() -> &'static Regex {
    static_regex!(r"\bglpat-[A-Za-z0-9_\-]{20,}")
}

fn jwt_re() -> &'static Regex {
    static_regex!(r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}")
}

fn slack_token_re() -> &'static Regex {
    static_regex!(r"\bxox[bpas]-[0-9a-zA-Z\-]{10,}")
}

fn stripe_key_re() -> &'static Regex {
    static_regex!(r"\b[sr]k_live_[0-9a-zA-Z]{10,}")
}

fn db_url_re() -> &'static Regex {
    static_regex!(r"\b(?:postgres|mysql|mongodb|redis)://[^\s]+:[^\s]+@")
}

fn npm_token_re() -> &'static Regex {
    static_regex!(r"\bnpm_[A-Za-z0-9]{10,}")
}

fn github_fine_grained_re() -> &'static Regex {
    static_regex!(r"\bgithub_pat_[A-Za-z0-9_]{20,}")
}

/// Secrets recognised by their own shape (a vendor prefix, a URL with
/// credentials), whatever key they are assigned to: `(name, label, regex)`.
/// The whole match is the secret. [`crate::core::redaction::redact_text`]
/// masks each one as `[REDACTED:<label>]`.
pub(crate) const TOKEN_SHAPES: &[(&str, &str, fn() -> &'static Regex)] = &[
    (
        "github_fine_grained",
        "GitHub token",
        github_fine_grained_re,
    ),
    ("anthropic_key", "Anthropic key", anthropic_key_re),
    ("openai_key", "OpenAI key", openai_key_re),
    ("gitlab_pat", "GitLab token", gitlab_pat_re),
    ("jwt", "JWT", jwt_re),
    ("slack_token", "Slack token", slack_token_re),
    ("stripe_key", "Stripe key", stripe_key_re),
    ("db_url", "Database URL credentials", db_url_re),
    ("npm_token", "npm token", npm_token_re),
];

/// Shapes reported by name; `redact_text` masks each of them.
const DETECTED_PATTERNS: &[(&str, fn() -> &'static Regex)] = &[
    ("aws_key", aws_key_re),
    ("private_key", private_key_re),
    ("github_token", github_token_re),
];

/// Key/value patterns with a kept prefix (group 1) and a guarded value
/// (group 2): identifier references, placeholders and language literals are
/// benign and neither reported nor redacted.
const GUARDED_PATTERNS: &[(&str, fn() -> &'static Regex)] = &[
    ("generic_api_key", generic_api_key_re),
    ("high_entropy_secret", high_entropy_b64_re),
];

/// True when a guarded-pattern match should be skipped: the value is benign
/// — shares the benign-value heuristics with `core::redaction` so the
/// two layers cannot drift.
fn guarded_match_is_benign(caps: &regex::Captures) -> bool {
    caps.get(2)
        .is_some_and(|value| crate::core::redaction::is_benign_secret_value(value.as_str()))
}

fn collect_matches(content: &str) -> Vec<SecretMatch> {
    let mut matches = Vec::new();

    let named = DETECTED_PATTERNS
        .iter()
        .copied()
        .chain(TOKEN_SHAPES.iter().map(|&(name, _, re)| (name, re)));
    for (name, regex_fn) in named {
        for _ in regex_fn().find_iter(content) {
            matches.push(SecretMatch { pattern_name: name });
        }
    }

    for &(name, regex_fn) in GUARDED_PATTERNS {
        for caps in regex_fn().captures_iter(content) {
            if !guarded_match_is_benign(&caps) {
                matches.push(SecretMatch { pattern_name: name });
            }
        }
    }

    matches
}

pub fn detect_secrets(content: &str) -> Vec<SecretMatch> {
    collect_matches(content)
}

/// Detects secrets in `content` and masks them: everything
/// [`crate::core::redaction::redact_text`] masks, then the guarded key/value
/// patterns as `[REDACTED:<pattern>]`.
pub fn scan_and_redact(content: &str) -> (String, Vec<SecretMatch>) {
    let matches = collect_matches(content);
    let mut redacted = crate::core::redaction::redact_text(content);
    for &(name, regex_fn) in GUARDED_PATTERNS {
        let re = regex_fn();
        redacted = re
            .replace_all(&redacted, |caps: &regex::Captures| {
                let whole = caps.get(0).map_or("", |m| m.as_str());
                if guarded_match_is_benign(caps) {
                    return whole.to_string();
                }
                let prefix = caps.get(1).map_or("", |m| m.as_str());
                format!("{prefix}[REDACTED:{name}]")
            })
            .to_string();
    }

    (redacted, matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_aws_key() {
        let input = "aws_key = AKIAIOSFODNN7EXAMPLE";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "aws_key"));
    }

    #[test]
    fn detects_private_key_header() {
        let input = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIB...";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "private_key"));
    }

    #[test]
    fn detects_github_token() {
        let input = "export GITHUB_TOKEN=ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijkl";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "github_token"));
    }

    #[test]
    fn detects_anthropic_key() {
        let input = "ANTHROPIC_API_KEY=sk-ant-api03-abcdef1234567890ABCD";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "anthropic_key"));
    }

    #[test]
    fn detects_openai_key() {
        let input = "OPENAI_API_KEY=sk-abcdefghijklmnopqrstuvwx";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "openai_key"));
    }

    #[test]
    fn detects_gitlab_pat() {
        let input = "token = glpat-xxxxxxxxxxxxxxxxxxxx";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "gitlab_pat"));
    }

    #[test]
    fn detects_generic_api_key() {
        let input = "api_key = abcdefghijklmnopqrstuvwxyz1234567890";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(
            |m| m.pattern_name == "generic_api_key" || m.pattern_name == "high_entropy_secret"
        ));
    }

    #[test]
    fn clean_content_returns_empty() {
        let input = "fn main() { println!(\"hello world\"); }";
        let matches = detect_secrets(input);
        assert!(matches.is_empty());
    }

    #[test]
    fn detects_jwt() {
        let input = "token = eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkw";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "jwt"));
    }

    #[test]
    fn detects_slack_token() {
        let input = "SLACK_TOKEN=xoxb-1234567890-abcdefghij";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "slack_token"));
    }

    #[test]
    fn detects_stripe_key() {
        let input = "stripe_key = sk_live_abcdefghij1234567890";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "stripe_key"));
    }

    #[test]
    fn detects_db_url() {
        let input = "DATABASE_URL=postgres://user:password@localhost:5432/db";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "db_url"));
    }

    #[test]
    fn detects_npm_token() {
        let input = "NPM_TOKEN=npm_abcdefghij1234567890";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "npm_token"));
    }

    #[test]
    fn detects_github_fine_grained() {
        let input = "token = github_pat_ABCDEFGHIJKLMNOPQRSTuvwx";
        let matches = detect_secrets(input);
        assert!(
            matches
                .iter()
                .any(|m| m.pattern_name == "github_fine_grained")
        );
    }

    #[test]
    fn scan_and_redact_replaces_secrets() {
        let input = "key = AKIAIOSFODNN7EXAMPLE";
        let (redacted, matches) = scan_and_redact(input);
        assert!(!matches.is_empty());
        assert!(redacted.contains("[REDACTED:AWS key]"));
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn multiple_secrets_on_different_lines() {
        let input = "AKIAIOSFODNN7EXAMPLE\nclean\nsk-abcdefghijklmnopqrstuvwx";
        let matches = detect_secrets(input);
        assert!(matches.len() >= 2);
        assert!(matches.iter().any(|m| m.pattern_name == "aws_key"));
        assert!(matches.iter().any(|m| m.pattern_name == "openai_key"));
    }

    /// The whole key is masked, not only its header line.
    #[test]
    fn scan_and_redact_masks_the_private_key_body() {
        let input = "$ cat id_ed25519\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----\n$ ls";
        let (out, matches) = scan_and_redact(input);
        assert!(matches.iter().any(|m| m.pattern_name == "private_key"));
        assert!(!out.contains("b3BlbnNzaC1rZXktdjEAAAAA"), "{out}");
        assert!(out.starts_with("$ cat id_ed25519\n"), "{out}");
        assert!(out.ends_with("\n$ ls"), "{out}");
    }

    /// A known token prefix inside a longer word is not a token.
    #[test]
    fn token_prefixes_need_a_word_boundary() {
        let input = "kiosk-internationalization and task-abcdefghijklmnopqrstuvwxyz";
        assert!(detect_secrets(input).is_empty(), "{input}");
        assert_eq!(scan_and_redact(input).0, input);
    }

    #[test]
    fn ec_private_key_detected() {
        let input = "-----BEGIN EC PRIVATE KEY-----";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "private_key"));
    }

    #[test]
    fn openssh_private_key_detected() {
        let input = "-----BEGIN OPENSSH PRIVATE KEY-----";
        let matches = detect_secrets(input);
        assert!(matches.iter().any(|m| m.pattern_name == "private_key"));
    }
}
