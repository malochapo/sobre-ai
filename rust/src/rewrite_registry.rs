/// Commands sobre knows how to compress. The Cursor/Gemini pre-run hooks wrap
/// only these in `sobre -c`, and never a file read (`FileRead`), whose
/// content must reach the agent verbatim.
pub const REWRITE_COMMANDS: &[RewriteEntry] = &[
    // Version control
    re("git", Category::Vcs),
    re("gh", Category::Vcs),
    // Rust
    re("cargo", Category::Build),
    // JavaScript/Node
    re("npm", Category::PackageManager),
    re("pnpm", Category::PackageManager),
    re("yarn", Category::PackageManager),
    re("bun", Category::Build),
    re("bunx", Category::Build),
    re("deno", Category::Build),
    re("vite", Category::Build),
    // Python
    re("python", Category::Build),
    re("python3", Category::Build),
    re("pip", Category::PackageManager),
    re("pip3", Category::PackageManager),
    re("uv", Category::PackageManager),
    re("pytest", Category::Build),
    re("mypy", Category::Lint),
    re("ruff", Category::Lint),
    // Go
    re("go", Category::Build),
    re("golangci-lint", Category::Lint),
    // Containers / Infra
    re("docker", Category::Infra),
    re("docker-compose", Category::Infra),
    re("kubectl", Category::Infra),
    re("helm", Category::Infra),
    re("aws", Category::Infra),
    re("terraform", Category::Infra),
    re("tofu", Category::Infra),
    // Linters / Formatters
    re("eslint", Category::Lint),
    re("prettier", Category::Lint),
    re("tsc", Category::Lint),
    re("biome", Category::Lint),
    // HTTP
    re("curl", Category::Http),
    re("wget", Category::Http),
    // PHP
    re("php", Category::Build),
    re("composer", Category::PackageManager),
    // .NET
    re("dotnet", Category::Build),
    // Ruby
    re("bundle", Category::PackageManager),
    re("rake", Category::Build),
    // Elixir
    re("mix", Category::Build),
    // Swift / Zig / CMake
    re("swift", Category::Build),
    re("zig", Category::Build),
    re("cmake", Category::Build),
    re("make", Category::Build),
    // Search
    re("grep", Category::Search),
    re("egrep", Category::Search),
    re("fgrep", Category::Search),
    re("rg", Category::Search),
    // File reads: never wrapped, never compressed after the fact
    re("cat", Category::FileRead),
    re("head", Category::FileRead),
    re("tail", Category::FileRead),
    re("tac", Category::FileRead),
    re("nl", Category::FileRead),
    re("bat", Category::FileRead),
    re("batcat", Category::FileRead),
    re("less", Category::FileRead),
    re("more", Category::FileRead),
    re("sed", Category::FileRead),
    re("awk", Category::FileRead),
    re("gawk", Category::FileRead),
    re("mawk", Category::FileRead),
    re("nawk", Category::FileRead),
    // Directory listing
    re("ls", Category::DirList),
    re("find", Category::DirList),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Vcs,
    Build,
    PackageManager,
    Lint,
    Infra,
    Http,
    Search,
    FileRead,
    DirList,
}

#[derive(Debug, Clone, Copy)]
pub struct RewriteEntry {
    pub command: &'static str,
    pub category: Category,
}

const fn re(command: &'static str, category: Category) -> RewriteEntry {
    RewriteEntry { command, category }
}

/// True when a command of the line prints file content, whose output sobre
/// never wraps or compresses: a `FileRead` entry (`cat`, `head`, `sed`, …;
/// `sed` and `awk` not when they edit in place), a grep-family search with
/// context lines (`-A`, `-B`, `-C`), `git show <rev>:<path>`, `git blame`
/// or `git cat-file`. Every command of a chain or pipeline counts, so
/// `cd src && sed -n '1,80p' x.py` is a file read. A command reading the
/// previous one's output (`git log | head`) is not: it prints that output.
pub fn is_file_read_command(cmd: &str) -> bool {
    crate::compound_lexer::commands(cmd)
        .iter()
        .any(|c| !c.piped && reads_file(c.text))
}

fn reads_file(command: &str) -> bool {
    let words: Vec<&str> = strip_env_prefix(command).split_whitespace().collect();
    let Some((first, args)) = words.split_first() else {
        return false;
    };
    let program = first.rsplit('/').next().unwrap_or(first).to_ascii_lowercase();
    match program.as_str() {
        "sed" | "awk" | "gawk" | "mawk" | "nawk" => {
            !crate::shell::compress::has_in_place_flag(command)
        }
        "grep" | "egrep" | "fgrep" | "rg" | "ag" | "ack" | "ugrep" => {
            args.iter().any(|a| is_context_flag(a))
        }
        "git" => git_prints_file(args),
        p => REWRITE_COMMANDS
            .iter()
            .any(|e| e.category == Category::FileRead && e.command == p),
    }
}

/// A grep-family option that prints lines around each match: `-A 3`, `-B3`,
/// `-nC2`, `-5`, `--context=3`, `--after-context`, `--before-context`. A
/// short-flag cluster stops at `e`, `f` or `m`, whose value follows them.
fn is_context_flag(word: &str) -> bool {
    if let Some(long) = word.strip_prefix("--") {
        return ["context", "after-context", "before-context"]
            .iter()
            .any(|f| long == *f || long.strip_prefix(f).is_some_and(|v| v.starts_with('=')));
    }
    let Some(short) = word.strip_prefix('-').filter(|s| !s.is_empty()) else {
        return false;
    };
    if short.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    for b in short.bytes() {
        match b {
            b'A' | b'B' | b'C' => return true,
            b'e' | b'f' | b'm' => return false,
            b if b.is_ascii_alphabetic() => {}
            _ => return false,
        }
    }
    false
}

/// `git show <rev>:<path>`, `git blame`, `git annotate`, `git cat-file`, and
/// `git grep` with context lines print a file's lines.
fn git_prints_file(args: &[&str]) -> bool {
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "-C" | "-c" | "--git-dir" | "--work-tree" | "--namespace" => {
                rest.next();
            }
            a if a.starts_with('-') => {}
            "blame" | "annotate" | "cat-file" => return true,
            "show" => return rest.any(|a| !a.starts_with('-') && a.contains(':')),
            "grep" => return rest.any(|a| is_context_flag(a)),
            _ => return false,
        }
    }
    false
}

/// Check if a command string matches a rewritable prefix (for hook handlers).
/// Excludes FileRead (handled separately in hook_handlers).
///
/// Handles common shell patterns:
/// - Bare command: `git status`
/// - Env-var prefixed: `PYTHONPATH=src python script.py`
/// - Path-qualified: `./.venv/bin/python`, `/usr/bin/python3`
pub fn is_rewritable_command(cmd: &str) -> bool {
    let effective = strip_env_prefix(cmd);
    let basename = extract_command_basename(effective);

    for entry in REWRITE_COMMANDS {
        if matches!(entry.category, Category::FileRead) {
            continue;
        }
        let prefix = format!("{} ", entry.command);
        if effective.starts_with(&prefix)
            || effective == entry.command
            || basename == entry.command
            || basename.starts_with(&format!("{} ", entry.command))
        {
            return true;
        }
    }
    false
}

/// Strip leading `KEY=value` env-var assignments from a command string.
/// e.g. `PYTHONPATH=src FOO=bar python x.py` → `python x.py`
fn strip_env_prefix(cmd: &str) -> &str {
    let mut rest = cmd;
    loop {
        let trimmed = rest.trim_start();
        if let Some(eq_pos) = trimmed.find('=') {
            let before_eq = &trimmed[..eq_pos];
            if !before_eq.is_empty()
                && before_eq
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && let Some(space_pos) = trimmed[eq_pos..].find(' ')
            {
                rest = &trimmed[eq_pos + space_pos..];
                continue;
            }
        }
        return trimmed;
    }
}

/// Extract the basename of a potentially path-qualified command.
/// e.g. `./.venv/bin/python script.py` → `python script.py`
///      `/usr/local/bin/cargo test` → `cargo test`
fn extract_command_basename(cmd: &str) -> &str {
    let first_space = cmd.find(' ').unwrap_or(cmd.len());
    let cmd_part = &cmd[..first_space];
    if let Some(slash_pos) = cmd_part.rfind('/') {
        &cmd[slash_pos + 1..]
    } else {
        cmd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for entry in REWRITE_COMMANDS {
            assert!(
                seen.insert(entry.command),
                "duplicate command: {}",
                entry.command
            );
        }
    }

    #[test]
    fn is_rewritable_matches() {
        assert!(is_rewritable_command("git status"));
        assert!(is_rewritable_command("cargo test --lib"));
        assert!(is_rewritable_command("npm run build"));
        assert!(is_rewritable_command("eslint"));
        assert!(is_rewritable_command("docker-compose up"));
        assert!(is_rewritable_command("bun install"));
        assert!(is_rewritable_command("bunx vitest"));
        assert!(is_rewritable_command("deno test"));
        assert!(is_rewritable_command("vite build"));
        assert!(is_rewritable_command("terraform plan"));
        assert!(is_rewritable_command("make build"));
        assert!(is_rewritable_command("dotnet build"));
        assert!(is_rewritable_command("python script.py"));
        assert!(is_rewritable_command("python3 -m pytest"));
        assert!(is_rewritable_command("uv run test"));
    }

    #[test]
    fn is_rewritable_env_prefix() {
        assert!(is_rewritable_command("PYTHONPATH=src python script.py"));
        assert!(is_rewritable_command("FOO=bar BAZ=1 cargo test"));
        assert!(is_rewritable_command("NODE_ENV=test npm run test"));
        assert!(!is_rewritable_command("FOO=bar echo hello"));
    }

    #[test]
    fn is_rewritable_path_qualified() {
        assert!(is_rewritable_command("./.venv/bin/python script.py"));
        assert!(is_rewritable_command("/usr/bin/python3 test.py"));
        assert!(is_rewritable_command("/usr/local/bin/cargo build"));
        assert!(is_rewritable_command(
            "PYTHONPATH=src ./.venv/bin/python script.py"
        ));
        assert!(!is_rewritable_command("/usr/bin/some-unknown-tool arg"));
    }

    #[test]
    fn is_rewritable_excludes() {
        assert!(!is_rewritable_command("echo hello"));
        assert!(!is_rewritable_command("cd src"));
        assert!(!is_rewritable_command("cat file.rs"));
        assert!(!is_rewritable_command("head -20 file.rs"));
        assert!(is_rewritable_command("rg pattern"));
        assert!(is_rewritable_command("grep -rn pattern src/"));
        assert!(is_rewritable_command("egrep 'foo|bar' file.rs"));
        assert!(is_rewritable_command("fgrep literal file.rs"));
        assert!(is_rewritable_command("ls /tmp"));
        assert!(is_rewritable_command("find . -name '*.rs'"));
    }

    #[test]
    fn strip_env_prefix_works() {
        assert_eq!(strip_env_prefix("python x.py"), "python x.py");
        assert_eq!(strip_env_prefix("FOO=bar python x.py"), "python x.py");
        assert_eq!(strip_env_prefix("A=1 B=2 cargo test"), "cargo test");
        assert_eq!(strip_env_prefix("  FOO=bar cmd"), "cmd");
        assert_eq!(strip_env_prefix("no_equals here"), "no_equals here");
    }

    #[test]
    fn extract_command_basename_works() {
        assert_eq!(extract_command_basename("python x.py"), "python x.py");
        assert_eq!(
            extract_command_basename("./.venv/bin/python x.py"),
            "python x.py"
        );
        assert_eq!(
            extract_command_basename("/usr/bin/python3 -m pytest"),
            "python3 -m pytest"
        );
        assert_eq!(extract_command_basename("cargo test"), "cargo test");
    }

    #[test]
    fn file_read_commands_detected() {
        assert!(is_file_read_command("cat file.rs"));
        assert!(is_file_read_command("head -20 file.rs"));
        assert!(is_file_read_command("tail -n 10 file.rs"));
        assert!(!is_file_read_command("git status"));
        assert!(!is_file_read_command("echo hello"));
    }

    /// Any command of a chain or pipeline that prints a file makes the line
    /// a file read; a command filtering the previous one's output does not.
    #[test]
    fn file_reads_are_found_in_every_command_of_a_line() {
        for cmd in [
            "sed -n '100,160p' src/x.py",
            "grep -n -A20 'def f' src/x.py",
            "cat a.py | head -50",
            "cd /repo && sed -n 625,705p tests/t.py; grep -n snooze src/x.py",
            "cd /repo; git log --oneline -5; sed -n 1,80p flows.py",
            "cd /repo && git log -3 && cat src/x.py",
            "nl -ba src/x.py | sed -n 1,80p",
            "awk 'NR>=10 && NR<=60' src/x.py",
            "FOO=1 /usr/bin/head -100 src/x.py",
            "rg -n -C3 'def f' src",
            "grep -rnA5 handler src",
            "grep -5 handler src/x.py",
            "grep --context=2 handler src/x.py",
            "git show HEAD:src/x.py",
            "git -C /repo show HEAD~2:src/x.py",
            "git blame src/x.py",
            "git cat-file -p HEAD:src/x.py",
            "git grep -n -B2 handler",
        ] {
            assert!(is_file_read_command(cmd), "{cmd}");
        }
        for cmd in [
            "git log | head -40",
            "cargo test 2>&1 | tail -30",
            "ps aux | awk '{print $2}'",
            "git log --oneline | sed -n 1,5p",
            "sed -i 's/a/b/' src/x.py",
            "cd /repo && sed -i.bak 's/a/b/' x.py && git diff --stat",
            "grep -rn handler src/",
            "grep -c handler src/x.py",
            "git show HEAD --stat",
            "git -C /repo log -3",
            "git status && git log",
            "pytest -q",
        ] {
            assert!(!is_file_read_command(cmd), "{cmd}");
        }
    }

    #[test]
    fn all_categories_represented() {
        let categories: std::collections::HashSet<_> =
            REWRITE_COMMANDS.iter().map(|e| e.category).collect();
        assert!(categories.contains(&Category::Vcs));
        assert!(categories.contains(&Category::Build));
        assert!(categories.contains(&Category::PackageManager));
        assert!(categories.contains(&Category::Lint));
        assert!(categories.contains(&Category::Infra));
        assert!(categories.contains(&Category::Http));
        assert!(categories.contains(&Category::Search));
        assert!(categories.contains(&Category::DirList));
    }

    #[test]
    fn every_command_rewritable_except_fileread() {
        for entry in REWRITE_COMMANDS {
            let cmd = format!("{} --version", entry.command);
            if matches!(entry.category, Category::FileRead) {
                assert!(
                    !is_rewritable_command(&cmd),
                    "{:?} command '{}' should NOT be rewritable via -c wrap",
                    entry.category,
                    entry.command
                );
            } else {
                assert!(
                    is_rewritable_command(&cmd),
                    "command '{}' should be rewritable",
                    entry.command
                );
            }
        }
    }
}
