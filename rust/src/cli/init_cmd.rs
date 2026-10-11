//! `sobre init`: connect sobre to this machine's agents.

use crate::cli::args::InitArgs;

/// `sobre init`: connect sobre to this machine's agents.
///
/// Every agent gets the hooks. The MCP server and the skill add about 400
/// tokens to every request, so they are connected with `--mcp`, on a yes to
/// the question asked in a terminal while an agent lacks them, and refreshed
/// where an agent already has sobre's MCP entry. Without a named agent and in
/// a terminal, it also asks about the proxy (it sees every request, so turning
/// it on is a real decision) until the answer is on record, and, in a build
/// with embeddings, about downloading the model while it is missing.
/// Everything else takes its default and is changed afterwards through
/// `sobre config`.
pub(crate) fn cmd_init(args: &InitArgs) {
    // An unknown agent (a typo) exits 2 when the line is parsed: running on
    // would connect every agent.
    let named = args.named();
    let (json, check, repair) = (args.json, args.check, args.repair);
    let _quiet = json.then(crate::core::runtime_flags::scoped_quiet);

    // `--repair` and `--json` are the machine entry points: no prompt, and a
    // non-zero exit when anything failed. `--check` writes nothing.
    let ask = !check && !repair && !json;
    let mcp = if args.mcp || (ask && ask_about_mcp(&named)) {
        crate::setup::Mcp::Connect
    } else {
        crate::setup::Mcp::KeepExisting
    };
    if ask && named.is_empty() {
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
    let report = crate::setup::run_init(&named, mcp, check);
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

/// The MCP question: whether to connect the MCP server and the skill besides
/// the hooks. Asked in a terminal while an agent about to be connected lacks
/// sobre's MCP entry; the answer is not recorded, and an agent that has the
/// entry keeps it.
fn ask_about_mcp(named: &[&str]) -> bool {
    use std::io::Write;

    if crate::shell::is_non_interactive() || !crate::setup::any_lacks_mcp(named) {
        return false;
    }

    crate::outln!();
    crate::outln!("  sobre's MCP tools (sobre_read, sobre_search, …) and its skill add");
    crate::outln!("  about 400 tokens to every request. The hooks work without them.");
    crate::outln!();
    print!("  Connect them too? [y/N] ");
    let _ = std::io::stdout().flush();

    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    let yes = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");
    if !yes {
        crate::outln!("  Skipped. Add them later with `sobre init --mcp`.");
    }
    yes
}

/// The proxy question. Skipped once the choice is on record, and without a
/// terminal, so scripted runs never block on stdin.
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
    let managed = crate::proxy_autostart::is_supported();
    let started = if managed {
        crate::proxy_autostart::install(port, false)
    } else {
        crate::proxy_setup::is_proxy_reachable(port)
    };
    if !started {
        if managed {
            crate::outln!(
                "  The proxy could not be started, so it stays off.\n  `sobre proxy start --port {port}` shows why; `sobre proxy enable` turns it on later."
            );
        } else {
            crate::outln!(
                "  No service manager here to run the proxy, so it stays off. Start it by\n  hand with `sobre proxy start -d`, then run `sobre proxy enable`."
            );
        }
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
