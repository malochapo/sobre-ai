use crate::hooks::to_bash_compatible_path;

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

pub fn install_shell_hook(args: &[String]) {
    let global = args.iter().any(|a| a == "--global" || a == "-g");
    let project = args.iter().any(|a| a == "--project");
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let no_hook = args.iter().any(|a| a == "--no-shell-hook")
        || crate::core::config::Config::load().shell_hook_disabled_effective();

    let explicit_mode = args
        .windows(2)
        .find(|w| w[0] == "--mode")
        .and_then(|w| crate::hooks::HookMode::from_str_loose(&w[1]));

    if args.windows(2).any(|w| w[0] == "--mode")
        && !args
            .windows(2)
            .any(|w| w[0] == "--mode" && crate::hooks::HookMode::from_str_loose(&w[1]).is_some())
    {
        let bad = args
            .windows(2)
            .find(|w| w[0] == "--mode")
            .map_or("?", |w| w[1].as_str());
        eprintln!("Unknown hook mode: '{bad}'. Valid: mcp, hybrid, replace");
        std::process::exit(1);
    }

    let agents: Vec<&str> = args
        .windows(2)
        .filter(|w| w[0] == "--agent")
        .map(|w| w[1].as_str())
        .collect();

    if !agents.is_empty() {
        let cwd = std::env::current_dir().unwrap_or_default();
        for agent_name in &agents {
            let mode =
                explicit_mode.unwrap_or_else(|| crate::hooks::recommend_hook_mode(agent_name));
            let result = crate::setup::setup_single_agent(agent_name, global, mode);
            for name in &result.rules.injected {
                qprintln!("  ✓ {name} rules injected");
            }
            for name in &result.rules.updated {
                qprintln!("  ✓ {name} rules updated");
            }
            for name in &result.rules.already {
                qprintln!("  ✓ {name} rules up-to-date");
            }
            if result.mcp_skipped {
                qprintln!("  • MCP registration skipped for {agent_name} (auto_update_mcp=false)");
            }
            for e in &result.errors {
                eprintln!("  ✗ {agent_name}: {e}");
            }
            if agent_name.eq_ignore_ascii_case("hermes") {
                qprintln!("\n  Beyond MCP, lean-ctx can be Hermes' active context engine");
                qprintln!("  (replaces the built-in ContextCompressor). Install the plugin from");
                qprintln!("  integrations/hermes-lean-ctx (scripts/install.sh), then set");
                qprintln!("  context.engine: \"lean-ctx\" in ~/.hermes/config.yaml.");
            }
            if project {
                crate::hooks::install_agent_project_hooks(agent_name, &cwd);
            }
        }
        if !global {
            crate::hooks::install_project_rules_for_agents(&agents);
        }
        qprintln!("\nRun 'lean-ctx gain' after using some commands to see your savings.");
        return;
    }

    let eval_shell = args
        .iter()
        .find(|a| matches!(a.as_str(), "bash" | "zsh" | "fish" | "powershell" | "pwsh"));
    if let Some(shell) = eval_shell
        && !global
    {
        super::shell_init::print_hook_stdout(shell);
        return;
    }

    let shell_name = std::env::var("SHELL").unwrap_or_default();
    let is_zsh = shell_name.contains("zsh");
    let is_fish = shell_name.contains("fish");
    let is_powershell = cfg!(windows) && shell_name.is_empty();

    let binary = crate::core::portable_binary::resolve_portable_binary();

    if dry_run {
        let rc = if is_powershell {
            dirs::home_dir().map_or_else(
                || "PowerShell profile".to_string(),
                |h| {
                    crate::shell::platform::resolve_powershell_profile_path(&h)
                        .to_string_lossy()
                        .into_owned()
                },
            )
        } else if is_fish {
            "~/.config/fish/config.fish".to_string()
        } else if is_zsh {
            "~/.zshrc".to_string()
        } else {
            "~/.bashrc".to_string()
        };
        qprintln!("\nlean-ctx init --dry-run\n");
        qprintln!("  Would modify:  {rc}");
        qprintln!("  Would backup:  {rc}.lean-ctx.bak");
        qprintln!("  Would alias:   git npm pnpm yarn cargo docker docker-compose kubectl");
        qprintln!("                 gh pip pip3 ruff go golangci-lint eslint prettier tsc");
        qprintln!("                 curl wget php composer (24 commands + k)");
        let data_dir = crate::core::data_dir::lean_ctx_data_dir().map_or_else(
            |_| "~/.config/lean-ctx/".to_string(),
            |p| p.to_string_lossy().to_string(),
        );
        qprintln!("  Would create:  {data_dir}");
        qprintln!("  Binary:        {binary}");
        qprintln!("\n  Safety: aliases auto-fallback to original command if lean-ctx is removed.");
        qprintln!("\n  Run without --dry-run to apply.");
        return;
    }

    if no_hook {
        qprintln!("Shell hook disabled (--no-shell-hook or shell_hook_disabled config).");
        qprintln!("MCP tools remain active.");
    } else if is_powershell {
        super::shell_init::init_powershell(&binary);
    } else {
        let bash_binary = to_bash_compatible_path(&binary);
        if is_fish {
            super::shell_init::init_fish(&bash_binary);
        } else {
            super::shell_init::init_posix(is_zsh, &bash_binary);
        }
    }

    if let Ok(lean_dir) = crate::core::data_dir::lean_ctx_data_dir()
        && !lean_dir.exists()
    {
        let _ = std::fs::create_dir_all(&lean_dir);
        qprintln!("Created {}", lean_dir.display());
    }

    let rc = if is_powershell {
        "$PROFILE"
    } else if is_fish {
        "config.fish"
    } else if is_zsh {
        ".zshrc"
    } else {
        ".bashrc"
    };

    qprintln!("\nlean-ctx init complete (24 aliases installed)");
    qprintln!();
    qprintln!("  Disable temporarily:  lean-ctx-off");
    qprintln!("  Re-enable:            lean-ctx-on");
    qprintln!("  Check status:         lean-ctx-status");
    qprintln!("  Full uninstall:       lean-ctx uninstall");
    qprintln!("  Diagnose issues:      lean-ctx doctor");
    qprintln!("  Preview changes:      lean-ctx init --global --dry-run");
    qprintln!();
    if is_powershell {
        qprintln!("  Restart PowerShell or run: . {rc}");
    } else {
        qprintln!("  Restart your shell or run: source ~/{rc}");
    }
    qprintln!();
    qprintln!("For AI tool integration: lean-ctx init --agent <tool> [--mode <mode>]");
    qprintln!("  Supported: aider, amazonq, amp, antigravity, antigravity-cli, augment,");
    qprintln!(
        "    claude, cline, codex, commandcode, continue, copilot, crush, cursor, emacs, gemini,"
    );
    qprintln!("    grok, hermes, jetbrains, kiro, neovim, openclaw, opencode, pi, qoder,");
    qprintln!("    qodercli, qoderwork, qwen, roo, sublime, trae, verdent, vscode,");
    qprintln!("    windsurf, zed");
    qprintln!("  Modes: mcp, hybrid, replace  (auto-detected per agent, override with --mode)");
}

pub fn install_shell_hook_quiet(args: &[String]) {
    let _quiet_guard = crate::core::runtime_flags::scoped_quiet();
    install_shell_hook(args);
}

// ═══════════════════════════════════════════════════════════════════════
// `sobre init` — the one installation command
// ═══════════════════════════════════════════════════════════════════════

/// `sobre init` — connect sobre to this machine.
///
/// Replaces what used to be five commands: `setup` (a twelve-step wizard),
/// `onboard` (the same thing with no questions), `bootstrap` (the same again
/// with a CI exit code), `install` (an alias of two of those) and `wrap`
/// (per-agent). They differed by interactivity and exit code, never by what
/// they installed.
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
    // shell rc and agent configs (the #476 class of bug: `install --help`
    // used to perform the install).
    const KNOWN: &[&str] = &[
        "--agent",
        "--global",
        "-g",
        "--project",
        "--mode",
        "--check",
        "--dry-run",
        "--repair",
        "--json",
        "--no-shell-hook",
        "--no-agent-aliases",
        "--skip-rules",
        "--no-auto-approve",
    ];
    if let Some(unknown) = args
        .iter()
        .find(|a| a.starts_with('-') && !KNOWN.contains(&a.as_str()))
    {
        eprintln!("init: unknown flag '{unknown}'\n");
        print_init_help();
        std::process::exit(2);
    }

    if args.iter().any(|a| a == "--no-agent-aliases") {
        let _ = crate::core::config::setter::set_by_key("skip_agent_aliases", "true");
    }

    // `init <shell>` prints the hook on stdout for `eval "$(sobre init zsh)"`.
    // It must stay first and emit nothing else — the shell evaluates it.
    let is_shell_arg = args
        .iter()
        .any(|a| matches!(a.as_str(), "bash" | "zsh" | "fish" | "powershell" | "pwsh"));

    // `init --agent X`, `init <agent>`, or an explicit shell-hook flag: the
    // primitive handles all three. `init <agent>` is what `wrap <agent>` was.
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
        install_shell_hook(&preview);
        return;
    }

    if is_shell_arg || has_agent_flag {
        install_shell_hook(args);
        return;
    }
    if let Some(agent) = named_agent {
        // Forward flags AND their values: `--mode` and `--agent` take one.
        // Filtering on `starts_with('-')` alone dropped every value, so
        // `init claude --mode mcp` silently fell back to the recommended mode.
        let mut forwarded = vec!["--agent".to_string(), agent.clone()];
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a.starts_with('-') {
                forwarded.push(a.clone());
                if matches!(a.as_str(), "--mode" | "--agent")
                    && let Some(value) = args.get(i + 1)
                    && !value.starts_with('-')
                {
                    forwarded.push(value.clone());
                    i += 1;
                }
            }
            i += 1;
        }
        install_shell_hook(&forwarded);
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
        no_auto_approve: args.iter().any(|a| a == "--no-auto-approve"),
        skip_rules: args.iter().any(|a| a == "--skip-rules"),
        skip_shell_hook: args.iter().any(|a| a == "--no-shell-hook"),
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
    println!("  actually spent. Everything else — MCP tools, the shell hook,");
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
        "Usage: sobre init [<agent>|<shell>] [options]

  sobre init                 Connect everything: shell hook, every detected
                             agent, the daemon. Asks once about the proxy.
  sobre init <agent>         One agent only (claude, cursor, codex, ...)
  sobre init <shell>         Print the shell hook for eval:
                               eval \"$(sobre init zsh)\"

Options:
  --check                    Show what would change, write nothing
  --repair                   No prompt, exits non-zero on failure (for CI)
  --json                     Machine-readable report; implies --repair
  --agent <name>             Same as the positional form, repeatable
  --mode <mcp|hybrid|replace>  Override the per-agent hook mode
  --global                   Write agent rules globally, not per project
  --project                  Also install project-local agent hooks
  --no-shell-hook            Skip the shell rc entirely
  --no-agent-aliases         Do not install claude/codex/gemini aliases
  --skip-rules               Do not write agent rules files
  --no-auto-approve          Do not configure auto-approve

Undo it all: sobre uninstall"
    );
}
