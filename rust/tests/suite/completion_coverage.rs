//! Integration tests for the shell completion engine.

use lean_ctx::cli::completions::spec::COMMAND_TREE;

#[test]
fn top_level_has_config_and_proxy() {
    let names: Vec<&str> = COMMAND_TREE.iter().map(|n| n.name).collect();
    assert!(names.contains(&"config"), "missing 'config'");
    assert!(names.contains(&"proxy"), "missing 'proxy'");
}

#[test]
fn top_level_declares_no_aliases() {
    // Every alias this tree carried named a command with no dispatch arm
    // (`bugs`, `savings`, `lockdown`, `upgrade`), or aliased a command that has
    // its own arm and its own flags (`secure` -> `yolo`, `find` -> `grep`).
    // Both kinds mis-complete. The mechanism stays in `CommandNode`; adding one
    // back should be deliberate, and `every_completion_name_has_a_dispatch_arm`
    // will hold it to a real command.
    let with_aliases: Vec<&str> = COMMAND_TREE
        .iter()
        .filter(|n| !n.aliases.is_empty())
        .map(|n| n.name)
        .collect();
    assert!(
        with_aliases.is_empty(),
        "unexpected aliases on: {with_aliases:?}"
    );
}
