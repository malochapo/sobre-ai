//! The command reference, rendered from the clap declarations in
//! [`super::args`]: `sobre help all` ([`reference_text`]) and
//! `docs/reference/generated/cli.md` ([`markdown`]).

use clap::{Arg, ArgAction, CommandFactory};

use super::args::Cli;

/// What the process's exit status means, for every command.
const EXIT_CODES: &[(i32, &str)] = &[
    (0, "Success"),
    (
        1,
        "The command ran and failed: a tool error (`call`), a failed check \
         (`doctor`, `doctor integrations`, an error from `doctor lint-context`, \
         `doctor overhead --gate`), an agent not connected (`init`), a proxy \
         that would not start, a `config set` not confirmed",
    ),
    (
        2,
        "Usage error: an unknown command, flag or argument, or a flag without \
         its value. Nothing ran.",
    ),
];

/// The exception to [`EXIT_CODES`].
const EXIT_CODE_EXCEPTION: &str = "`-c` and `raw` exit with the command's own exit code.";

/// One command the reference lists: the words after `sobre`, and its
/// declaration.
struct Entry<'a> {
    path: String,
    cmd: &'a clap::Command,
}

/// Every command that runs, depth first in declaration order. A group that
/// needs a subcommand (`proxy`) is not listed itself, only its subcommands.
fn entries(root: &clap::Command, include_hidden: bool) -> Vec<Entry<'_>> {
    fn walk<'a>(
        cmd: &'a clap::Command,
        prefix: &str,
        include_hidden: bool,
        out: &mut Vec<Entry<'a>>,
    ) {
        for sub in cmd.get_subcommands() {
            if sub.is_hide_set() && !include_hidden {
                continue;
            }
            let path = if prefix.is_empty() {
                sub.get_name().to_string()
            } else {
                format!("{prefix} {}", sub.get_name())
            };
            if !sub.is_subcommand_required_set() {
                out.push(Entry {
                    path: path.clone(),
                    cmd: sub,
                });
            }
            walk(sub, &path, include_hidden, out);
        }
    }
    let mut out = Vec::new();
    walk(root, "", include_hidden, &mut out);
    out
}

fn about(cmd: &clap::Command) -> String {
    cmd.get_about().map(ToString::to_string).unwrap_or_default()
}

/// The details paragraph, on one line.
#[cfg(any(test, feature = "test-support"))]
fn details(cmd: &clap::Command) -> String {
    cmd.get_after_help()
        .map(|d| {
            d.to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn takes_value(arg: &Arg) -> bool {
    arg.get_action().takes_values()
}

fn value_name(arg: &Arg) -> String {
    arg.get_value_names()
        .and_then(|names| names.first())
        .map_or_else(|| arg.get_id().to_string(), ToString::to_string)
}

/// `<tool>`, `[<agent>...]`, `<compress|cursor|...>`, `[all]`.
fn positional(arg: &Arg) -> String {
    let values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|v| !v.is_hide_set())
        .map(|v| v.get_name().to_string())
        .collect();
    let many = if matches!(arg.get_action(), ArgAction::Append) {
        "..."
    } else {
        ""
    };
    match (values.is_empty(), arg.is_required_set()) {
        (true, true) => format!("<{}>{many}", value_name(arg)),
        (true, false) => format!("[<{}>{many}]", value_name(arg)),
        (false, true) => format!("<{}>{many}", values.join("|")),
        (false, false) => format!("[{}{many}]", values.join("|")),
    }
}

/// `--port <n>`.
fn flag_label(arg: &Arg) -> String {
    let mut label = format!("--{}", arg.get_long().unwrap_or_default());
    if takes_value(arg) {
        label.push_str(&format!(" <{}>", value_name(arg)));
    }
    label
}

/// How an argument is named in a table: `-p, --port <n>`, `<key>`.
fn arg_names(arg: &Arg) -> String {
    if arg.is_positional() {
        return positional(arg);
    }
    match arg.get_short() {
        Some(short) => format!("-{short}, {}", flag_label(arg)),
        None => flag_label(arg),
    }
}

/// An argument's help, with its default value when it has one.
fn arg_help(arg: &Arg) -> String {
    let mut help = arg.get_help().map(ToString::to_string).unwrap_or_default();
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|v| v.to_string_lossy().into_owned())
        .collect();
    if takes_value(arg) && !defaults.is_empty() {
        help.push_str(&format!(" (default: {})", defaults.join(", ")));
    }
    help
}

/// The arguments a command lists, positionals first.
fn args_of(cmd: &clap::Command) -> Vec<&Arg> {
    let (positionals, flags): (Vec<&Arg>, Vec<&Arg>) = cmd
        .get_arguments()
        .filter(|a| !a.is_hide_set())
        .partition(|a| a.is_positional());
    positionals.into_iter().chain(flags).collect()
}

/// `sobre <path> <positionals> [flags]`.
fn usage(entry: &Entry<'_>) -> String {
    let mut out = format!("sobre {}", entry.path);
    for arg in args_of(entry.cmd) {
        out.push(' ');
        if arg.is_positional() {
            out.push_str(&positional(arg));
        } else {
            out.push_str(&format!("[{}]", flag_label(arg)));
        }
    }
    out
}

/// The full reference, as `sobre help all` prints it.
pub(crate) fn reference_text() -> String {
    let mut out = String::new();
    let root = Cli::command();
    for entry in entries(&root, false) {
        out.push_str(&format!("  {}\n", usage(&entry)));
        out.push_str(&format!("      {}\n", about(entry.cmd)));
        for arg in args_of(entry.cmd) {
            out.push_str(&format!("      {:<24} {}\n", arg_names(arg), arg_help(arg)));
        }
    }
    out.push_str("\n  sobre --version, -V          Print the version\n");
    out.push_str("  sobre <command> --help       One command's flags and details\n");
    out.push_str("\nEXIT CODES:\n");
    for (code, meaning) in EXIT_CODES {
        out.push_str(&format!("  {code}  {meaning}\n"));
    }
    out.push_str(&format!("  {EXIT_CODE_EXCEPTION}\n"));
    out
}

/// `docs/reference/generated/cli.md`. It also lists the internal `hook`
/// command, which `help all` leaves out.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn markdown() -> String {
    let mut out = String::new();
    out.push_str("# Appendix — Command Line (generated)\n\n");
    out.push_str(
        "<!-- GENERATED FILE — do not edit by hand. Run: \
         `cargo run --example gen_docs --features dev-tools` -->\n\n",
    );
    out.push_str(
        "Source of truth: `rust/src/cli/args.rs`, the clap declarations of every \
         command. They parse the command line (an unknown flag, a missing value \
         or an unexpected argument exits 2 before anything runs) and print \
         `sobre <command> --help`; `sobre help all` and this page are rendered \
         from them.\n\n",
    );
    out.push_str(
        "Global: `sobre --version` (`-V`) prints the version; `--help` (`-h`) \
         after any command but `-c` and `raw`, which take the rest of the line, \
         prints its help.\n\n",
    );
    let root = Cli::command();
    for entry in entries(&root, true) {
        out.push_str(&format!("## `sobre {}`\n\n", entry.path));
        out.push_str(&format!("{}.\n\n", about(entry.cmd)));
        out.push_str(&format!("```text\n{}\n```\n\n", usage(&entry)));
        let args = args_of(entry.cmd);
        if !args.is_empty() {
            out.push_str("| Argument | Meaning |\n|---|---|\n");
            for arg in args {
                out.push_str(&format!(
                    "| `{}` | {} |\n",
                    arg_names(arg).replace('|', "\\|"),
                    arg_help(arg).replace('|', "\\|")
                ));
            }
            out.push('\n');
        }
        let details = details(entry.cmd);
        if !details.is_empty() {
            out.push_str(&details);
            out.push_str("\n\n");
        }
    }
    out.push_str("## Exit codes\n\n| Code | Meaning |\n|---|---|\n");
    for (code, meaning) in EXIT_CODES {
        out.push_str(&format!("| {code} | {meaning} |\n"));
    }
    out.push_str(&format!("\n{EXIT_CODE_EXCEPTION}\n"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reference_names_every_command_but_the_hidden_one() {
        let text = reference_text();
        let root = Cli::command();
        for entry in entries(&root, false) {
            assert!(
                text.contains(&format!("sobre {}", entry.path)),
                "{}",
                entry.path
            );
        }
        for path in ["gain", "-c", "config set", "proxy start", "doctor overhead"] {
            assert!(text.contains(&format!("  sobre {path}")), "{path}");
        }
        assert!(!text.contains("sobre hook"), "{text}");
        assert!(markdown().contains("## `sobre hook`"));
    }

    /// Every command says what it does, every argument what it is for, and
    /// every value has a name.
    #[test]
    fn every_command_and_argument_is_described() {
        let root = Cli::command();
        for entry in entries(&root, true) {
            assert!(!about(entry.cmd).is_empty(), "{}", entry.path);
            for arg in args_of(entry.cmd) {
                let id = arg.get_id();
                assert!(arg.get_help().is_some(), "{}: {id}", entry.path);
                if takes_value(arg) && arg.get_possible_values().is_empty() {
                    assert!(arg.get_value_names().is_some(), "{}: {id}", entry.path);
                }
            }
        }
    }

    #[test]
    fn usage_lines_name_arguments_and_flags() {
        let md = markdown();
        for line in [
            "sobre -c <command>...",
            "sobre call <tool> [--project-root <path>] [--json <json>] [--json-file <path>]",
            "sobre init [<agent>...] [--mcp] [--check] [--repair] [--json] [--agent <agent>]",
            "sobre config set <key> <value> [--yes]",
            "sobre proxy start [--port <n>] [--detach]",
            "sobre hook <compress|cursor|gemini|read-dedup|observe>",
            "sobre help [all]",
        ] {
            assert!(md.contains(&format!("```text\n{line}\n```")), "{line}");
        }
        assert!(md.contains("| `-p, --port <n>` |"));
        assert!(md.contains("(default: all)"));
    }
}
