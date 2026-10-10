//! The command line, declared once with clap. These declarations:
//!
//! - parse every command: an unknown flag, a missing value or an unexpected
//!   argument exits 2 before anything runs;
//! - print `sobre <command> --help`;
//! - are walked by [`super::reference`] for `sobre help all` and
//!   `docs/reference/generated/cli.md`.
//!
//! Each handler takes the struct or enum of its command. Help text is plain
//! and wrapped by hand: clap's wrapping (`wrap_help`) is not built in.

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};

/// `sobre`. Without a command it serves MCP when stdin is not a terminal, and
/// prints the quickstart when it is.
#[derive(Debug, Parser)]
#[command(
    name = "sobre",
    bin_name = "sobre",
    version,
    // `sobre help [all]` is sobre's own command; `<command> --help` prints a
    // command's help.
    disable_help_subcommand = true
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

/// A usage error only the handler of `command` can see, printed as clap
/// prints its own; exits 2.
pub(crate) fn usage_error(command: &str, message: &str) -> ! {
    let mut cli = Cli::command();
    cli.build();
    match cli.find_subcommand_mut(command) {
        Some(cmd) => cmd.error(clap::error::ErrorKind::ValueValidation, message),
        None => cli.error(clap::error::ErrorKind::ValueValidation, message),
    }
    .exit()
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Run the MCP server on stdin/stdout
    #[command(after_help = "\
Agents start this themselves. Bare `sobre` does the same when stdin is not a
terminal; in a terminal it prints a short quickstart instead.")]
    Mcp,

    /// Run a shell command with compressed, labelled output
    // `<binary> -c '<command>'` is the wire form the Cursor and Gemini hooks
    // emit (hook_handlers/rewrite.rs), and shell/exec.rs matches on it when
    // collapsing nesting: it stays a flag. Named `-c`, it has no other name.
    #[command(
        name = "-c",
        short_flag = 'c',
        override_usage = "sobre -c <command>...",
        after_help = "\
What the Cursor and Gemini hooks run. The output says, on stderr, what was
compressed; `sobre_expand` brings back the original. Exits with the
command's own exit code."
    )]
    Exec(ShellCommand),

    /// Run a shell command with its exact output, uncompressed
    #[command(after_help = "Exits with the command's own exit code.")]
    Raw(ShellCommand),

    /// Call one MCP tool from the shell
    #[command(after_help = "\
Each call starts an empty session, as a new server does. Exits 1 when the
tool reports an error, 2 on a usage error.")]
    Call(CallArgs),

    /// Connect agents to sobre: MCP entry, hooks, skill
    #[command(after_help = "\
Without an agent: every installed agent gets its MCP entry, hooks and skill,
and init asks once about the proxy and, in a build with embeddings, about
downloading the model. With agents (claude, codex, copilot, cursor, gemini,
opencode, vscode, windsurf, zed): only those, installed or not, with no
question. Never edits shell rc files. Edits keep comments and key order; a
file sobre edits for the first time is copied to <file>.sobre.bak. Exits 1
when an agent could not be connected. Undo it all: sobre uninstall.")]
    Init(InitArgs),

    /// Check the install: binary, config, agents, hooks, proxy, data
    #[command(after_help = "\
Exits 1 when a check fails. More checks: sobre doctor integrations, doctor
overhead, doctor lint-context.")]
    Doctor(DoctorArgs),

    /// Tokens saved, their value, real cost with the proxy, and overhead
    #[command(after_help = "\
Every figure comes from the usage ledger (usage.jsonl). Real cost needs the
proxy; money figures are estimates.")]
    Gain(GainArgs),

    /// The project's indexes: build them, or say what is built
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },

    /// Show the config file (same as config show)
    Config {
        #[command(subcommand)]
        command: Option<ConfigCommand>,
    },

    /// The local ONNX runtime and embedding model (same as embeddings status)
    Embeddings {
        #[command(subcommand)]
        command: Option<EmbeddingsCommand>,
    },

    /// The local API proxy: run it, stop it, wire the agents to it
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },

    /// Internal: run by the hooks init installs
    #[command(hide = true)]
    Hook {
        /// The hook that fired
        #[arg(value_enum, value_name = "action")]
        action: HookAction,
    },

    /// Remove sobre from this machine
    #[command(after_help = "\
Removes the proxy (autostart unit, process, the URLs it put in agent
configs); sobre's MCP entry, hooks, skill and plugin from every agent (a
file that held nothing else is deleted); all sobre data; and the binary the
agents run, unless cargo installed it (cargo uninstall sobre) or it is a
build in a cargo target directory. On Windows a running binary cannot be
deleted: delete it once uninstall has exited. Every change is listed as it
is made. Running agents keep their sobre server until they restart. Any
other argument is an error: nothing is removed.")]
    Uninstall(UninstallArgs),

    /// The common commands; `help all` prints the full reference
    Help {
        /// Print every command, flag and exit code instead
        #[arg(value_enum, value_name = "topic")]
        topic: Option<HelpTopic>,
    },
}

/// The command `-c` and `raw` run: the rest of the line, read as it is.
#[derive(Debug, Args)]
pub(crate) struct ShellCommand {
    /// The command: one quoted argument, or several joined with spaces
    #[arg(
        value_name = "command",
        required = true,
        num_args = 1..,
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub(crate) command: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct CallArgs {
    /// The tool's name: sobre_read, sobre_search, ...
    #[arg(value_name = "tool")]
    pub(crate) tool: String,
    /// The project the tool works in (default: the git root around the
    /// current directory)
    #[arg(long, value_name = "path")]
    pub(crate) project_root: Option<String>,
    /// The tool's arguments, as a JSON object
    #[arg(long, value_name = "json", conflicts_with = "json_file")]
    pub(crate) json: Option<String>,
    /// Read the arguments from this file
    #[arg(long, value_name = "path")]
    pub(crate) json_file: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// The agents to connect
    #[arg(value_name = "agent", value_parser = agent)]
    pub(crate) agents: Vec<&'static str>,
    /// List the files init would create or change, and write nothing
    #[arg(long)]
    pub(crate) check: bool,
    /// Ask no question (for scripts and CI)
    #[arg(long)]
    pub(crate) repair: bool,
    /// Machine-readable report; asks no question
    #[arg(long)]
    pub(crate) json: bool,
    /// Same as naming the agent; repeatable
    #[arg(long = "agent", value_name = "agent", value_parser = agent)]
    pub(crate) agent: Vec<&'static str>,
}

impl InitArgs {
    /// The agents named either way, each once.
    pub(crate) fn named(&self) -> Vec<&'static str> {
        let mut named: Vec<&'static str> = Vec::new();
        for key in self.agents.iter().chain(&self.agent) {
            if !named.contains(key) {
                named.push(key);
            }
        }
        named
    }
}

/// An agent `init` knows, as [`crate::setup::agent_key`] names it.
fn agent(name: &str) -> Result<&'static str, String> {
    crate::setup::agent_key(name).ok_or_else(|| {
        "unknown agent (init knows claude, codex, copilot, cursor, gemini, opencode, vscode, \
         windsurf, zed)"
            .to_string()
    })
}

#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub(crate) struct DoctorArgs {
    #[command(subcommand)]
    pub(crate) check: Option<DoctorCheck>,
    /// Repair the agents that have sobre, drop stale indexes, and say what
    /// was done
    #[arg(long)]
    pub(crate) fix: bool,
    /// Machine-readable output
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum DoctorCheck {
    /// Check each agent's integration
    Integrations {
        /// Repair the integrations first
        #[arg(long)]
        fix: bool,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// The fixed context cost sobre adds to every request
    Overhead {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
        /// Exit 1 when the cost is over the 8,000-token budget
        #[arg(long)]
        gate: bool,
    },
    /// Low-signal or duplicate lines in the context sobre injects
    LintContext {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct GainArgs {
    /// Savings per tool or command
    #[arg(long)]
    pub(crate) by_tool: bool,
    /// Saved tokens and real cost per day
    #[arg(long)]
    pub(crate) daily: bool,
    /// Machine-readable output
    #[arg(long)]
    pub(crate) json: bool,
    /// The period to cover
    #[arg(
        long,
        value_name = "all|today|Nd",
        default_value = "all",
        value_parser = period
    )]
    pub(crate) period: PeriodArg,
    /// Rows shown by --by-tool
    #[arg(long, value_name = "n", default_value_t = 10)]
    pub(crate) limit: usize,
    /// Price saved tokens at this model's rate
    #[arg(long, value_name = "model")]
    pub(crate) model: Option<String>,
    /// Clear the usage ledger
    #[arg(long)]
    pub(crate) reset: bool,
}

/// `--period`, as written and as read.
#[derive(Debug, Clone)]
pub(crate) struct PeriodArg {
    pub(crate) label: String,
    pub(crate) period: crate::core::usage::Period,
}

fn period(s: &str) -> Result<PeriodArg, String> {
    crate::core::usage::Period::parse(s)
        .map(|period| PeriodArg {
            label: s.to_string(),
            period,
        })
        .ok_or_else(|| "use all, today, or <n>d (7d, 30d)".to_string())
}

#[derive(Debug, Subcommand)]
pub(crate) enum IndexCommand {
    /// Build the project's indexes
    #[command(after_help = "\
The graph and BM25 indexes, and the dense index in a build with local
embeddings. Leave files out with `ignore` in config.toml.")]
    Build {
        /// Drop the indexes first and rebuild from scratch
        #[arg(long)]
        force: bool,
        #[command(flatten)]
        root: RootArg,
    },
    /// What is built, and how large
    Status {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        root: RootArg,
    },
}

#[derive(Debug, Args)]
pub(crate) struct RootArg {
    /// The project (default: the git root around the current directory)
    #[arg(long, value_name = "path")]
    pub(crate) root: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConfigCommand {
    /// Show the config file
    Show,
    /// Change one key
    #[command(after_help = "\
Booleans take true/false (also yes/no, 1/0); a list takes
comma-separated values. The value is checked before the file is written;
the file keeps its comments. `default`, an empty value or the key's default
value removes the key, so its default applies. Exits 2 on an unknown key or
a value sobre would not use.")]
    Set(ConfigSetArgs),
    /// Print the config file's path
    Path,
    /// Write the config file with every key documented
    #[command(after_help = "Keeps an existing file as config.toml.sobre.bak.")]
    Init,
}

#[derive(Debug, Args)]
pub(crate) struct ConfigSetArgs {
    /// A key `sobre config` lists
    #[arg(value_name = "key")]
    pub(crate) key: String,
    /// Its new value
    #[arg(value_name = "value")]
    pub(crate) value: String,
    /// Skip the confirmation a change that widens what sobre does asks for
    #[arg(short, long)]
    pub(crate) yes: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum EmbeddingsCommand {
    /// The local ONNX runtime and embedding model (builds with local
    /// embeddings)
    Status,
    /// Download the ONNX runtime and the model
    Provision {
        /// Download again even when present
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProxyCommand {
    /// Run the API proxy (in the foreground unless --detach)
    Start {
        /// The port to listen on (default proxy.port)
        #[arg(short, long, value_name = "n", value_parser = clap::value_parser!(u16).range(1..))]
        port: Option<u16>,
        /// Run in the background
        #[arg(short, long)]
        detach: bool,
    },
    /// Unload the autostart unit and stop the proxy
    Stop(PortArg),
    /// Restart the proxy so it re-reads config.toml
    Restart(PortArg),
    /// The proxy's config, process and live upstreams
    Status(PortArg),
    /// Turn the proxy on
    #[command(after_help = "\
Sets proxy.enabled, installs the autostart unit (launchd on macOS, a systemd
user service on Linux) and points the agents at the proxy. Where there is no
service manager (Windows, Linux without a systemd user session), it needs a
proxy started by hand first (sobre proxy start -d), and refuses without one.
Exits 1 when the proxy does not run.")]
    Enable {
        /// Also rewire agents that point to another endpoint
        #[arg(long)]
        force: bool,
    },
    /// Turn the proxy off and restore the agents' endpoints
    Disable,
    /// Remove proxy URLs left in agent configs while the proxy is off
    Cleanup,
    /// Print the proxy's bearer token
    Token {
        /// Print the token alone
        #[arg(short, long)]
        quiet: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct PortArg {
    /// The proxy's port (default: the port it was last started on, else
    /// proxy.port)
    #[arg(short, long, value_name = "n", value_parser = clap::value_parser!(u16).range(1..))]
    pub(crate) port: Option<u16>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum HookAction {
    Compress,
    Cursor,
    Gemini,
    ReadDedup,
    Observe,
}

#[derive(Debug, Args)]
pub(crate) struct UninstallArgs {
    /// Keep config.toml, for a later reinstall
    #[arg(long)]
    pub(crate) keep_config: bool,
    /// Leave the sobre binary in place
    #[arg(long)]
    pub(crate) keep_binary: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum HelpTopic {
    All,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

    fn parse(args: &[&str]) -> Result<Cli, ErrorKind> {
        Cli::try_parse_from(std::iter::once("sobre").chain(args.iter().copied()))
            .map_err(|e| e.kind())
    }

    /// clap's own checks: unique names and flags, settings that fit.
    #[test]
    fn the_declarations_are_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn flags_are_checked() {
        assert!(parse(&["gain", "--period", "7d", "--json"]).is_ok());
        assert!(parse(&["gain", "--period=7d"]).is_ok());
        assert_eq!(
            parse(&["gain", "--perod=7d"]).err(),
            Some(ErrorKind::UnknownArgument)
        );
        assert!(parse(&["gain", "--period"]).is_err());
        assert!(parse(&["gain", "--period=week"]).is_err());
        assert!(parse(&["gain", "--json=yes"]).is_err());
        assert!(parse(&["gain", "extra"]).is_err());
        assert_eq!(
            parse(&["gain", "--help"]).err(),
            Some(ErrorKind::DisplayHelp)
        );
        assert!(parse(&["proxy", "start", "-p", "9000", "-d"]).is_ok());
        assert!(parse(&["proxy", "start", "--port=0"]).is_err());
        // `--help` after a verb never runs it.
        assert_eq!(
            parse(&["proxy", "enable", "--help"]).err(),
            Some(ErrorKind::DisplayHelp)
        );
        assert_eq!(
            parse(&["uninstall", "-h"]).err(),
            Some(ErrorKind::DisplayHelp)
        );
        // `uninstall claude` is an error, never a full uninstall.
        assert!(parse(&["uninstall", "claude"]).is_err());
        assert!(parse(&["uninstall", "--dryrun"]).is_err());
    }

    /// `-c` and `raw` take the rest of the line as it is, flags included.
    #[test]
    fn shell_commands_are_read_whole() {
        let Ok(Cli {
            command: Some(Command::Exec(sh)),
        }) = parse(&["-c", "ls", "--color", "-la"])
        else {
            panic!("-c did not parse");
        };
        assert_eq!(sh.command, ["ls", "--color", "-la"]);
        let Ok(Cli {
            command: Some(Command::Raw(sh)),
        }) = parse(&["raw", "echo hi"])
        else {
            panic!("raw did not parse");
        };
        assert_eq!(sh.command, ["echo hi"]);
        assert!(parse(&["-c"]).is_err());
        assert!(parse(&["raw"]).is_err());
        // `-c` has no other name.
        assert!(parse(&["exec", "ls"]).is_err());
    }

    #[test]
    fn init_takes_agents_both_ways() {
        let Ok(Cli {
            command: Some(Command::Init(init)),
        }) = parse(&["init", "claude", "--agent", "codex", "--agent=claude-code"])
        else {
            panic!("init did not parse");
        };
        assert_eq!(init.named(), ["claude", "codex"]);
        assert!(parse(&["init", "--agent"]).is_err());
        assert_eq!(
            parse(&["init", "notanagent"]).err(),
            Some(ErrorKind::ValueValidation)
        );
    }

    #[test]
    fn config_set_takes_a_key_and_a_value() {
        let Ok(Cli {
            command:
                Some(Command::Config {
                    command: Some(ConfigCommand::Set(set)),
                }),
        }) = parse(&["config", "set", "proxy.port", "", "-y"])
        else {
            panic!("config set did not parse");
        };
        assert_eq!(
            (set.key.as_str(), set.value.as_str(), set.yes),
            ("proxy.port", "", true)
        );
        assert!(parse(&["config", "set", "proxy.port"]).is_err());
        assert!(parse(&["config", "validate"]).is_err());
    }

    #[test]
    fn doctor_flags_go_after_the_check() {
        assert!(parse(&["doctor", "integrations", "--fix", "--json"]).is_ok());
        assert!(parse(&["doctor", "--fix", "integrations"]).is_err());
        assert!(parse(&["doctor", "overhead", "--gate"]).is_ok());
    }
}
