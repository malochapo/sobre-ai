macro_rules! static_regex {
    ($pattern:expr_2021) => {{
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| {
            regex::Regex::new($pattern).expect(concat!("BUG: invalid static regex: ", $pattern))
        })
    }};
}

/// Unquoted identifier or property-access chains (`SvelteKit`,
/// `inputEnv.POCKETBASE_SUPERUSER_PASSWORD`, `serverEnv.getStripeSecretKey`,
/// `confirmRequiredEndpointKeySchema`) are code REFERENCES to a secret, never
/// the literal value — the value lives in a gitignored `.env`. Digits make a
/// token secret-shaped (base64/hex), so any digit keeps the redaction
/// (conservative: `password=hunter2` stays covered).
/// Detect pure numeric values — integers, floats, scientific notation.
/// These are never secrets even when the key contains `token` or `key`.
fn looks_like_number(v: &str) -> bool {
    if v.is_empty() {
        return false;
    }
    let s = v.trim_start_matches(['+', '-']);
    if s.is_empty() {
        return false;
    }
    // Integer, float, or scientific notation (1.4e-06, 0.5, 600, 1e10)
    s.parse::<f64>().is_ok()
        && s.chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || c == '+' || c == '-')
}

/// Env-variable reference patterns that should not be redacted.
/// Matches: `os.environ/NAME`, `os.getenv("NAME")`, `process.env.NAME`,
/// `inputEnv.NAME`, `${NAME}`, `$NAME`, `%NAME%` (Windows).
fn is_env_reference(v: &str) -> bool {
    if v.starts_with("os.environ/")
        || v.starts_with("os.getenv(")
        || v.starts_with("process.env.")
        || v.starts_with("System.getenv(")
        || v.starts_with("ENV[")
        || v.starts_with("env(")
    {
        return true;
    }
    // Variable interpolation: ${VAR}, $VAR, %VAR%
    if (v.starts_with("${") && v.ends_with('}'))
        || (v.starts_with('$')
            && v[1..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'))
        || (v.starts_with('%') && v.ends_with('%') && v.len() > 2)
    {
        return true;
    }
    // Dotted identifier chains with an env-like prefix
    if let Some(prefix) = v.split('.').next() {
        let pl = prefix.to_ascii_lowercase();
        if matches!(
            pl.as_str(),
            "env" | "inputenv" | "serverenv" | "secrets" | "vars" | "environ"
        ) && v.contains('.')
        {
            return true;
        }
    }
    false
}

fn is_identifier_reference(value: &str) -> bool {
    let v = value.trim();
    if v.is_empty() || v.starts_with('"') || v.starts_with('\'') || v.starts_with('`') {
        return false;
    }
    // Env-variable reference patterns are not secrets.
    // `os.environ/MY_SERVICE_API_KEY`, `process.env.NAME`, `${VAR}`, `$VAR`.
    if is_env_reference(v) {
        return true;
    }
    if v.contains(|c: char| c.is_ascii_digit()) {
        return false;
    }
    v.split('.').all(|segment| {
        let mut chars = segment.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
    })
}

/// Obvious placeholder/example values (`ghp_change_me`, `your_key_here`,
/// `<insert-token>`) are documentation, not secrets — `.env.example` files
/// must survive sobre_read verbatim.
fn is_placeholder_value(value: &str) -> bool {
    let v = value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_ascii_lowercase();
    if v.starts_with('<') && v.ends_with('>') {
        return true;
    }
    const MARKERS: &[&str] = &[
        "change_me",
        "change-me",
        "changeme",
        "example",
        "placeholder",
        "your_",
        "your-",
        "xxx",
        "dummy",
        "sample",
        "todo",
        "fixme",
        "replace_me",
        "replace-me",
    ];
    MARKERS.iter().any(|m| v.contains(m))
}

/// Right-hand sides that look like `key: value` but are obviously not secrets:
/// TypeScript type annotations and language literals. Redacting these corrupts
/// source files read through `sobre_read`, so the key/value rules skip
/// them. Compared case-insensitively after trimming surrounding quotes.
fn is_non_secret_literal(value: &str) -> bool {
    let v = value
        .trim()
        .trim_matches(|c| c == '"' || c == '\'' || c == '`');
    // Pure numbers (integer, float, scientific notation) are never secrets.
    // `input_cost_per_token: 1.4e-06` must not be redacted.
    if looks_like_number(v) {
        return true;
    }
    // Type expressions are never flat secret tokens: real keys/tokens are drawn
    // from `[A-Za-z0-9+/=_-]`, whereas type annotations carry angle brackets,
    // unions, arrays or call/object syntax. `password: Promise<string>` and
    // `apiKey: Record<string, unknown>` must survive sobre_read verbatim.
    if v.contains(['<', '>', '|', '(', ')', '[', ']', '{', '}']) {
        return true;
    }
    matches!(
        v.to_ascii_lowercase().as_str(),
        "" | "undefined"
            | "null"
            | "none"
            | "nil"
            | "true"
            | "false"
            | "string"
            | "number"
            | "boolean"
            | "bigint"
            | "symbol"
            | "object"
            | "any"
            | "unknown"
            | "never"
            | "void"
            | "nan"
            | "date"
    )
}

/// One redaction rule: a labelled regex plus how the match is rebuilt.
struct Rule {
    label: &'static str,
    re: &'static regex::Regex,
    /// When set, group 1 is a prefix to keep and group 2 is the secret value;
    /// the match is left untouched if that value is a non-secret literal
    /// (`password: undefined`), an identifier reference
    /// (`serverEnv.getStripeSecretKey`) or a placeholder (`ghp_change_me`) —
    /// see `is_benign_secret_value`.
    guard_value: bool,
}

/// Combined benign-value check for key/value secret rules:
/// language literals and type annotations, unquoted identifier/property
/// references, and documentation placeholders are never redacted. Quoted
/// string values stay protected — they ARE literal values.
pub(crate) fn is_benign_secret_value(value: &str) -> bool {
    is_comparison_operator_residue(value)
        || is_non_secret_literal(value)
        || is_identifier_reference(value)
        || is_placeholder_value(value)
}

/// When the regex `key=` consumes one `=` from `==`, the captured
/// "value" is a bare `=` (or `==`, `!=`). A comparison operator is never
/// a secret; redacting it corrupts source semantics.
fn is_comparison_operator_residue(value: &str) -> bool {
    let v = value.trim();
    matches!(v, "=" | "==" | "!=" | "<=" | ">=" | "===" | "!==")
}

/// The single source of truth for secret patterns. `shell::redact` delegates
/// here so the two layers can never drift apart again.
///
/// Word boundaries: the key/value alternations start with
/// `(?:^|[^a-z0-9])` (consumed into the kept prefix — the regex crate has no
/// lookbehind) so camelCase subwords (`superuserPassword`,
/// `getStripeSecretKey`) never trigger a rule, while SNAKE_CASE env names
/// (`GITHUB_FEEDBACK_TOKEN`) still do: `_` remains a permitted predecessor.
fn redaction_rules() -> Vec<Rule> {
    vec![
        Rule {
            label: "Bearer token",
            re: static_regex!(r"(?i)(bearer\s+)[a-zA-Z0-9\-_\.]{8,}"),
            guard_value: false,
        },
        Rule {
            label: "Authorization header",
            re: static_regex!(r"(?i)(authorization:\s*(?:basic|bearer|token)\s+)[^\s\r\n]+"),
            guard_value: false,
        },
        // Key/value secrets: group 1 = predecessor + `name=`/`name: ` prefix
        // (kept), group 2 = the value (redacted unless benign).
        Rule {
            label: "API key param",
            re: static_regex!(
                r#"(?im)((?:^|[^a-z0-9])(?:api[_-]?key|apikey|access[_-]?key|secret[_-]?key|token|password|passwd|pwd|secret)\s*[=:]\s*)([^\s\r\n,;&"']+)"#
            ),
            guard_value: true,
        },
        // Whole token is the secret — no prefix group, so the entire match is
        // replaced. (Previously group 1 captured the key itself and leaked it.)
        Rule {
            label: "AWS key",
            re: static_regex!(r"AKIA[0-9A-Z]{12,}"),
            guard_value: false,
        },
        Rule {
            label: "Private key block",
            re: static_regex!(
                r"(?s)(-----BEGIN\s+(?:RSA\s+)?PRIVATE\s+KEY-----).+?-----END\s+(?:RSA\s+)?PRIVATE\s+KEY-----"
            ),
            guard_value: false,
        },
        Rule {
            label: "GitHub token",
            re: static_regex!(r"(gh[pousr]_)[a-zA-Z0-9]{20,}"),
            guard_value: false,
        },
        // Group 1 = prefix (kept), group 2 = the 32+ char value. Guarded since
        // 32-char identifiers like `confirmRequiredEndpointKeySchema`
        // are references, not secrets.
        Rule {
            label: "Generic long secret",
            re: static_regex!(
                r#"(?im)((?:^|[^a-z0-9])(?:key|token|secret|password|credential|auth)\s*[=:]\s*)(['"]?[a-zA-Z0-9+/=\-_]{32,}['"]?)"#
            ),
            guard_value: true,
        },
    ]
}

pub fn redact_text(input: &str) -> String {
    let mut out = input.to_string();
    for rule in redaction_rules() {
        out = rule
            .re
            .replace_all(&out, |caps: &regex::Captures| {
                let whole = caps.get(0).map_or("", |m| m.as_str());
                if rule.guard_value
                    && let Some(value) = caps.get(2)
                    && is_benign_secret_value(value.as_str())
                {
                    // Not a secret (identifier reference, literal, placeholder)
                    // — keep verbatim.
                    return whole.to_string();
                }
                match caps.get(1) {
                    Some(prefix) => format!("{}[REDACTED:{}]", prefix.as_str(), rule.label),
                    None => format!("[REDACTED:{}]", rule.label),
                }
            })
            .to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- exclude_patterns compilation cache ---

    #[test]
    fn redacts_bearer_token() {
        let s = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345";
        let out = redact_text(s);
        assert!(out.contains("[REDACTED"));
        assert!(!out.contains("abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn redacts_private_key_block() {
        let s = "-----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----";
        let out = redact_text(s);
        assert!(out.contains("[REDACTED"));
        assert!(!out.contains("\nabc\n"));
    }

    #[test]
    fn redacts_api_key_param_value() {
        let out = redact_text("password=hunter2-super-secret-value");
        assert!(
            out.contains("password=[REDACTED:API key param]"),
            "got: {out}"
        );
        assert!(!out.contains("hunter2"));
    }

    /// TypeScript type annotations and language literals must NOT be
    /// redacted — over-eager masking corrupted source files read via sobre_read.
    #[test]
    fn keeps_non_secret_literals() {
        for s in [
            "password: undefined",
            "secret: string",
            "token: null",
            "apiKey: boolean",
            "password = false",
            "secret: any",
            "let pwd: number = 1",
        ] {
            assert_eq!(redact_text(s), s, "must not redact non-secret literal: {s}");
        }
    }

    /// TS type annotations (generics, unions, arrays, function/object
    /// types) carry angle brackets / brackets that real secret tokens never do,
    /// so they must survive verbatim even when the key looks sensitive.
    #[test]
    fn keeps_type_annotations() {
        for s in [
            "password: Promise<string>",
            "apiKey: Record<string, unknown>",
            "token: string[]",
            "secret: () => void",
            "password: string | undefined",
            "credential: { value: string }",
        ] {
            assert_eq!(redact_text(s), s, "must not redact type annotation: {s}");
        }
    }

    /// Whole-token secrets must be removed, not annotated in place — previously
    /// the closure kept group 1 (the key itself) and only appended `[REDACTED]`.
    #[test]
    fn fully_redacts_aws_key() {
        let out = redact_text("AKIAIOSFODNN7EXAMPLE");
        assert!(
            !out.contains("AKIAIOSFODNN7EXAMPLE"),
            "AWS key leaked: {out}"
        );
        assert!(out.contains("[REDACTED:AWS key]"));
    }

    #[test]
    fn fully_redacts_generic_long_secret() {
        // `credential=` is not covered by the API-key-param rule, so this
        // exercises the generic fallback (the previously leaky path).
        let secret = "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6"; // 32 chars
        let out = redact_text(&format!("credential={secret}"));
        assert!(!out.contains(secret), "long secret leaked: {out}");
        assert!(
            out.contains("credential=[REDACTED:Generic long secret]"),
            "got: {out}"
        );
    }

    #[test]
    fn redacts_github_token_keeping_prefix() {
        let out = redact_text("ghp_abcdefghijklmnopqrstuvwxyz0123");
        assert!(out.starts_with("ghp_[REDACTED:GitHub token]"), "got: {out}");
        assert!(!out.contains("abcdefghijklmnopqrstuvwxyz"));
    }

    // ── benign identifier references, prose and placeholders ──

    /// Repro 1: prose that mentions a keyword must not have the following
    /// word redacted — "token: SvelteKit's…" is documentation, not a secret.
    #[test]
    fn keeps_prose_identifier_after_keyword() {
        let s = "the CSRF token: SvelteKit's native origin-check on form actions";
        assert_eq!(redact_text(s), s, "prose must survive verbatim");
    }

    /// Repro 2: camelCase subwords must not trigger the keyword alternation,
    /// and identifier/property-access RHS values are references, not secrets.
    #[test]
    fn keeps_identifier_and_property_references() {
        for s in [
            "superuserPassword: inputEnv.POCKETBASE_SUPERUSER_PASSWORD",
            "export const getStripeSecretKey = serverEnv.getStripeSecretKey;",
            "const apiKey = config.stripeApiKey",
        ] {
            assert_eq!(redact_text(s), s, "identifier reference redacted: {s}");
        }
    }

    /// Repro 3: a 32+ char identifier (Zod schema name) is a reference —
    /// "Generic long secret" needs the same value guard as the API-key rule.
    #[test]
    fn keeps_long_schema_identifier() {
        let s = "endpoint_key: confirmRequiredEndpointKeySchema,";
        assert_eq!(redact_text(s), s, "schema identifier must not be redacted");
    }

    /// Repro 4: obvious placeholder values (.env.example) are documentation.
    #[test]
    fn keeps_placeholder_values() {
        for s in [
            "GITHUB_FEEDBACK_TOKEN=ghp_change_me",
            "API_KEY=your_key_here",
            "password=<insert-password>",
            "SECRET_KEY=xxxxxxxx",
        ] {
            assert_eq!(redact_text(s), s, "placeholder redacted: {s}");
        }
    }

    /// The flip side: real secret-shaped values must STILL be redacted after
    /// the guards.
    #[test]
    fn still_redacts_real_secret_values() {
        // Digit-bearing value after a snake_case env name.
        let out = redact_text("GITHUB_TOKEN=ghpA1b2c3d4e5f6g7h8");
        assert!(!out.contains("ghpA1b2c3d4e5f6g7h8"), "leaked: {out}");
        // SNAKE_CASE env assignment with digits (the _ predecessor stays a
        // word boundary that MATCHES).
        let out = redact_text("MY_SECRET=abc123def456ghi789");
        assert!(!out.contains("abc123def456ghi789"), "leaked: {out}");
        // Quoted 32+ char literal: a quoted value is never an identifier
        // reference, so the Generic-long-secret guard keeps redacting it.
        let quoted = "key: 'abcdefghijklmnopqrstuvwxyzabcdef'";
        let out = redact_text(quoted);
        assert!(
            !out.contains("abcdefghijklmnopqrstuvwxyzabcdef"),
            "leaked: {out}"
        );
    }

    #[test]
    fn identifier_and_placeholder_heuristics() {
        assert!(is_identifier_reference("serverEnv.getStripeSecretKey"));
        assert!(is_identifier_reference("confirmRequiredEndpointKeySchema"));
        assert!(is_identifier_reference("$scope._private"));
        assert!(!is_identifier_reference("abc123"), "digits → secret-shaped");
        assert!(!is_identifier_reference("\"quoted\""), "literal value");
        assert!(!is_identifier_reference("a-b"), "dash is not identifier");
        assert!(is_placeholder_value("ghp_change_me"));
        assert!(is_placeholder_value("<token>"));
        assert!(is_placeholder_value("your_api_key_123"));
        assert!(!is_placeholder_value("A1b2C3d4E5f6G7h8"));
    }

    /// Scientific notation and plain numbers after keys containing
    /// `token` must not be redacted — these are pricing/cost fields.
    #[test]
    fn keeps_numeric_values() {
        for (input, desc) in [
            ("input_cost_per_token: 1.4e-06", "scientific notation"),
            ("output_cost_per_token: 4.4e-06", "scientific notation"),
            (
                "cache_read_input_token_cost: 1.9e-07",
                "scientific notation",
            ),
            ("token: 600", "plain integer"),
            ("secret: 0.5", "decimal float"),
            ("api_key: 42", "small integer"),
            ("password: 3.14", "pi float"),
        ] {
            assert_eq!(
                redact_text(input),
                input,
                "must not redact numeric value ({desc}): {input}"
            );
        }
    }

    /// Env-variable references must not be redacted — they are
    /// pointers to secrets, not the secrets themselves.
    #[test]
    fn keeps_env_references() {
        for (input, desc) in [
            (
                "api_key: os.environ/MY_SERVICE_API_KEY",
                "Python os.environ/",
            ),
            ("secret: os.getenv(MY_KEY)", "Python os.getenv()"),
            ("token: process.env.API_TOKEN", "Node process.env"),
            (
                "password: inputEnv.POCKETBASE_SUPERUSER_PASSWORD",
                "inputEnv dot ref",
            ),
            ("api_key: ${MY_API_KEY}", "shell interpolation ${}"),
            ("secret: $MY_SECRET", "shell $VAR"),
            ("token: %API_TOKEN%", "Windows %VAR%"),
            ("api_key: ENV[API_KEY]", "Ruby ENV[]"),
            ("secret: env(SECRET_KEY)", "Laravel env()"),
            ("password: System.getenv(DB_PASS)", "Java System.getenv"),
        ] {
            assert_eq!(
                redact_text(input),
                input,
                "must not redact env ref ({desc}): {input}"
            );
        }
    }

    /// Real secrets must STILL be redacted (regression guard).
    #[test]
    fn still_redacts_real_secrets() {
        for input in [
            "api_key: sk-1234567890abcdef1234567890abcdef",
            "password: hunter2-super-secret-value",
            "token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payload.signature",
        ] {
            let out = redact_text(input);
            assert!(
                out.contains("[REDACTED"),
                "must redact real secret: {input} -> {out}"
            );
        }
    }

    /// `token == ""` in Go source must not be redacted — the regex
    /// consumes the first `=` leaving group 2 = `=` (comparison residue).
    #[test]
    fn keeps_comparison_operators_in_source() {
        for s in [
            r#"if token == "" {"#,
            r#"if token, err = checkPulsares(); token == "" || err != nil {"#,
            r#"if password != "" {"#,
            r"assert secret == expected_value",
        ] {
            assert_eq!(redact_text(s), s, "comparison operator corrupted: {s}");
        }
    }
}
