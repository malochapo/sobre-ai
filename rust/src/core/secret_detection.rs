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
    pub line_number: usize,
    pub redacted_preview: String,
}

fn aws_key_re() -> &'static Regex {
    static_regex!(r"AKIA[0-9A-Z]{16}")
}

fn private_key_re() -> &'static Regex {
    static_regex!(r"-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----")
}

fn github_token_re() -> &'static Regex {
    static_regex!(r"gh[ps]_[A-Za-z0-9_]{36,}")
}

fn anthropic_key_re() -> &'static Regex {
    static_regex!(r"sk-ant-[A-Za-z0-9_\-]{20,}")
}

fn openai_key_re() -> &'static Regex {
    static_regex!(r"sk-[A-Za-z0-9]{20,}")
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
    static_regex!(r"glpat-[A-Za-z0-9_\-]{20,}")
}

fn jwt_re() -> &'static Regex {
    static_regex!(r"eyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}")
}

fn slack_token_re() -> &'static Regex {
    static_regex!(r"xox[bpas]-[0-9a-zA-Z\-]{10,}")
}

fn stripe_key_re() -> &'static Regex {
    static_regex!(r"[sr]k_live_[0-9a-zA-Z]{10,}")
}

fn db_url_re() -> &'static Regex {
    static_regex!(r"(?:postgres|mysql|mongodb|redis)://[^\s]+:[^\s]+@")
}

fn npm_token_re() -> &'static Regex {
    static_regex!(r"npm_[A-Za-z0-9]{10,}")
}

fn github_fine_grained_re() -> &'static Regex {
    static_regex!(r"github_pat_[A-Za-z0-9_]{20,}")
}

const BUILTIN_PATTERNS: &[(&str, fn() -> &'static Regex)] = &[
    ("aws_key", aws_key_re),
    ("private_key", private_key_re),
    ("github_token", github_token_re),
    ("github_fine_grained", github_fine_grained_re),
    ("anthropic_key", anthropic_key_re),
    ("openai_key", openai_key_re),
    ("gitlab_pat", gitlab_pat_re),
    ("jwt", jwt_re),
    ("slack_token", slack_token_re),
    ("stripe_key", stripe_key_re),
    ("db_url", db_url_re),
    ("npm_token", npm_token_re),
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

fn make_redacted_preview(matched: &str) -> String {
    let chars: Vec<char> = matched.chars().collect();
    if chars.len() <= 6 {
        return "***".to_string();
    }
    let prefix: String = chars[..4].iter().collect();
    let suffix: String = chars[chars.len() - 2..].iter().collect();
    format!("{prefix}***{suffix}")
}

fn collect_matches(content: &str) -> Vec<SecretMatch> {
    let mut matches = Vec::new();

    let line_offsets: Vec<usize> = std::iter::once(0)
        .chain(content.match_indices('\n').map(|(i, _)| i + 1))
        .collect();

    let offset_to_line = |byte_offset: usize| -> usize {
        match line_offsets.binary_search(&byte_offset) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    };

    for &(name, regex_fn) in BUILTIN_PATTERNS {
        let re = regex_fn();
        for m in re.find_iter(content) {
            matches.push(SecretMatch {
                pattern_name: name,
                line_number: offset_to_line(m.start()),
                redacted_preview: make_redacted_preview(m.as_str()),
            });
        }
    }

    for &(name, regex_fn) in GUARDED_PATTERNS {
        let re = regex_fn();
        for caps in re.captures_iter(content) {
            let whole = caps.get(0).map_or("", |m| m.as_str());
            if guarded_match_is_benign(&caps) {
                continue;
            }
            let start = caps.get(0).map_or(0, |m| m.start());
            matches.push(SecretMatch {
                pattern_name: name,
                line_number: offset_to_line(start),
                redacted_preview: make_redacted_preview(whole),
            });
        }
    }

    matches
}

pub fn detect_secrets(content: &str) -> Vec<SecretMatch> {
    collect_matches(content)
}

/// Detects secrets in `content` and masks each one as `[REDACTED:<pattern>]`.
pub fn scan_and_redact(content: &str) -> (String, Vec<SecretMatch>) {
    let matches = collect_matches(content);
    if matches.is_empty() {
        return (content.to_string(), matches);
    }

    let mut redacted = content.to_string();
    for &(name, regex_fn) in BUILTIN_PATTERNS {
        redacted = regex_fn()
            .replace_all(&redacted, format!("[REDACTED:{name}]").as_str())
            .to_string();
    }

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
    fn redacted_preview_format() {
        let preview = make_redacted_preview("AKIAIOSFODNN7EXAMPLE");
        assert!(preview.starts_with("AKIA"));
        assert!(preview.ends_with("LE"));
        assert!(preview.contains("***"));
    }

    #[test]
    fn redacted_preview_short_string() {
        let preview = make_redacted_preview("short");
        assert_eq!(preview, "***");
    }

    #[test]
    fn scan_and_redact_replaces_secrets() {
        let input = "key = AKIAIOSFODNN7EXAMPLE";
        let (redacted, matches) = scan_and_redact(input);
        assert!(!matches.is_empty());
        assert!(redacted.contains("[REDACTED:aws_key]"));
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn line_numbers_are_correct() {
        let input = "line1\nline2\nAKIAIOSFODNN7EXAMPLE\nline4";
        let matches = detect_secrets(input);
        assert!(!matches.is_empty());
        assert_eq!(matches[0].line_number, 3);
    }

    #[test]
    fn multiple_secrets_on_different_lines() {
        let input = "AKIAIOSFODNN7EXAMPLE\nclean\nsk-abcdefghijklmnopqrstuvwx";
        let matches = detect_secrets(input);
        assert!(matches.len() >= 2);
        let aws = matches
            .iter()
            .find(|m| m.pattern_name == "aws_key")
            .unwrap();
        assert_eq!(aws.line_number, 1);
        let oai = matches
            .iter()
            .find(|m| m.pattern_name == "openai_key")
            .unwrap();
        assert_eq!(oai.line_number, 3);
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
