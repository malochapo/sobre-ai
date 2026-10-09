//! The command line, declared once. This table drives:
//!
//! - flag checking: every command calls [`check_or_exit`], so an unknown
//!   flag, a missing value or an unexpected argument exits 2 before anything
//!   runs, and `--help` prints the command's entry;
//! - `sobre help all`, rendered by [`reference_text`];
//! - `docs/reference/generated/cli.md`, rendered by [`markdown`].
//!
//! A test (`every_flag_a_handler_reads_is_declared`) fails when a handler reads
//! a flag this table does not list.

/// One flag of a command.
pub(crate) struct Flag {
    /// `--name`.
    pub name: &'static str,
    /// `-x`, when the flag has a short form.
    pub short: Option<&'static str>,
    /// The value's placeholder (`<path>`) when the flag takes one, given as
    /// `--name <v>` or `--name=<v>`.
    pub value: Option<&'static str>,
    pub help: &'static str,
}

/// One command (or subcommand: `proxy start`).
pub(crate) struct Command {
    /// The words after `sobre`: `gain`, `proxy start`.
    pub path: &'static str,
    /// Its positional arguments, as shown in the usage line; empty when it
    /// takes none.
    pub args: &'static str,
    pub summary: &'static str,
    pub flags: &'static [Flag],
    /// More to say, shown by `--help` and in the reference; may be empty.
    pub details: &'static str,
    /// The arguments are a shell command, passed on unread (`-c`, `raw`).
    pub raw_args: bool,
}

const JSON: Flag = Flag {
    name: "--json",
    short: None,
    value: None,
    help: "Machine-readable output",
};

const ROOT: Flag = Flag {
    name: "--root",
    short: None,
    value: Some("<path>"),
    help: "The project (default: the git root around the current directory)",
};

const PORT: Flag = Flag {
    name: "--port",
    short: Some("-p"),
    value: Some("<n>"),
    help: "The proxy's port (default: the port it was last started on, else proxy.port)",
};

const fn cmd(path: &'static str, args: &'static str, summary: &'static str) -> Command {
    Command {
        path,
        args,
        summary,
        flags: &[],
        details: "",
        raw_args: false,
    }
}

/// Every command, in the order the reference lists them.
pub(crate) const COMMANDS: &[Command] = &[
    Command {
        details: "Agents start this themselves. Bare `sobre` does the same when \
                  stdin is not a terminal; in a terminal it prints a short \
                  quickstart instead.",
        ..cmd("mcp", "", "Run the MCP server on stdin/stdout")
    },
    Command {
        details: "What the Cursor and Gemini hooks run. The output says, on \
                  stderr, what was compressed; `sobre_expand` brings back the \
                  original. Exits with the command's own exit code.",
        raw_args: true,
        ..cmd(
            "-c",
            "<command>",
            "Run a shell command with compressed, labelled output",
        )
    },
    Command {
        raw_args: true,
        ..cmd(
            "raw",
            "<command>",
            "Run a shell command with its exact output, uncompressed",
        )
    },
    Command {
        flags: &[
            Flag {
                name: "--project-root",
                short: None,
                value: Some("<path>"),
                help: "The project the tool works in (default: the git root around the \
                       current directory)",
            },
            Flag {
                name: "--json",
                short: None,
                value: Some("<json>"),
                help: "The tool's arguments, as a JSON object",
            },
            Flag {
                name: "--json-file",
                short: None,
                value: Some("<path>"),
                help: "Read the arguments from this file",
            },
        ],
        details: "Each call starts an empty session, as a new server does. Exits \
                  1 when the tool reports an error, 2 on a usage error.",
        ..cmd("call", "<tool>", "Call one MCP tool from the shell")
    },
    Command {
        flags: &[
            Flag {
                name: "--check",
                short: None,
                value: None,
                help: "List the files init would create or change, and write nothing",
            },
            Flag {
                name: "--repair",
                short: None,
                value: None,
                help: "Ask no question (for scripts and CI)",
            },
            Flag {
                name: "--json",
                short: None,
                value: None,
                help: "Machine-readable report; asks no question",
            },
            Flag {
                name: "--agent",
                short: None,
                value: Some("<agent>"),
                help: "Same as naming the agent; repeatable",
            },
        ],
        details: "Without an agent: every installed agent gets its MCP entry, \
                  hooks and skill, and init asks once about the proxy and, in a \
                  build with embeddings, about downloading the model. With \
                  agents (claude, codex, copilot, cursor, gemini, opencode, \
                  vscode, windsurf, zed): only those, installed or not, with no \
                  question. Never edits shell rc files. Edits keep comments and \
                  key order; a file sobre edits for the first time is copied to \
                  <file>.sobre.bak. Exits 1 when an agent could not be \
                  connected. Undo it all: sobre uninstall.",
        ..cmd(
            "init",
            "[<agent>...]",
            "Connect agents to sobre: MCP entry, hooks, skill",
        )
    },
    Command {
        flags: &[
            Flag {
                name: "--fix",
                short: None,
                value: None,
                help: "Repair what can be repaired, and say what was done",
            },
            JSON,
        ],
        details: "Exits 1 when a check fails. More checks: sobre doctor \
                  integrations, doctor overhead, doctor lint-context.",
        ..cmd(
            "doctor",
            "",
            "Check the install: binary, config, agents, hooks, proxy, data",
        )
    },
    Command {
        flags: &[
            Flag {
                name: "--fix",
                short: None,
                value: None,
                help: "Repair the integrations first",
            },
            JSON,
        ],
        ..cmd("doctor integrations", "", "Check each agent's integration")
    },
    Command {
        flags: &[
            JSON,
            Flag {
                name: "--gate",
                short: None,
                value: None,
                help: "Exit 1 when the cost is over the 8,000-token budget",
            },
        ],
        ..cmd(
            "doctor overhead",
            "",
            "The fixed context cost sobre adds to every request",
        )
    },
    Command {
        flags: &[JSON],
        ..cmd(
            "doctor lint-context",
            "",
            "Low-signal or duplicate lines in the context sobre injects",
        )
    },
    Command {
        flags: &[
            Flag {
                name: "--by-tool",
                short: None,
                value: None,
                help: "Savings per tool or command",
            },
            Flag {
                name: "--daily",
                short: None,
                value: None,
                help: "Saved tokens and real cost per day",
            },
            JSON,
            Flag {
                name: "--period",
                short: None,
                value: Some("<all|today|Nd>"),
                help: "The period to cover (default all)",
            },
            Flag {
                name: "--limit",
                short: None,
                value: Some("<n>"),
                help: "Rows shown by --by-tool",
            },
            Flag {
                name: "--model",
                short: None,
                value: Some("<model>"),
                help: "Price saved tokens at this model's rate",
            },
            Flag {
                name: "--reset",
                short: None,
                value: None,
                help: "Clear the usage ledger",
            },
        ],
        details: "Every figure comes from the usage ledger (usage.jsonl). Real \
                  cost needs the proxy; money figures are estimates.",
        ..cmd(
            "gain",
            "",
            "Tokens saved, their value, real cost with the proxy, and overhead",
        )
    },
    Command {
        flags: &[
            Flag {
                name: "--force",
                short: None,
                value: None,
                help: "Drop the indexes first and rebuild from scratch",
            },
            ROOT,
        ],
        details: "The graph and BM25 indexes, and the dense index in a build \
                  with local embeddings. Leave files out with `ignore` in \
                  config.toml.",
        ..cmd("index build", "", "Build the project's indexes")
    },
    Command {
        flags: &[JSON, ROOT],
        ..cmd("index status", "", "What is built, and how large")
    },
    cmd("config", "", "Show the config file (same as config show)"),
    cmd("config show", "", "Show the config file"),
    Command {
        flags: &[Flag {
            name: "--yes",
            short: Some("-y"),
            value: None,
            help: "Skip the confirmation a change that widens what sobre does asks for",
        }],
        details: "Booleans take true/false (also yes/no, on/off, 1/0); a list \
                  takes comma-separated values. The value is checked before the \
                  file is written; the file keeps its comments. `default`, an \
                  empty value or the key's default value removes the key, so \
                  its default applies. Exits 2 on an unknown key or a value \
                  sobre would not use.",
        ..cmd("config set", "<key> <value>", "Change one key")
    },
    cmd("config path", "", "Print the config file's path"),
    Command {
        details: "Keeps an existing file as config.toml.sobre.bak.",
        ..cmd(
            "config init",
            "",
            "Write the config file with every key documented",
        )
    },
    cmd(
        "embeddings status",
        "",
        "The local ONNX runtime and embedding model (builds with local embeddings)",
    ),
    Command {
        flags: &[Flag {
            name: "--force",
            short: None,
            value: None,
            help: "Download again even when present",
        }],
        ..cmd(
            "embeddings provision",
            "",
            "Download the ONNX runtime and the model",
        )
    },
    Command {
        flags: &[
            Flag {
                name: "--port",
                short: Some("-p"),
                value: Some("<n>"),
                help: "The port to listen on (default proxy.port)",
            },
            Flag {
                name: "--detach",
                short: Some("-d"),
                value: None,
                help: "Run in the background",
            },
        ],
        ..cmd(
            "proxy start",
            "",
            "Run the API proxy (in the foreground unless --detach)",
        )
    },
    Command {
        flags: &[PORT],
        ..cmd(
            "proxy stop",
            "",
            "Unload the autostart unit and stop the proxy",
        )
    },
    Command {
        flags: &[PORT],
        ..cmd(
            "proxy restart",
            "",
            "Restart the proxy so it re-reads config.toml",
        )
    },
    Command {
        flags: &[PORT],
        ..cmd(
            "proxy status",
            "",
            "The proxy's config, process and live upstreams",
        )
    },
    Command {
        flags: &[Flag {
            name: "--force",
            short: None,
            value: None,
            help: "Also rewire agents that point to another endpoint",
        }],
        details: "Sets proxy.enabled, installs the autostart unit (macOS and \
                  Linux) and points the agents at the proxy. On Windows it \
                  refuses: start the proxy by hand there.",
        ..cmd("proxy enable", "", "Turn the proxy on")
    },
    cmd(
        "proxy disable",
        "",
        "Turn the proxy off and restore the agents' endpoints",
    ),
    cmd(
        "proxy cleanup",
        "",
        "Remove proxy URLs left in agent configs while the proxy is off",
    ),
    Command {
        flags: &[Flag {
            name: "--quiet",
            short: Some("-q"),
            value: None,
            help: "Print the token alone",
        }],
        ..cmd("proxy token", "", "Print the proxy's bearer token")
    },
    cmd(
        "hook",
        "<compress|cursor|gemini|read-dedup|observe>",
        "Internal: run by the hooks init installs",
    ),
    Command {
        flags: &[
            Flag {
                name: "--keep-config",
                short: None,
                value: None,
                help: "Keep config.toml, for a later reinstall",
            },
            Flag {
                name: "--keep-binary",
                short: None,
                value: None,
                help: "Leave the sobre binary in place",
            },
        ],
        details: "Removes the proxy (autostart unit, process, the URLs it put in \
                  agent configs); sobre's MCP entry, hooks, skill and plugin \
                  from every agent (a file that held nothing else is deleted); \
                  all sobre data; and the binary the agents run. Every change is \
                  listed as it is made. Running agents keep their sobre server \
                  until they restart. Any other argument is an error: nothing is \
                  removed.",
        ..cmd("uninstall", "", "Remove sobre from this machine")
    },
    cmd(
        "help",
        "[all]",
        "The common commands; `help all` prints this reference",
    ),
];

/// What the process's exit status means, for every command.
pub(crate) const EXIT_CODES: &[(i32, &str)] = &[
    (0, "Success"),
    (
        1,
        "The command ran and failed: a tool error (`call`), a failed check \
         (`doctor`, `doctor overhead --gate`), an agent not connected \
         (`init`), a proxy that would not start",
    ),
    (
        2,
        "Usage error: an unknown command, flag or argument, or a flag without \
         its value. Nothing ran.",
    ),
];

/// The entry for `path`. Panics on a path the table does not list: every
/// caller passes a literal.
pub(crate) fn command(path: &str) -> &'static Command {
    COMMANDS
        .iter()
        .find(|c| c.path == path)
        .unwrap_or_else(|| panic!("no command '{path}' in cli::spec::COMMANDS"))
}

/// What [`check`] found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Checked {
    Ok,
    Help,
}

/// Check `args` (the words after the command's path) against its entry:
/// every flag is listed, every flag that takes a value has one, and
/// positional arguments appear only where the command takes some.
pub(crate) fn check(cmd: &Command, args: &[String]) -> Result<Checked, String> {
    if cmd.raw_args {
        return Ok(Checked::Ok);
    }
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--help" || arg == "-h" {
            return Ok(Checked::Help);
        }
        if !arg.starts_with('-') || arg == "-" {
            if cmd.args.is_empty() {
                return Err(format!("unexpected argument '{arg}'"));
            }
            continue;
        }
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (arg.as_str(), None),
        };
        let Some(flag) = cmd
            .flags
            .iter()
            .find(|f| f.name == name || f.short == Some(name))
        else {
            return Err(format!("unknown flag '{name}'"));
        };
        match (flag.value, inline) {
            (Some(_), Some("")) => {
                return Err(format!("{} needs a value", flag.name));
            }
            (Some(_), None) => {
                if it.next().is_none() {
                    return Err(format!("{} needs a value", flag.name));
                }
            }
            (None, Some(_)) => return Err(format!("{} takes no value", flag.name)),
            (Some(_), Some(_)) | (None, None) => {}
        }
    }
    Ok(Checked::Ok)
}

/// [`check`] for a command about to run: `--help` prints its entry and exits
/// 0; a usage error prints what is wrong and the usage line, and exits 2.
pub(crate) fn check_or_exit(path: &str, args: &[String]) {
    let cmd = command(path);
    match check(cmd, args) {
        Ok(Checked::Ok) => {}
        Ok(Checked::Help) => {
            print!("{}", command_text(cmd));
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("sobre {path}: {e}");
            eprintln!("usage: {}", usage(cmd));
            eprintln!("       sobre {path} --help for more");
            std::process::exit(2);
        }
    }
}

/// `sobre <path> [<args>] [flags]`.
pub(crate) fn usage(cmd: &Command) -> String {
    let mut out = format!("sobre {}", cmd.path);
    if !cmd.args.is_empty() {
        out.push(' ');
        out.push_str(cmd.args);
    }
    for f in cmd.flags {
        out.push_str(&format!(" [{}]", flag_label(f)));
    }
    out
}

fn flag_label(f: &Flag) -> String {
    let mut label = f.name.to_string();
    if let Some(v) = f.value {
        label.push(' ');
        label.push_str(v);
    }
    label
}

fn flag_names(f: &Flag) -> String {
    let mut names = flag_label(f);
    if let Some(s) = f.short {
        names = format!("{s}, {names}");
    }
    names
}

/// One command's entry, as `--help` prints it.
pub(crate) fn command_text(cmd: &Command) -> String {
    let mut out = format!("{}\n\nusage: {}\n", cmd.summary, usage(cmd));
    if !cmd.flags.is_empty() {
        out.push('\n');
        for f in cmd.flags {
            out.push_str(&format!("  {:<26} {}\n", flag_names(f), f.help));
        }
    }
    if !cmd.details.is_empty() {
        out.push('\n');
        out.push_str(&wrap(cmd.details, 76));
        out.push('\n');
    }
    out
}

/// The commands under `group` (`proxy`), as `sobre proxy --help` prints them.
pub(crate) fn group_text(group: &str) -> String {
    let prefix = format!("{group} ");
    let mut out = format!("usage: sobre {group} <command>\n\n");
    for cmd in COMMANDS.iter().filter(|c| c.path.starts_with(&prefix)) {
        let sub = &cmd.path[prefix.len()..];
        out.push_str(&format!("  {sub:<14} {}\n", cmd.summary));
    }
    out.push_str(&format!("\nsobre {group} <command> --help for its flags\n"));
    out
}

/// The full reference, as `sobre help all` prints it.
pub(crate) fn reference_text() -> String {
    let mut out = String::new();
    for cmd in COMMANDS {
        out.push_str(&format!("  {}\n", usage(cmd)));
        out.push_str(&format!("      {}\n", cmd.summary));
        for f in cmd.flags {
            out.push_str(&format!("      {:<24} {}\n", flag_names(f), f.help));
        }
    }
    out.push_str("\n  sobre --version, -V          Print the version\n");
    out.push_str("  sobre <command> --help       One command's flags and details\n");
    out.push_str("\nEXIT CODES:\n");
    for (code, meaning) in EXIT_CODES {
        out.push_str(&format!("  {code}  {meaning}\n"));
    }
    out
}

/// `docs/reference/generated/cli.md`.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn markdown() -> String {
    let mut out = String::new();
    out.push_str("# Appendix — Command Line (generated)\n\n");
    out.push_str(
        "<!-- GENERATED FILE — do not edit by hand. Run: \
         `cargo run --example gen_docs --features dev-tools` -->\n\n",
    );
    out.push_str(
        "Source of truth: `rust/src/cli/spec.rs`. The same table checks every \
         command's flags (an unknown flag, a missing value or an unexpected \
         argument exits 2 before anything runs) and prints `sobre help all` \
         and `sobre <command> --help`.\n\n",
    );
    out.push_str("Global: `sobre --version` (`-V`) prints the version; `--help` (`-h`) after any command prints its entry.\n\n");
    for cmd in COMMANDS {
        out.push_str(&format!("## `sobre {}`\n\n", cmd.path));
        out.push_str(&format!("{}.\n\n", cmd.summary));
        out.push_str(&format!("```text\n{}\n```\n\n", usage(cmd)));
        if !cmd.flags.is_empty() {
            out.push_str("| Flag | Meaning |\n|---|---|\n");
            for f in cmd.flags {
                out.push_str(&format!("| `{}` | {} |\n", flag_names(f), f.help));
            }
            out.push('\n');
        }
        if !cmd.details.is_empty() {
            out.push_str(cmd.details);
            out.push_str("\n\n");
        }
    }
    out.push_str("## Exit codes\n\n| Code | Meaning |\n|---|---|\n");
    for (code, meaning) in EXIT_CODES {
        out.push_str(&format!("| {code} | {meaning} |\n"));
    }
    out
}

fn wrap(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            out.push_str(&line);
            out.push('\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    out.push_str(&line);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn flags_are_checked_against_the_table() {
        let gain = command("gain");
        assert_eq!(
            check(gain, &args(&["--period", "7d", "--json"])),
            Ok(Checked::Ok)
        );
        assert_eq!(check(gain, &args(&["--period=7d"])), Ok(Checked::Ok));
        assert!(check(gain, &args(&["--perod=7d"])).is_err());
        assert!(check(gain, &args(&["--period"])).is_err());
        assert!(check(gain, &args(&["--json=yes"])).is_err());
        assert!(check(gain, &args(&["extra"])).is_err());
        assert_eq!(check(gain, &args(&["--help"])), Ok(Checked::Help));
        // A value is never read as a flag.
        let call = command("call");
        assert_eq!(
            check(call, &args(&["sobre_tree", "--json", "-h"])),
            Ok(Checked::Ok)
        );
        // `-c` and `raw` take a shell command, unread.
        assert_eq!(
            check(command("-c"), &args(&["ls", "--color"])),
            Ok(Checked::Ok)
        );
        let proxy = command("proxy start");
        assert_eq!(check(proxy, &args(&["-p", "9000", "-d"])), Ok(Checked::Ok));
        // `--help` after a verb never runs it.
        assert_eq!(
            check(command("proxy enable"), &args(&["--help"])),
            Ok(Checked::Help)
        );
        assert_eq!(
            check(command("uninstall"), &args(&["-h"])),
            Ok(Checked::Help)
        );
        // `uninstall claude` is an error, never a full uninstall.
        assert!(check(command("uninstall"), &args(&["claude"])).is_err());
    }

    #[test]
    fn every_path_is_unique_and_every_flag_named() {
        let mut paths: Vec<&str> = COMMANDS.iter().map(|c| c.path).collect();
        paths.sort_unstable();
        let before = paths.len();
        paths.dedup();
        assert_eq!(paths.len(), before, "a path is listed twice");
        for cmd in COMMANDS {
            for f in cmd.flags {
                assert!(f.name.starts_with("--"), "{}: {}", cmd.path, f.name);
                assert!(!f.help.is_empty(), "{}: {}", cmd.path, f.name);
            }
        }
    }

    /// Every flag literal a command handler's source reads is in the table,
    /// so the table cannot fall behind the parsers.
    #[test]
    fn every_flag_a_handler_reads_is_declared() {
        let sources = [
            include_str!("call_cmd.rs"),
            include_str!("config_cmd.rs"),
            include_str!("embeddings_cmd.rs"),
            include_str!("index_cmd.rs"),
            include_str!("init_cmd.rs"),
            include_str!("prompt.rs"),
            include_str!("common.rs"),
            include_str!("dispatch/analytics/gain.rs"),
            include_str!("dispatch/network/proxy.rs"),
            include_str!("../doctor/mod.rs"),
            include_str!("../uninstall/mod.rs"),
        ];
        let declared: Vec<&str> = COMMANDS
            .iter()
            .flat_map(|c| c.flags.iter())
            .flat_map(|f| [Some(f.name), f.short])
            .flatten()
            .chain(["--help", "-h"])
            .collect();
        let re = regex::Regex::new(r#""(--?[a-z][a-z0-9-]*)=?""#).unwrap();
        for src in sources {
            // Only the code: a test module's literals are inputs, not flags.
            let code = src.split("#[cfg(test)]").next().unwrap_or(src);
            for cap in re.captures_iter(code) {
                let flag = &cap[1];
                assert!(
                    declared.contains(&flag),
                    "a handler reads {flag}, which cli::spec does not declare"
                );
            }
        }
    }

    #[test]
    fn the_reference_names_every_command() {
        let text = reference_text();
        for cmd in COMMANDS {
            assert!(
                text.contains(&format!("sobre {}", cmd.path)),
                "{}",
                cmd.path
            );
        }
    }
}
