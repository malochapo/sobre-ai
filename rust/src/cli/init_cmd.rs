// ═══════════════════════════════════════════════════════════════════════
// `sobre init` — the one installation command
// ═══════════════════════════════════════════════════════════════════════

/// `sobre init` — connect sobre to this machine's agents.
///
/// One question is asked, and only once: the proxy sees every request, so
/// turning it on is a real decision. Everything else takes a safe default and
/// is changed afterwards through `sobre config`.
pub fn cmd_init(args: &[String]) {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_init_help();
        return;
    }

    // An unknown flag must never fall through to a run that rewrites agent
    // configs.
    const KNOWN: &[&str] = &["--agent", "--check", "--dry-run", "--repair", "--json"];
    if let Some(unknown) = args
        .iter()
        .find(|a| a.starts_with('-') && !KNOWN.contains(&a.as_str()))
    {
        crate::errln!("init: unknown flag '{unknown}'\n");
        print_init_help();
        std::process::exit(2);
    }

    // `init <agent>…` and `init --agent <agent>`: any other positional (a
    // typo) is an error, since falling through would connect every agent.
    let mut named: Vec<&'static str> = Vec::new();
    for arg in args.iter().filter(|a| !a.starts_with('-')) {
        match crate::setup::agent_key(arg) {
            Some(key) if !named.contains(&key) => named.push(key),
            Some(_) => {}
            None => {
                crate::errln!("init: unknown agent '{arg}'\n");
                print_init_help();
                std::process::exit(2);
            }
        }
    }

    let json = args.iter().any(|a| a == "--json");
    let check = args.iter().any(|a| a == "--check" || a == "--dry-run");
    let repair = args.iter().any(|a| a == "--repair");
    let _quiet = json.then(crate::core::runtime_flags::scoped_quiet);

    // `--repair` and `--json` are the machine entry points: no prompt, and a
    // non-zero exit when anything failed. `--check` writes nothing.
    if !check && !repair && !json && named.is_empty() {
        ask_about_proxy();
        ask_about_embeddings();
    }

    if !json {
        crate::outln!(
            "\nsobre init{}\n",
            if check {
                " --check (nothing is written)"
            } else {
                ""
            }
        );
    }
    let report = crate::setup::run_init(&named, check);
    if json {
        crate::outln!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
        );
    }
    if !report.success {
        std::process::exit(1);
    }
}

/// The one prompt. Skipped once the choice is on record, and skipped entirely
/// under `--repair`/`--json` so scripted runs never block on stdin.
fn ask_about_proxy() {
    use std::io::Write;

    if crate::core::config::Config::load().proxy.enabled.is_some() {
        return;
    }
    if crate::shell::is_non_interactive() {
        return;
    }

    crate::outln!();
    crate::outln!("  The local proxy routes LLM requests through sobre, which buys");
    crate::outln!("  extra tool-result compression and a record of the tokens each");
    crate::outln!("  call was billed. Everything else — MCP tools, the agent hooks,");
    crate::outln!("  gain tracking — works without it.");
    crate::outln!();
    print!("  Enable the local proxy? [y/N] ");
    let _ = std::io::stdout().flush();

    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    let yes = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");

    if !yes {
        if let Err(e) =
            crate::core::config::Config::update_global(|c| c.proxy.enabled = Some(false))
        {
            tracing::warn!("could not persist the proxy choice: {e}");
        }
        crate::outln!("  Skipped. Turn it on later with `sobre proxy enable`.");
        return;
    }
    // Same path as `sobre proxy enable`: nothing is recorded unless the
    // proxy actually runs.
    let port = crate::proxy_setup::default_port();
    let started = if crate::proxy_autostart::is_supported() {
        crate::proxy_autostart::install(port, false)
    } else {
        crate::proxy_setup::is_proxy_reachable(port)
    };
    if !started {
        crate::outln!(
            "  The proxy could not be started, so it stays off. Start it by hand with\n  `sobre proxy start -d`, then run `sobre proxy enable`."
        );
        return;
    }
    if let Err(e) = crate::core::config::Config::update_global(|c| c.proxy.enabled = Some(true)) {
        tracing::warn!("could not persist the proxy choice: {e}");
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(500));
    if let Some(home) = dirs::home_dir() {
        crate::proxy_setup::install_proxy_env(&home, port, false);
    }
    crate::outln!("  Proxy enabled.");
}

/// Semantic search needs a local model. It is downloaded only with consent:
/// here, or later with `sobre embeddings provision`. Asked only by builds
/// that can use it, and only while the model is missing.
fn ask_about_embeddings() {
    #[cfg(feature = "dense")]
    {
        use std::io::Write;

        if crate::core::embeddings::EmbeddingEngine::is_available()
            || crate::shell::is_non_interactive()
        {
            return;
        }

        crate::outln!();
        crate::outln!("  Semantic search runs a small embedding model on this machine.");
        crate::outln!("  It needs the ONNX Runtime and the model (about 120 MB, one-time).");
        crate::outln!();
        print!("  Download them now? [y/N] ");
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        if matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            if let Err(e) = crate::cli::embeddings_cmd::provision(false) {
                crate::errln!("  Download failed: {e}");
            }
        } else {
            crate::outln!("  Skipped. Download later with `sobre embeddings provision`.");
        }
    }
}

fn print_init_help() {
    crate::outln!(
        "Usage: sobre init [<agent>...] [options]

  sobre init                 Connect every installed agent: its MCP entry,
                             hooks and skill. Asks once about the proxy and,
                             in a build with embeddings, about downloading
                             the model. Never edits your shell rc files.
  sobre init <agent>...      Only the named agents, installed or not, with
                             no question: claude, codex, copilot, cursor,
                             gemini, opencode, vscode, windsurf, zed

Options:
  --check, --dry-run         List the files init would create or change,
                             and write nothing
  --repair                   Ask no question (for scripts and CI)
  --json                     Machine-readable report; asks no question
  --agent <name>             Same as naming the agent, repeatable

init exits non-zero when an agent could not be connected.

Edits keep your comments and key order; a file sobre edits for the first
time is copied to <file>.sobre.bak (an empty one for a file sobre creates).
Undo it all: sobre uninstall"
    );
}
