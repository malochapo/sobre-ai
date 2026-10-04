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

#[test]
fn normal_verbs_do_not_trigger_help() {
    assert!(!wants_help(&args(&["enable"])));
    assert!(!wants_help(&args(&["status"])));
    assert!(!wants_help(&args(&[])));
    // Values that merely contain "help" as a substring must not match.
    assert!(!wants_help(&args(&["--helper"])));
}
