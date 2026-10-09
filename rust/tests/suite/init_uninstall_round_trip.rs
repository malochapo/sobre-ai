//! `init` then `uninstall` gives the HOME back as it was: no file, directory
//! or backup left behind, even after `init` ran twice and once more from a
//! binary that moved. Runs the real binary in an isolated HOME, with no
//! `claude` CLI on the PATH and no sobre or XDG variable from the caller.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every path under `root` (directories as `None`, files with their bytes).
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                out.insert(rel, None);
                stack.push(path);
            } else {
                out.insert(rel, Some(std::fs::read(&path).unwrap_or_default()));
            }
        }
    }
    out
}

fn run(binary: &Path, home: &Path, args: &[&str]) {
    let out = Command::new(binary)
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("spawn sobre");
    assert!(
        out.status.success(),
        "`sobre {}` failed:\nstdout:\n{}\nstderr:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn init_twice_then_moved_binary_then_uninstall_leaves_home_as_it_was() {
    let root = tempfile::tempdir().expect("tempdir");
    let home = root.path().join("home");
    // What makes each agent detected, as a user would have it.
    for dir in [
        ".claude",
        ".codex",
        ".gemini",
        ".cursor",
        ".copilot",
        ".codeium/windsurf",
        ".config/zed",
        ".config/opencode",
        ".config/Code/User",
    ] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    std::fs::write(
        home.join(".config/Code/User/settings.json"),
        "{\n  // the user's\n  \"editor.fontSize\": 13\n}\n",
    )
    .unwrap();
    std::fs::write(
        home.join(".config/zed/settings.json"),
        "// Zed settings\n{\n  \"theme\": \"One Dark\",\n}\n",
    )
    .unwrap();
    let before = snapshot(&home);

    let installed = PathBuf::from(env!("CARGO_BIN_EXE_sobre"));
    let moved = root.path().join("bin").join("sobre");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::copy(&installed, &moved).unwrap();

    run(&installed, &home, &["init", "--repair"]);
    run(&installed, &home, &["init", "--repair"]);
    run(&moved, &home, &["init", "--repair"]);
    run(&moved, &home, &["uninstall", "--keep-binary"]);

    let after = snapshot(&home);
    let left: Vec<_> = after
        .iter()
        .filter(|(path, content)| before.get(*path) != Some(*content))
        .map(|(path, _)| path.display().to_string())
        .collect();
    let gone: Vec<_> = before
        .keys()
        .filter(|path| !after.contains_key(*path))
        .map(|path| path.display().to_string())
        .collect();
    assert!(
        left.is_empty() && gone.is_empty(),
        "HOME differs after init and uninstall\nadded or changed: {left:#?}\nremoved: {gone:#?}"
    );
}
