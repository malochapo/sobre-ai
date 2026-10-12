//! The `sobre help` text.

/// Short, friendly orientation shown when a human runs bare `sobre` in a
/// terminal (where the silent stdio MCP server would otherwise just hang). One
/// obvious next step (`init`), not the full command reference.
pub(super) fn quickstart_text() -> String {
    format!(
        "sobre {version} — local context tools for coding agents

Your agent runs sobre through the hooks `sobre init` connects; after
`sobre init --mcp` it also starts sobre as an MCP server over stdin/stdout
(`sobre mcp` does the same by hand). From a terminal you probably want one
of these:

  sobre init          Connect every detected agent
  sobre init claude   Connect one agent (claude, codex, cursor, ...)
  sobre doctor        Check that everything is wired up correctly
  sobre help          Common commands (or `help all` for everything)
",
        version = crate::VERSION,
    )
}

pub(super) fn print_quickstart() {
    print!("{}", quickstart_text());
}

/// One-line capability summary under the `--help` title. The MCP-tool count is
/// derived from the registry (single source of truth) so it can never drift.
pub(super) fn capability_banner() -> String {
    format!(
        "compressed shell output | {} MCP tools, opt-in | local, no telemetry",
        crate::server::registry::tool_count()
    )
}

/// Concise, tiered help shown by `sobre help`, `sobre --help` and `sobre -h`.
/// The exhaustive reference lives behind `sobre help all`.
pub(super) fn print_help_concise() {
    print!("{}", concise_help_text());
}

pub(super) fn concise_help_text() -> String {
    format!(
        "sobre {version} — local context tools for coding agents

{banner}

GETTING STARTED:
    sobre init                  Connect every detected agent
    sobre init <agent>          Connect one agent (claude, codex, copilot, cursor,
                                   gemini, opencode, vscode, windsurf, zed)
    sobre doctor                Check that everything is wired up correctly
    sobre gain                  What sobre compressed, from its usage ledger

EVERYDAY COMMANDS:
    sobre -c \"command\"          Run a shell command with compressed output
    sobre raw \"command\"         Run a shell command with its exact output
    sobre call <tool> ...       Call one MCP tool from the shell

MANAGE:
    sobre config                Show the config; `config set <key> <value>` changes a key
    sobre uninstall             Remove sobre: agent entries, data and binary

MORE:
    sobre help all              Full command reference (every subcommand)
",
        version = crate::VERSION,
        banner = capability_banner(),
    )
}

pub(super) fn print_help() {
    print!("{}", full_help_text());
}

/// `sobre help all`: the command reference rendered from the clap
/// declarations ([`crate::cli::reference`]), then what the shell commands
/// compress.
pub(super) fn full_help_text() -> String {
    format!(
        "sobre {version} — local context tools for coding agents

{banner}

COMMANDS:
{reference}
SHELL OUTPUT PATTERNS (for the hooks, sobre_shell and -c):
    git, docker, npm/pnpm, cargo, gh, kubectl, python (pip, ruff, poetry, uv),
    linters, tsc and bundlers, ruby, test runners, terraform/make/maven/gradle,
    data tools, security scanners, jj/mise/buf, cloud CLIs, curl/grep/find/ls

TROUBLESHOOTING:
    Something broken?    sobre doctor --fix
    Preview init?        sobre init --check
    Remove sobre?        sobre uninstall --help   (what it removes)
",
        version = crate::VERSION,
        banner = capability_banner(),
        reference = crate::cli::reference::reference_text(),
    )
}
