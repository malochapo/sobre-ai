//! Domain-specific abbreviation dictionaries for terse compression.
//!
//! Each dictionary provides whole-word-matching abbreviations for a specific
//! domain (git, cargo, npm, general), with word-boundary-aware matching.

/// A single abbreviation rule: replaces `long` with `short` at word boundaries.
pub struct Abbreviation {
    pub long: &'static str,
    pub short: &'static str,
}

/// Empty — all 60 single-English-word rules tokenize identically to their
/// abbreviations under BPE (1 tok -> 1 tok), so they saved zero tokens while
/// corrupting source code, file paths, and search results (#973, #980, #981).
/// Phrase-level rules (multi-word) are the ones that yield real savings and
/// live in the domain dictionaries (GIT, CARGO, NPM).
pub const GENERAL: &[Abbreviation] = &[];

pub const GIT: &[Abbreviation] = &[
    Abbreviation {
        long: "modified",
        short: "M",
    },
    Abbreviation {
        long: "deleted",
        short: "D",
    },
    Abbreviation {
        long: "untracked",
        short: "?",
    },
    Abbreviation {
        long: "renamed",
        short: "R",
    },
    Abbreviation {
        long: "copied",
        short: "C",
    },
    Abbreviation {
        long: "insertion",
        short: "+",
    },
    Abbreviation {
        long: "deletion",
        short: "-",
    },
    Abbreviation {
        long: "detached",
        short: "det",
    },
    Abbreviation {
        long: "conflict",
        short: "!!",
    },
    Abbreviation {
        long: "changes not staged for commit",
        short: "unstaged",
    },
    Abbreviation {
        long: "Changes to be committed",
        short: "staged",
    },
    Abbreviation {
        long: "nothing to commit, working tree clean",
        short: "clean",
    },
];

pub const CARGO: &[Abbreviation] = &[
    Abbreviation {
        long: "Compiling",
        short: "CC",
    },
    Abbreviation {
        long: "Downloading",
        short: "DL",
    },
    Abbreviation {
        long: "Finished",
        short: "OK",
    },
    Abbreviation {
        long: "warning",
        short: "W",
    },
    Abbreviation {
        long: "test result: ok",
        short: "PASS",
    },
    Abbreviation {
        long: "test result: FAILED",
        short: "FAIL",
    },
    Abbreviation {
        long: "running",
        short: "run",
    },
    Abbreviation {
        long: "Blocking waiting for file lock on package cache",
        short: "LOCK",
    },
    Abbreviation {
        long: "Updating crates.io index",
        short: "IDX",
    },
    Abbreviation {
        long: "target/debug",
        short: "t/d",
    },
    Abbreviation {
        long: "target/release",
        short: "t/r",
    },
];

pub const NPM: &[Abbreviation] = &[
    Abbreviation {
        long: "added",
        short: "+",
    },
    Abbreviation {
        long: "removed",
        short: "-",
    },
    Abbreviation {
        long: "node_modules",
        short: "n_m",
    },
    Abbreviation {
        long: "devDependencies",
        short: "devDeps",
    },
    Abbreviation {
        long: "peerDependencies",
        short: "peerDeps",
    },
    Abbreviation {
        long: "optionalDependencies",
        short: "optDeps",
    },
    Abbreviation {
        long: "npm warn",
        short: "W",
    },
    Abbreviation {
        long: "npm error",
        short: "E",
    },
];

/// Applies whole-word abbreviations from the given dictionaries to the text.
/// Uses a single scan: first checks which patterns exist, then applies only matches.
pub fn apply_dictionaries(text: &str, level: DictLevel) -> String {
    let dicts: Vec<&[Abbreviation]> = match level {
        DictLevel::General => vec![GENERAL],
        DictLevel::Full => vec![GENERAL, GIT, CARGO, NPM],
    };

    let mut result = text.to_string();
    for dict in dicts {
        for abbr in dict {
            result = replace_whole_word(&result, abbr.long, abbr.short);
        }
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DictLevel {
    General,
    Full,
}

fn is_word_boundary(b: u8) -> bool {
    !b.is_ascii_alphanumeric() && b != b'-' && b != b'_' && b != b'\'' && b != b'"'
}

/// #973: true when `[match_start..match_end)` sits inside a file-path token —
/// the surrounding whitespace-delimited word contains `/` or `\`.  Dictionary
/// substitutions inside paths emit non-existent paths (`environment.rs` →
/// `env.rs`).
fn is_inside_path(text: &[u8], match_start: usize, match_end: usize) -> bool {
    let token_start = text[..match_start]
        .iter()
        .rposition(u8::is_ascii_whitespace)
        .map_or(0, |i| i + 1);
    let token_end = text[match_end..]
        .iter()
        .position(u8::is_ascii_whitespace)
        .map_or(text.len(), |i| match_end + i);
    let token = &text[token_start..token_end];
    token.contains(&b'/') || token.contains(&b'\\')
}

/// Whole-word replacement — **case-sensitive**, path-aware, non-ASCII safe.
///
/// #981 fix: matching was case-insensitive, collapsing `context.Context` into
/// `ctx.ctx`.  Now matches the exact case of the pattern only.  All byte
/// offsets come from a single string (the original text), eliminating the
/// lowercased-copy divergence that panicked on non-ASCII input (ß→ss changes
/// byte length).
///
/// #973 fix: matches inside file-path tokens (containing `/` or `\`) are
/// skipped so `src/environment.rs` is never rewritten to `src/env.rs`.
pub(crate) fn replace_whole_word(text: &str, pattern: &str, replacement: &str) -> String {
    if pattern.is_empty() || !text.contains(pattern) {
        return text.to_string();
    }

    let bytes = text.as_bytes();
    let pat_len = pattern.len();
    let mut result = String::with_capacity(text.len());
    let mut start = 0;

    while let Some(pos) = text[start..].find(pattern) {
        let abs_pos = start + pos;
        let end_pos = abs_pos + pat_len;

        let before_ok = abs_pos == 0 || is_word_boundary(bytes[abs_pos - 1]);
        let after_ok = end_pos >= bytes.len() || is_word_boundary(bytes[end_pos]);

        result.push_str(&text[start..abs_pos]);

        if before_ok && after_ok && !is_inside_path(bytes, abs_pos, end_pos) {
            result.push_str(replacement);
        } else {
            result.push_str(&text[abs_pos..end_pos]);
        }
        start = end_pos;
    }
    result.push_str(&text[start..]);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_replaces_standalone() {
        let r = replace_whole_word("the function works", "function", "fn");
        assert_eq!(r, "the fn works");
    }

    #[test]
    fn whole_word_skips_substring() {
        let r = replace_whole_word("dysfunction", "function", "fn");
        assert_eq!(r, "dysfunction");
    }

    #[test]
    fn whole_word_at_start() {
        let r = replace_whole_word("function call", "function", "fn");
        assert_eq!(r, "fn call");
    }

    #[test]
    fn whole_word_at_end() {
        let r = replace_whole_word("call function", "function", "fn");
        assert_eq!(r, "call fn");
    }

    #[test]
    fn whole_word_with_punctuation() {
        let r = replace_whole_word("function(arg)", "function", "fn");
        assert_eq!(r, "fn(arg)");
    }

    // #981: case-sensitive matching — `Context` ≠ `context`.
    #[test]
    fn case_sensitive_preserves_different_casing() {
        assert_eq!(
            replace_whole_word("context.Context", "context", "ctx"),
            "ctx.Context",
            "only lowercase `context` should be replaced (#981)"
        );
    }

    // #981: non-ASCII must not panic.
    #[test]
    fn non_ascii_input_does_not_panic() {
        let r = replace_whole_word("die Größe der function", "function", "fn");
        assert_eq!(r, "die Größe der fn");
    }

    #[test]
    fn non_ascii_with_no_match_returns_unchanged() {
        let r = replace_whole_word("Ströme und Flüsse", "function", "fn");
        assert_eq!(r, "Ströme und Flüsse");
    }

    // #973: file paths must never be rewritten.
    #[test]
    fn path_words_are_never_abbreviated() {
        assert_eq!(
            replace_whole_word("src/environment.rs changed", "environment", "env"),
            "src/environment.rs changed",
            "words inside paths must be preserved (#973)"
        );
    }

    #[test]
    fn path_with_backslash_protected() {
        assert_eq!(
            replace_whole_word("src\\configuration\\mod.rs", "configuration", "cfg"),
            "src\\configuration\\mod.rs"
        );
    }

    #[test]
    fn standalone_word_still_replaced_next_to_path() {
        assert_eq!(
            replace_whole_word(
                "the environment in src/environment.rs",
                "environment",
                "env"
            ),
            "the env in src/environment.rs",
            "standalone word replaced, path-embedded word preserved"
        );
    }

    #[test]
    fn general_dict_stays_empty() {
        assert!(
            GENERAL.is_empty(),
            "GENERAL must stay empty: all 60 single-word rules save 0 tokens              under BPE and corrupt source/paths (#973, #980, #981)"
        );
    }

    #[test]
    fn full_dict_includes_domain() {
        let r = apply_dictionaries("Compiling lean-ctx", DictLevel::Full);
        assert!(r.contains("CC"), "cargo abbreviation should apply: {r}");
    }

    #[test]
    fn no_abbreviation_inflates_tokens() {
        for (name, dict) in [("GIT", GIT), ("CARGO", CARGO), ("NPM", NPM)] {
            for abbr in dict {
                let long_tok = crate::core::tokens::count_tokens(abbr.long);
                let short_tok = crate::core::tokens::count_tokens(abbr.short);
                assert!(
                    short_tok <= long_tok,
                    "{name}: '{}'->'{}'  inflates ({long_tok} tok -> {short_tok} tok)",
                    abbr.long,
                    abbr.short,
                );
            }
        }
    }

    #[test]
    fn dict_count_git() {
        assert!(
            GIT.len() >= 9,
            "should have 9+ git abbreviations, got {}",
            GIT.len()
        );
    }

    #[test]
    fn git_dict_never_abbreviates_subcommands() {
        let git_subcommands = [
            "commit", "branch", "checkout", "merge", "stash", "rebase", "push", "pull", "fetch",
            "clone", "tag", "reset", "bisect", "log", "diff", "show", "status", "add",
        ];
        for abbr in GIT {
            assert!(
                !git_subcommands.contains(&abbr.long),
                "GIT dictionary must NOT abbreviate git subcommand '{}' (→ '{}'). \
                 Agents will misinterpret abbreviated output as valid commands.",
                abbr.long,
                abbr.short
            );
        }
    }

    #[test]
    fn commit_word_survives_full_dict() {
        let text = "commit abc1234 on branch main";
        let result = apply_dictionaries(text, DictLevel::Full);
        assert!(
            result.contains("commit"),
            "word 'commit' must not be abbreviated in output: {result}"
        );
    }

    #[test]
    fn branch_word_survives_full_dict() {
        let text = "Your branch is ahead of 'origin/main' by 2 commits";
        let result = apply_dictionaries(text, DictLevel::Full);
        assert!(
            result.contains("branch"),
            "word 'branch' must not be abbreviated in output: {result}"
        );
    }

    // #973: paths in realistic shell output survive dictionary application.
    #[test]
    fn dict_preserves_file_paths_in_shell_output() {
        let text = "warning: unused variable in src/configuration/environment.rs:42";
        let result = apply_dictionaries(text, DictLevel::Full);
        assert!(
            result.contains("src/configuration/environment.rs:42"),
            "file path must survive dictionary: {result}"
        );
    }

    #[test]
    fn dictionaries_leave_source_code_intact() {
        let go_src = "func handler(ctx context.Context) (api.Result, error) { return doWork(ctx) }";
        let result = apply_dictionaries(go_src, DictLevel::Full);
        for keyword in ["context.Context", "error", "return"] {
            assert!(
                result.contains(keyword),
                "source keyword '{keyword}' must survive dictionaries: {result}"
            );
        }
        let paths = "src/environment.rs src/configuration.rs src/repository.rs";
        let result = apply_dictionaries(paths, DictLevel::Full);
        assert_eq!(
            result, paths,
            "file paths must survive dictionaries verbatim"
        );
    }
}
