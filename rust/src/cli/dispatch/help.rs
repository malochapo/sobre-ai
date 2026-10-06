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
    lean-ctx init                  Connect every detected agent, plus the shell hook
    lean-ctx init <agent>          Connect one agent (claude, codex, copilot, cursor,
                                   gemini, opencode, windsurf, zed)
    lean-ctx doctor                Check that everything is wired up correctly
    lean-ctx gain                  See how many tokens you have saved

EVERYDAY COMMANDS:
    lean-ctx -c \"command\"          Run a shell command with compressed output
    lean-ctx raw \"command\"         Same, but full uncompressed output
    lean-ctx read <file>           Read a file with compression
    lean-ctx grep <pattern>        Search with compressed output
    lean-ctx tools health          Token budget of the tool surface (unused tools,
                                   stale knowledge)

MANAGE:
    lean-ctx status                Am I connected? (quick check)
    lean-ctx uninstall             Remove everything init wrote

ESCAPE HATCHES:
    lean-ctx raw \"command\"         Run one command with unmodified output
    lean-ctx-off                   Turn the shell aliases off for this shell
    shell_hook = false             (config.toml) Never install the shell hook

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
    lean-ctx init                  Connect every detected agent, plus the shell hook
    lean-ctx init <agent>          Connect one agent
    lean-ctx doctor                Check that everything is wired up correctly
    lean-ctx gain                  See how many tokens you have saved
    (everything below is reference — run `lean-ctx help` for the short version)

USAGE:
    lean-ctx                       Start the MCP server (stdio)
    lean-ctx mcp                   Same, explicitly
    lean-ctx -t \"command\"          Track a command (full output + stats, no compression)
    lean-ctx -c \"command\"          Run with compressed output (used by agent hooks)
    lean-ctx -c --raw \"command\"    Run without compression
    lean-ctx raw \"command\"         Full, exact output
    lean-ctx exec \"command\"        Same as -c

SAVINGS:
    gain                           Savings dashboard (colors, bars, sparklines, USD)
    gain --live                    Refresh every second in place
    gain --deep                    Cost, tasks, agents and heatmap combined
    gain --graph                   30-day savings chart
    gain --daily                   Day-by-day table with USD
    gain --score                   Gain score breakdown
    gain --cost                    Agent cost attribution (estimated)
    gain --measured [--json]       Provider bill measured by the proxy
    gain --tasks | --agents | --by-tool | --heatmap   Breakdowns
    gain --opportunity             Missed savings in your shell history
    gain --wrapped                 Recap card (terminal)
    gain --json | --raw            Machine-readable output
    gain --reset                   Clear all savings data
    gain --model=<m> --period=<week|month|all> --limit=<N> --no-cache-adjust
    output-savings [--json]        Output-token reduction
    ghost [--json]                 Hidden token waste
    cep                            Compression metrics, cache, modes, trends
    benchmark run|report|scorecard Benchmarks on your own project

FILES AND SEARCH:
    read <file> [-m mode]          Read a file with compression
    diff <file1> <file2>           Compressed file diff
    grep <pattern> [path]          Search with compressed output
    glob <pattern> [path]          Find files by glob
    find <pattern> [path]          Find files with compressed output
    ls [path]                      Directory listing with compression
    deps [path]                    Project dependencies
    repomap [--max-tokens N] [--limit N] [--json]  Ranked repository map
    call <tool> [args]             Call one MCP tool from the command line

PROJECT ANALYSIS:
    graph build|status|related|impact|symbol|context   Project graph queries
    graph --heatmap                Per-file heat: tokens x graph connections
    index status|build|build-full|watch [--force]       Codebase index
    smells scan|summary|rules|file [--rule=<r>] [--path=<p>] [--json]
    health [path] [--json] [--gate]  Code health: complexity, naming, token tax
    visualize [--output F] [--open]  Interactive HTML graph report

MEMORY:
    session task|finding|save|load  Live session memory
    sessions list|show|delete|cleanup  Saved session snapshots
    knowledge remember|recall|search|export|import|remove|status  Project facts
    gotchas list|clear|export|stats  Auto-detected error patterns
    learn [--mine <dir>]           Show learned gotchas (read-only)
    snapshot create|list|show|verify|restore|publish|import  Signed context snapshots
    compact [path]                 Compact Claude Code / Cursor transcripts

SETUP AND MAINTENANCE:
    init [--agent <name>] [--global] [--project] [--check] [--repair]
    status [--json]                Setup and MCP status
    doctor [--fix] [--json]        Diagnostics (and repair)
    doctor integrations [--json]   Per-agent integration checks
    config                         Show or edit the config file
    terse [0.0-1.0|off|lite|standard|max|raw]  Set compression_level
    tools health                   Tool-surface token budget
    embeddings status|provision    Local ONNX runtime for semantic search
    pricing status|refresh         Model prices (fetched only on refresh)
    proxy start [--port=4444] [--detach] | status   Local API proxy
    cache list|clear|stats         File read cache
    tee list|clear|show <file>|last  Saved full outputs (see ctx_expand)
    filter list|validate|init      Custom compression filters
    slow-log list|clear            Slow command log
    debug-log list|tail N|clear|path  Opt-in tool-call log (config key debug_log)
    verify-cache [path] [--json]   Prove the session cache
    conformance [--json]           Conformance scorecard (non-zero exit on failure)
    audit                          Audit-trail report with hash-chain check
    introspect                     Which subsystems are active at runtime
    completions <shell>            Shell completion script
    stop | restart                 Stop or restart lean-ctx processes
    dev-install                    Build, install atomically, restart (development)
    codesign-setup                 macOS: stable signing identity (stops TCC prompts)
    uninstall [--keep-config] [--keep-binary] [--dry-run]
                                   Remove hooks, MCP configs, the skill, autostart,
                                   data and the binary

SHELL HOOK PATTERNS (95+):
    git, docker, npm/pnpm, cargo, gh, kubectl, python (pip, ruff, poetry, uv),
    linters, tsc and bundlers, ruby, test runners, terraform/make/maven/gradle,
    data tools, security scanners, jj/mise/buf, cloud CLIs, curl/grep/find/ls

READ MODES:
    auto                           Pick the mode (default)
    full                           Full content (unchanged re-reads ≈ 13 tokens)
    anchored                       Full content with line+hash anchors for ctx_patch
    raw                            Exact bytes, no cache
    map                            Structure and API signatures
    signatures                     Declarations only (tree-sitter)
    task                           Parts relevant to the session task
    reference                      One-line reference stub
    aggressive                     Syntax-stripped content
    entropy                        Entropy-filtered content
    diff                           Changed lines only
    lines:N-M                      Line ranges (e.g. lines:10-50,80)

OPTIONS:
    --version, -V                  Show version
    --help, -h                     Show this help

TROUBLESHOOTING:
    Commands broken?     lean-ctx-off             (fixes the current shell)
    Permanent fix?       lean-ctx uninstall       (removes all hooks)
    Manual fix?          Edit {rc_file}, remove the \"lean-ctx shell hook\" block
    Binary missing?      Aliases fall back to the original commands
    Preview init?        lean-ctx init --check
",
        version = env!("CARGO_PKG_VERSION"),
        banner = capability_banner(),
        rc_file = crate::shell_hook::shell_rc_file(),
    );
}
