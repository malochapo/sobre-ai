pub(crate) fn quiet_enabled() -> bool {
    crate::core::runtime_flags::quiet_enabled()
}

macro_rules! qprintln {
    ($($t:tt)*) => {
        if !quiet_enabled() {
            println!($($t)*);
        }
    };
}

/// `init --agent <name>` (repeatable): connect the named agents. With
/// `--dry-run`, describe what `init` would write instead.
fn install_agents(args: &[String]) {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let agents: Vec<&str> = args
        .windows(2)
        .filter(|w| w[0] == "--agent")
        .map(|w| w[1].as_str())
        .collect();

    if dry_run {
        let binary = crate::core::portable_binary::resolve_portable_binary();
        let data_dir = crate::core::data_dir::lean_ctx_data_dir().map_or_else(
            |_| "~/.local/share/lean-ctx/".to_string(),
            |p| p.to_string_lossy().to_string(),
        );
        qprintln!("\nlean-ctx init --check\n");
        if agents.is_empty() {
            qprintln!("  Would connect: every detected agent (MCP entry, hooks, skill)");
        } else {
            qprintln!("  Would connect: {}", agents.join(", "));
        }
        qprintln!("  Would create:  {data_dir}");
        qprintln!("  Binary:        {binary}");
        qprintln!("  Shell files:   untouched (sobre never edits your shell rc)");
        qprintln!("\n  Run without --check to apply.");
        return;
    }

    for agent_name in &agents {
        let result = crate::setup::setup_single_agent(agent_name);
        for e in &result.errors {
            eprintln!("  ✗ {agent_name}: {e}");
        }
    }
    qprintln!("\nRun 'lean-ctx gain' after using some commands to see your savings.");
}

// ═══════════════════════════════════════════════════════════════════════
// `sobre init` — the one installation command
// ═══════════════════════════════════════════════════════════════════════

/// `sobre init` — connect sobre to this machine.
///
/// One question is still asked, and only one: the proxy sees every request,
/// so turning it on is a real decision. Everything else takes a safe default
/// and is changed afterwards through `sobre config`.
pub fn cmd_init(args: &[String]) {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_init_help();
        return;
    }

    // An unknown flag must never fall through to a run that rewrites the
    // shell rc and agent configs (#476).
    const KNOWN: &[&str] = &["--agent", "--check", "--dry-run", "--repair", "--json"];
    if let Some(unknown) = args
        .iter()
        .find(|a| a.starts_with('-') && !KNOWN.contains(&a.as_str()))
    {
        eprintln!("init: unknown flag '{unknown}'\n");
        print_init_help();
        std::process::exit(2);
    }

    // `init --agent X` or `init <agent>`.
    // The registry is the list of agents sobre knows how to configure, so it
    // also answers "is this positional an agent name?".
    let known_agents: Vec<String> = dirs::home_dir()
        .map(|h| crate::core::editor_registry::build_targets(&h))
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.agent_key)
        .collect();
    let named_agent = args
        .iter()
        .find(|a| !a.starts_with('-') && known_agents.iter().any(|k| k == *a))
        .cloned();
    let has_agent_flag = args.windows(2).any(|w| w[0] == "--agent");
    // Any other positional (a shell name, a typo) is an error: falling through
    // would run the whole install.
    let agent_values: Vec<&String> = args
        .windows(2)
        .filter(|w| w[0] == "--agent")
        .map(|w| &w[1])
        .collect();
    if let Some(stray) = args.iter().find(|a| {
        !a.starts_with('-') && !known_agents.iter().any(|k| k == *a) && !agent_values.contains(a)
    }) {
        eprintln!("init: unknown agent '{stray}'\n");
        print_init_help();
        std::process::exit(2);
    }

    let json = args.iter().any(|a| a == "--json");
    let check = args.iter().any(|a| a == "--check" || a == "--dry-run");
    let repair = args.iter().any(|a| a == "--repair");

    // `--check` writes nothing, so it must be answered before any branch that
    // installs — including the per-agent one. Tested after it, `init claude
    // --check` performed a real install while the help promised otherwise.
    if check {
        let mut preview = vec!["--dry-run".to_string()];
        if let Some(ref agent) = named_agent {
            preview.push("--agent".to_string());
            preview.push(agent.clone());
        }
        install_agents(&preview);
        return;
    }

    if has_agent_flag {
        install_agents(args);
        return;
    }
    if let Some(agent) = named_agent {
        // Forward flags AND their values: `--agent` takes one; filtering on
        // `starts_with('-')` alone would drop it.
        let mut forwarded = vec!["--agent".to_string(), agent.clone()];
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a.starts_with('-') {
                forwarded.push(a.clone());
                if a == "--agent"
                    && let Some(value) = args.get(i + 1)
                    && !value.starts_with('-')
                {
                    forwarded.push(value.clone());
                    i += 1;
                }
            }
            i += 1;
        }
        install_agents(&forwarded);
        return;
    }

    // `--repair` is the machine entry point: no prompt, and a non-zero exit
    // when anything failed. CI gates on that exit code.
    if !repair && !json {
        ask_about_proxy();
        ask_about_embeddings();
    }

    let opts = crate::setup::SetupOptions {
        non_interactive: true,
        yes: true,
        fix: true,
        json,
        ..Default::default()
    };
    match crate::setup::run_setup_with_options(opts) {
        Ok(report) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
                );
            }
            if !report.success {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("init: {e}");
            std::process::exit(1);
        }
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

    println!();
    println!("  The local proxy routes LLM requests through sobre, which buys");
    println!("  extra tool-result compression and an exact record of what you");
    println!("  actually spent. Everything else — MCP tools, the agent hooks,");
    println!("  gain tracking — works without it.");
    println!();
    print!("  Enable the local proxy? [y/N] ");
    let _ = std::io::stdout().flush();

    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    let yes = matches!(input.trim().to_lowercase().as_str(), "y" | "yes");

    if let Err(e) = crate::core::config::Config::update_global(|c| c.proxy.enabled = Some(yes)) {
        tracing::warn!("could not persist the proxy choice: {e}");
        return;
    }
    if yes {
        let port = crate::proxy_setup::default_port();
        crate::proxy_autostart::install(port, false);
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Some(home) = dirs::home_dir() {
            crate::proxy_setup::install_proxy_env(&home, port, false);
        }
        println!("  Proxy enabled.");
    } else {
        println!("  Skipped. Turn it on later with `sobre proxy enable`.");
    }
}

/// Semantic search needs a local model. It is downloaded only with consent:
/// here, or later with `lean-ctx embeddings provision`. Asked only by builds
/// that can use it, and only while the model is missing.
fn ask_about_embeddings() {
    #[cfg(feature = "embeddings")]
    {
        use std::io::Write;

        if crate::core::embeddings::EmbeddingEngine::is_available()
            || crate::shell::is_non_interactive()
        {
            return;
        }

        println!();
        println!("  Semantic search runs a small embedding model on this machine.");
        println!("  It needs the ONNX Runtime and the model (about 120 MB, one-time).");
        println!();
        print!("  Download them now? [y/N] ");
        let _ = std::io::stdout().flush();

        let mut input = String::new();
        let _ = std::io::stdin().read_line(&mut input);
        if matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            if let Err(e) = crate::cli::embeddings_cmd::provision(false) {
                eprintln!("  Download failed: {e}");
            }
        } else {
            println!("  Skipped. Download later with `lean-ctx embeddings provision`.");
        }
    }
}

fn print_init_help() {
    println!(
        "Usage: sobre init [<agent>] [options]

  sobre init                 Connect every detected agent (MCP entry, hooks,
                             skill). Asks once about the proxy. Never edits
                             your shell rc files.
  sobre init <agent>         One agent only (claude, cursor, codex, ...)

Options:
  --check                    Show what would change, write nothing
  --repair                   No prompt, exits non-zero on failure (for CI)
  --json                     Machine-readable report; implies --repair
  --agent <name>             Same as the positional form, repeatable

Undo it all: sobre uninstall"
    );
}
