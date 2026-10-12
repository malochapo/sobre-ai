use std::ffi::OsString;

use clap::{CommandFactory, FromArgMatches};

use crate::cli::args::{Cli, Command, HelpTopic, HookAction};
use crate::{core, doctor, hook_handlers, shell, uninstall};

mod analytics;
mod help;
mod lifecycle;
mod network;
mod server;

#[allow(clippy::wildcard_imports)]
use analytics::*;
#[allow(clippy::wildcard_imports)]
use help::*;
#[allow(clippy::wildcard_imports)]
use network::*;
#[allow(clippy::wildcard_imports)]
use server::*;

pub fn run() {
    let mut args: Vec<OsString> = std::env::args_os().collect();

    // On Linux, if the binary was replaced while running, systemd may write
    // the path with " (deleted)" suffix into ExecStart, causing "(deleted)"
    // to appear as an argument. Strip it defensively.
    if args.get(1).is_some_and(|a| a == "(deleted)") {
        args.remove(1);
    }

    // A usage error exits 2 here, `--help` and `--version` exit 0: nothing
    // has run yet.
    let cli = parse(args);

    if !is_server_mode(&cli) {
        restore_sigpipe_default();
    }

    if !matches!(cli.command, None | Some(Command::Mcp)) {
        crate::core::logging::init_logging("warn");
    }

    match cli.command {
        // Bare `sobre` in an interactive terminal: a human almost certainly
        // did not mean to start a silent stdio MCP server (which just hangs
        // waiting for JSON-RPC). Show a short quickstart instead. MCP clients
        // pipe stdin (not a TTY) so they still get the server, and explicit
        // `sobre mcp` always serves regardless of TTY.
        None if std::io::IsTerminal::is_terminal(&std::io::stdin()) => print_quickstart(),
        None | Some(Command::Mcp) => {
            if let Err(e) = run_mcp_server() {
                tracing::error!("sobre: {e}");
                std::process::exit(1);
            }
        }
        Some(Command::Exec(sh)) => handle_exec(&sh.command),
        Some(Command::Raw(sh)) => handle_raw(&sh.command),
        Some(Command::Call(call)) => super::cmd_call(&call),
        Some(Command::Init(init)) => super::cmd_init(&init),
        Some(Command::Doctor(doctor)) => {
            let code = doctor::run_cli(&doctor);
            if code != 0 {
                std::process::exit(code);
            }
        }
        Some(Command::Gain(gain)) => cmd_gain(&gain),
        Some(Command::Index { command }) => super::cmd_index(&command),
        Some(Command::Config { command }) => super::cmd_config(command.as_ref()),
        Some(Command::Embeddings { command }) => {
            super::embeddings_cmd::cmd_embeddings(command.as_ref());
        }
        Some(Command::Proxy { command }) => cmd_proxy(&command),
        Some(Command::Hook { action }) => run_hook(action),
        Some(Command::Uninstall(options)) => uninstall::run(&options),
        Some(Command::Help { topic: None }) => print_help_concise(),
        Some(Command::Help {
            topic: Some(HelpTopic::All),
        }) => print_help(),
    }
}

/// The command line, or the process exits: 2 on a usage error, 0 after
/// `--help` or `--version`. `sobre --help` prints what `sobre help` prints.
fn parse(args: Vec<OsString>) -> Cli {
    let matches = Cli::command()
        .override_help(concise_help_text())
        .get_matches_from(args);
    Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit())
}

fn is_server_mode(cli: &Cli) -> bool {
    matches!(
        cli.command,
        None | Some(Command::Mcp | Command::Proxy { .. })
    )
}

fn run_hook(action: HookAction) {
    // Hooks run inside the agent shell environment, so they can see
    // runtime/session vars (e.g. CODEX_THREAD_ID) that the long-lived MCP
    // server process never receives. Bridge them for sobre_shell.
    core::agent_runtime_env::capture();
    // Rewrite hooks self-bound their work and FAIL OPEN inside the handler,
    // so they must NOT also carry the force-exit watchdog (which would
    // `exit(1)` with no reply and wedge the host). The remaining hooks keep
    // the simple zombie-guard.
    if !matches!(action, HookAction::Cursor | HookAction::Gemini) {
        hook_handlers::arm_watchdog(std::time::Duration::from_secs(5));
    }
    match action {
        HookAction::Compress => hook_handlers::handle_compress(),
        HookAction::Cursor => hook_handlers::handle_cursor(),
        HookAction::Gemini => hook_handlers::handle_gemini(),
        HookAction::ReadDedup => hook_handlers::handle_read_dedup(),
        HookAction::Observe => hook_handlers::handle_observe(),
    }
}

/// The shell command `-c` and `raw` were given: one argument as it is,
/// several joined with spaces. A blank command is a usage error (exit 2).
fn shell_command(name: &str, words: &[String]) -> String {
    if words.iter().all(|w| w.trim().is_empty()) {
        crate::cli::args::usage_error(name, "the command is empty");
    }
    match words {
        [one] => one.clone(),
        _ => shell::join_command(words),
    }
}

fn handle_exec(words: &[String]) -> ! {
    let command = shell_command("-c", words);
    // The `sobre -c` wrapper runs inside the agent shell, which carries
    // runtime/session vars the MCP server never sees. Bridge them so sobre_shell
    // can forward them too.
    core::agent_runtime_env::capture();
    if crate::shell::reentry::should_pass_through() {
        passthrough(&command);
    }
    let code = shell::exec(&command, shell::ExecMode::Compress);
    std::process::exit(code);
}

fn handle_raw(words: &[String]) -> ! {
    let command = shell_command("raw", words);
    let code = shell::exec(&command, shell::ExecMode::Raw);
    std::process::exit(code);
}

/// Restore the default SIGPIPE disposition for short-lived CLI invocations.
///
/// Rust's runtime ignores SIGPIPE process-wide, so `sobre doctor | head`
/// would make `println!` panic with BrokenPipe, and the LineWriter flush in
/// stdout's Drop panic again *during unwinding*, which aborts (SIGABRT, exit
/// 134). CLIs such as cat, grep and rg terminate silently with exit 141
/// instead; SIG_DFL gives exactly that. Children spawned via
/// std::process::Command are unaffected either way (std resets their SIGPIPE
/// disposition).
#[cfg(unix)]
fn restore_sigpipe_default() {
    // SAFETY: signal(2) with SIG_DFL has no preconditions and is called once
    // during single-threaded startup, before any I/O.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn restore_sigpipe_default() {}

fn passthrough(command: &str) -> ! {
    let (shell, flag) = shell::shell_and_flag();
    let mut cmd = std::process::Command::new(&shell);
    cmd.arg(&flag).arg(command);
    shell::reentry::mark_child(&mut cmd);
    shell::platform::apply_utf8_locale(&mut cmd);
    let status = cmd.status().map_or(127, shell::exit_code);
    std::process::exit(status);
}

pub(super) fn run_async<F: std::future::Future>(future: F) -> F::Output {
    // A failed runtime build (e.g. exhausted FDs) must not abort with a panic
    // backtrace the user can't act on — report it plainly and exit.
    match tokio::runtime::Runtime::new() {
        Ok(rt) => rt.block_on(future),
        Err(e) => {
            eprintln!("sobre: failed to create async runtime: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serial_test::serial;

    fn parsed(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("sobre").chain(args.iter().copied()))
            .unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    #[test]
    fn server_modes_keep_ignored_sigpipe() {
        for args in [&["mcp"][..], &["proxy", "status"]] {
            assert!(
                is_server_mode(&parsed(args)),
                "{args:?} must count as server mode"
            );
        }
        // Bare invocation = MCP server spawned by a client.
        assert!(is_server_mode(&parsed(&[])));
    }

    #[test]
    fn cli_modes_restore_default_sigpipe() {
        for args in [
            &["doctor"][..],
            &["-c", "ls"],
            &["index", "status"],
            &["call", "sobre_tree"],
            &["gain"],
            &["help"],
        ] {
            assert!(
                !is_server_mode(&parsed(args)),
                "{args:?} must count as CLI mode (SIGPIPE default)"
            );
        }
    }

    #[test]
    fn quickstart_is_short_and_points_to_setup() {
        let q = quickstart_text();
        assert!(q.contains("sobre init"), "quickstart must point to init");
        assert!(q.contains("sobre help"), "quickstart must point to help");
        // Must stay a *quickstart*, not the full reference — keep it tight.
        assert!(
            q.lines().count() <= 16,
            "quickstart should be short; got {} lines",
            q.lines().count()
        );
        assert!(
            !q.contains("COMMANDS:"),
            "quickstart must not inline the full command reference"
        );
    }

    #[test]
    fn concise_help_is_short_and_points_to_full() {
        let h = concise_help_text();
        assert!(h.contains("sobre init"), "must lead with init");
        assert!(h.contains("sobre help all"), "must point to full reference");
        assert!(h.contains("sobre gain"), "must surface the usage report");
        // Concise means concise — keep it well under the full reference.
        assert!(
            h.lines().count() <= 40,
            "concise help should stay short; got {} lines",
            h.lines().count()
        );
        assert!(
            !h.contains("SHELL OUTPUT PATTERNS"),
            "concise help must not inline the full pattern catalog"
        );
    }

    #[test]
    fn capability_banner_tool_count_matches_registry() {
        let n = crate::server::registry::tool_count();
        let banner = capability_banner();
        assert!(
            banner.contains(&format!("{n} MCP tools")),
            "banner must show the live registry count ({n}); got: {banner}"
        );
    }

    #[test]
    #[serial]
    fn worker_threads_default_clamps_low() {
        assert_eq!(resolve_worker_threads(1), 1);
    }

    #[test]
    #[serial]
    fn worker_threads_default_clamps_high() {
        assert_eq!(resolve_worker_threads(32), 4);
    }

    #[test]
    #[serial]
    fn worker_threads_default_passthrough() {
        assert_eq!(resolve_worker_threads(3), 3);
    }
}
