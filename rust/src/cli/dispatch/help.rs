// Auto-split from the former monolithic dispatch.rs. run() (the command
// match) stays in mod.rs; standalone helpers grouped by concern.

/// Short, friendly orientation shown when a human runs bare `lean-ctx` in a
/// terminal (where the silent stdio MCP server would otherwise just hang). One
/// obvious next step (`init`), not the full command reference.
pub(super) fn quickstart_text() -> String {
    format!(
        "lean-ctx {version} — sobre, local context tools for coding agents

With no arguments, lean-ctx speaks the MCP protocol on stdin/stdout — that is
for your AI agent, not for interactive use, so it is waiting silently. You
probably want one of these:

  lean-ctx init          Connect every detected agent
  lean-ctx init claude   Connect one agent (claude, codex, cursor, ...)
  lean-ctx doctor        Check that everything is wired up correctly
  lean-ctx help          Common commands (or `help all` for everything)
",
        version = env!("CARGO_PKG_VERSION"),
    )
}

pub(super) fn print_quickstart() {
    print!("{}", quickstart_text());
}

/// One-line capability summary under the `--help` title. The MCP-tool count is
/// derived from the registry (single source of truth) so it can never drift.
pub(super) fn capability_banner() -> String {
    format!(
        "{} MCP tools | compressed shell, read and search | local, no telemetry",
        crate::server::registry::tool_count()
    )
}

/// Concise, tiered help shown by default for `lean-ctx help` and `--help`.
/// The exhaustive reference lives behind `lean-ctx help all`.
pub(super) fn print_help_concise() {
    print!("{}", concise_help_text());
}

pub(super) fn concise_help_text() -> String {
    format!(
        "lean-ctx {version} — sobre, local context tools for coding agents

{banner}

GETTING STARTED:
    lean-ctx init                  Connect every detected agent
    lean-ctx init <agent>          Connect one agent (claude, codex, copilot, cursor,
                                   gemini, opencode, windsurf, zed)
    lean-ctx doctor                Check that everything is wired up correctly
    lean-ctx gain                  See how many tokens you have saved

EVERYDAY COMMANDS:
    lean-ctx -c \"command\"          Run a shell command with compressed output
    lean-ctx raw \"command\"         Run a shell command with its exact output
    lean-ctx call <tool> ...       Call one MCP tool from the shell

MANAGE:
    lean-ctx config                Show or edit the config file
    lean-ctx uninstall             Remove everything init wrote

MORE:
    lean-ctx help all              Full command reference (every subcommand)
",
        version = env!("CARGO_PKG_VERSION"),
        banner = capability_banner(),
    )
}

pub(super) fn print_help() {
    println!(
        "lean-ctx {version} — sobre, local context tools for coding agents

{banner}

GETTING STARTED:
    lean-ctx init                  Connect every detected agent
    lean-ctx init <agent>          Connect one agent
    lean-ctx doctor                Check that everything is wired up correctly
    lean-ctx gain                  See how many tokens you have saved
    (everything below is reference — run `lean-ctx help` for the short version)

USAGE:
    lean-ctx                       Start the MCP server (stdio)
    lean-ctx mcp                   Same, explicitly
    lean-ctx -c \"command\"          Run with compressed output (used by agent hooks)
    lean-ctx raw \"command\"         Run with the exact, uncompressed output
    lean-ctx call <tool> --project-root <path> --json '<json>'
                                   Call one MCP tool from the shell

SAVINGS:
    gain                           Tokens saved, real cost (with the proxy), overhead
    gain --by-tool [--limit=N]     Savings per tool or command
    gain --daily                   Saved tokens and real cost per day
    gain --json                    Machine-readable output
    gain --period=<all|today|Nd>   Period to cover (default all)
    gain --model=<m>               Price saved tokens at this model's rate
    gain --reset                   Clear the usage ledger

INDEX:
    index build [--force] [--root <path>]
                                   Build the graph and BM25 indexes (and the dense
                                   index in builds with local embeddings);
                                   --force rebuilds from scratch
    index status [--json]          What is built, and how large

SETUP AND MAINTENANCE:
    init [<agent>] [--check] [--repair] [--json]
    doctor [--fix] [--json]        Diagnostics (and repair), version, connected agents,
                                   debug-log path
    doctor integrations [--json]   Per-agent integration checks
    doctor overhead [--json]       Fixed context cost per session
    doctor lint-context [--json]   Low-signal or duplicate lines in injected context
    config [show]                  Show the config file
    config set <key> <value>       Change one key
    config path | config init      Print the file's path | write it with every key documented
    embeddings status|provision    Local ONNX runtime and model for semantic search
    proxy start [--port=N] [--detach] | stop | restart | status
                                   Local API proxy
    proxy enable | disable         Turn the proxy on or off (autostart and agent wiring)
    proxy cleanup                  Remove proxy URLs left in agent configs while it is off
    proxy token                    Print the proxy's bearer token
    hook <name>                    Internal: run by the hooks init installs
    uninstall [--keep-config] [--keep-binary] [--dry-run]
                                   Remove hooks, MCP configs, the skill, autostart,
                                   data and the binary

SHELL HOOK PATTERNS (95+):
    git, docker, npm/pnpm, cargo, gh, kubectl, python (pip, ruff, poetry, uv),
    linters, tsc and bundlers, ruby, test runners, terraform/make/maven/gradle,
    data tools, security scanners, jj/mise/buf, cloud CLIs, curl/grep/find/ls

OPTIONS:
    --version, -V                  Show version
    --help, -h                     Show this help

TROUBLESHOOTING:
    Something broken?    lean-ctx doctor --fix
    Permanent fix?       lean-ctx uninstall       (removes all hooks)
    Preview init?        lean-ctx init --check
",
        version = env!("CARGO_PKG_VERSION"),
        banner = capability_banner(),
    );
}
