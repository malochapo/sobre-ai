//! Tests for the network dispatch (help guard, proxy parsing, open-mode fallback).

use super::wants_help;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_string()).collect()
}

// GH #393: `daemon enable --help` executed instead of showing help.
// The guard must catch help flags at any position, for any verb.
#[test]
fn help_flag_detected_after_verb() {
    assert!(wants_help(&args(&["enable", "--help"])));
    assert!(wants_help(&args(&["disable", "-h"])));
    assert!(wants_help(&args(&["restart", "--help"])));
    assert!(wants_help(&args(&["help"])));
    assert!(wants_help(&args(&["--help"])));
}

// #603/#616: the Codex ChatGPT-subscription opt-in must be bridged from the
// shell env into config.toml (the managed proxy / env-less setup passes never
// see the env var), but only once and never overriding an explicit config.
#[test]
fn codex_chatgpt_optin_persists_only_when_env_set_and_not_already_on() {
    use super::should_persist_codex_chatgpt_optin as persist;
    // Env opt-in present and config has not enabled it yet → persist.
    assert!(persist(true, None));
    assert!(persist(true, Some(false)));
    // Already enabled in config → idempotent no-op.
    assert!(!persist(true, Some(true)));
    // Env absent → never touch config; config stays the source of truth.
    assert!(!persist(false, None));
    assert!(!persist(false, Some(false)));
    assert!(!persist(false, Some(true)));
}

// The durable `proxy codex-chatgpt <arg>` switch: on/off (+ synonyms) mutate,
// `status`/no-arg is read-only, anything else is rejected (never a silent flip).
#[test]
fn codex_chatgpt_action_parsing_is_explicit() {
    use super::CodexChatgptAction::{Off, On, Status, Unknown};
    use super::parse_codex_chatgpt_action as parse;
    assert_eq!(parse(Some("on")), On);
    assert_eq!(parse(Some("enable")), On);
    assert_eq!(parse(Some("true")), On);
    assert_eq!(parse(Some("off")), Off);
    assert_eq!(parse(Some("disable")), Off);
    assert_eq!(parse(Some("false")), Off);
    assert_eq!(parse(Some("status")), Status);
    assert_eq!(parse(None), Status, "bare call must be read-only status");
    assert_eq!(parse(Some("nonsense")), Unknown);
}

#[test]
fn normal_verbs_do_not_trigger_help() {
    assert!(!wants_help(&args(&["enable"])));
    assert!(!wants_help(&args(&["status"])));
    assert!(!wants_help(&args(&[])));
    // Values that merely contain "help" as a substring must not match.
    assert!(!wants_help(&args(&["--helper"])));
}
