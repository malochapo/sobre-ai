//! What counts as a re-fetch: only the same thing asked for again. Ordinary
//! work on different files and commands never lowers compression.

use sobre::core::refetch_signals::RefetchSignals;

/// Past the cold start, which counts nothing.
fn warm() -> RefetchSignals {
    let mut signals = RefetchSignals::default();
    for i in 0..3 {
        signals.record_read(&format!("cold{i}.rs"), "full", false);
    }
    signals
}

#[test]
fn different_commands_are_not_reruns() {
    let mut signals = warm();
    signals.record_shell("cargo build");
    signals.record_shell("cargo test");
    signals.record_shell("cargo clippy");
    assert_eq!(signals.correction_count(), 0);
}

#[test]
fn reading_many_different_files_is_not_a_correction() {
    let mut signals = warm();
    for i in 0..10 {
        signals.record_read(&format!("src/file{i}.rs"), "map", false);
    }
    assert_eq!(signals.correction_count(), 0);
}

#[test]
fn every_rerun_of_the_same_command_counts() {
    let mut signals = warm();
    signals.record_shell("cargo test --lib");
    signals.record_shell("cargo test --lib");
    signals.record_shell("cargo test --lib");
    assert_eq!(signals.correction_count(), 2);
}
