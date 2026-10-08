use crate::{core, doctor, hook_handlers, shell, uninstall};

mod analytics;
mod help;
mod lifecycle;
mod network;
mod server;
pub(crate) mod suggest;

#[allow(clippy::wildcard_imports)]
use analytics::*;
#[allow(clippy::wildcard_imports)]
use help::*;
#[allow(clippy::wildcard_imports)]
use network::*;
#[allow(clippy::wildcard_imports)]
use server::*;

pub fn run() {
    let mut args: Vec<String> = std::env::args().collect();

    // On Linux, if the binary was replaced while running, systemd may write
    // the path with " (deleted)" suffix into ExecStart, causing "(deleted)"
    // to appear as an argument. Strip it defensively.
    if args.get(1).is_some_and(|a| a == "(deleted)") {
        args.remove(1);
    }

    if !is_server_mode(&args) {
        restore_sigpipe_default();
    }

    let enters_mcp = args.len() == 1 || args.get(1).is_some_and(|a| a == "mcp");
    if !enters_mcp {
        crate::core::logging::init_logging();
    }

    if args.len() > 1 {
        let rest = args[2..].to_vec();

        match args[1].as_str() {
            // `-c` is the wire form the Cursor/Gemini pre-run hooks emit
            // (`<binary> -c '<command>'`, hook_handlers/rewrite.rs). Removing it
            // makes every wrapped command fail. shell/exec.rs also matches on
            // it when collapsing nesting.
            "-c" => handle_exec(&args),
            "gain" => {
                cmd_gain(&rest);
                return;
            }
            "embeddings" => {
                crate::cli::embeddings_cmd::cmd_embeddings(&rest);
                return;
            }
            "index" => {
                crate::cli::cmd_index(&rest);
                return;
            }
            "proxy" => {
                cmd_proxy(&rest);
                return;
            }
            "init" => {
                super::cmd_init(&rest);
                return;
            }
            "call" => {
                super::cmd_call(&rest);
                return;
            }
            "config" => {
                super::cmd_config(&rest);
                return;
            }
            "doctor" => {
                let code = doctor::run_cli(&rest);
                if code != 0 {
                    std::process::exit(code);
                }
                return;
            }
            "hook" => {
                // Hooks run inside the agent shell environment, so they can see
                // runtime/session vars (e.g. CODEX_THREAD_ID) that the long-lived
                // MCP server process never receives. Bridge them for sobre_shell (#370).
                core::agent_runtime_env::capture();
                let action = rest.first().map_or("help", std::string::String::as_str);
                // Rewrite hooks self-bound their work and FAIL OPEN inside the
                // handler (#1035), so they must NOT also carry the force-exit
                // watchdog (which would `exit(1)` with no reply and wedge the
                // host). The remaining hooks keep the simple zombie-guard.
                if !matches!(action, "cursor" | "gemini") {
                    hook_handlers::arm_watchdog(std::time::Duration::from_secs(5));
                }
                match action {
                    "compress" => hook_handlers::handle_compress(),
                    "cursor" => hook_handlers::handle_cursor(),
                    "gemini" => hook_handlers::handle_gemini(),
                    "read-dedup" => hook_handlers::handle_read_dedup(),
                    "observe" => hook_handlers::handle_observe(),
                    _ => {
                        eprintln!("Usage: sobre hook <compress|cursor|gemini|read-dedup|observe>");
                        eprintln!("  Internal commands used by agent hooks");
                        std::process::exit(2);
                    }
                }
                return;
            }
            "uninstall" => {
                // Safety: `--help`/`-h` must NEVER fall through to a real
                // uninstall (issue #476). Short-circuit before any removal.
                if rest.iter().any(|a| a == "--help" || a == "-h") {
                    uninstall::print_help();
                    return;
                }
                let dry_run = rest.iter().any(|a| a == "--dry-run");
                let keep_config = rest.iter().any(|a| a == "--keep-config");
                let keep_binary = rest.iter().any(|a| a == "--keep-binary");
                uninstall::run(dry_run, keep_config, keep_binary);
                return;
            }
            // `raw` runs the command with its output passed through unchanged.
            "raw" => handle_raw(&args, &rest),
            "--version" | "-V" => {
                println!("sobre {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "help" | "--help" | "-h" => {
                if rest
                    .iter()
                    .any(|a| matches!(a.as_str(), "all" | "full" | "--all" | "-a"))
                {
                    print_help();
                } else {
                    print_help_concise();
                }
                return;
            }
            "mcp" => {}
            _ => {
                let unknown = &args[1];
                eprintln!("sobre: unknown command '{unknown}'");
                if let Some(suggestion) = suggest::closest_command(unknown) {
                    eprintln!("       did you mean '{suggestion}'?");
                }
                eprintln!("       run 'sobre help' for the full command list");
                std::process::exit(2);
            }
        }
    }

    // Bare `sobre` in an interactive terminal: a human almost certainly did
    // not mean to start a silent stdio MCP server (which just hangs waiting for
    // JSON-RPC). Show a short quickstart instead. MCP clients pipe stdin (not a
    // TTY) so they still get the server, and explicit `sobre mcp` always
    // serves regardless of TTY.
    if args.len() == 1 && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        print_quickstart();
        return;
    }

    if let Err(e) = run_mcp_server() {
        tracing::error!("sobre: {e}");
        std::process::exit(1);
    }
}

fn is_server_mode(args: &[String]) -> bool {
    args.len() == 1
        || args
            .get(1)
            .is_some_and(|a| matches!(a.as_str(), "mcp" | "proxy"))
}

fn handle_exec(args: &[String]) -> ! {
    let cmd_args = &args[2..];
    if cmd_args.iter().all(|a| a.trim().is_empty()) {
        eprintln!("Usage: sobre -c \"command\"");
        std::process::exit(2);
    }
    let command = if cmd_args.len() == 1 {
        cmd_args[0].clone()
    } else {
        shell::join_command(cmd_args)
    };
    // The `sobre -c` wrapper runs inside the agent shell, which carries
    // runtime/session vars the MCP server never sees. Bridge them so sobre_shell
    // can forward them too (#370).
    core::agent_runtime_env::capture();
    if crate::shell::reentry::should_pass_through() {
        passthrough(&command);
    }
    core::runtime_flags::enable_compress();
    let code = shell::exec(&command);
    core::tool_lifecycle::flush_all();
    std::process::exit(code);
}

fn handle_raw(args: &[String], rest: &[String]) -> ! {
    if rest.is_empty() {
        eprintln!("Usage: sobre raw \"command\"");
        eprintln!("Runs the command with output passed through unchanged (no compression).");
        std::process::exit(2);
    }
    let command = if rest.len() == 1 {
        rest[0].clone()
    } else {
        shell::join_command(&args[2..])
    };
    core::runtime_flags::enable_raw();
    let code = shell::exec(&command);
    std::process::exit(code);
}

/// Restore the default SIGPIPE disposition for short-lived CLI invocations.
///
/// Rust's runtime ignores SIGPIPE process-wide, so `sobre doctor | head`
/// made `println!` panic with BrokenPipe; the LineWriter flush in stdout's
/// Drop then panicked again *during unwinding*, which aborts — the SIGABRT
/// (exit 134) of upstream #378 / GL#436. Real CLIs (cat, grep, rg) terminate
/// silently with exit 141 instead; SIG_DFL gives us exactly that. Children
/// spawned via std::process::Command are unaffected either way (std resets
/// their SIGPIPE disposition since Rust 1.65).
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
    let status = cmd.status().map_or(127, |s| s.code().unwrap_or(1));
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
    use serial_test::serial;

    fn args_of(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn server_modes_keep_ignored_sigpipe() {
        for mode in ["mcp", "proxy"] {
            assert!(
                is_server_mode(&args_of(&["sobre", mode])),
                "{mode} must count as server mode"
            );
        }
        // Bare invocation = MCP server spawned by a client.
        assert!(is_server_mode(&args_of(&["sobre"])));
    }

    #[test]
    fn cli_modes_restore_default_sigpipe() {
        for mode in ["doctor", "-c", "index", "call", "gain", "help"] {
            assert!(
                !is_server_mode(&args_of(&["sobre", mode])),
                "{mode} must count as CLI mode (SIGPIPE default)"
            );
        }
    }

    #[test]
    fn quickstart_is_short_and_points_to_setup() {
        let q = quickstart_text();
        assert!(q.contains("sobre init"), "quickstart must point to wrap");
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
        assert!(h.contains("sobre gain"), "must surface the savings report");
        // Concise means concise — keep it well under the full reference.
        assert!(
            h.lines().count() <= 40,
            "concise help should stay short; got {} lines",
            h.lines().count()
        );
        assert!(
            !h.contains("SHELL HOOK PATTERNS"),
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
