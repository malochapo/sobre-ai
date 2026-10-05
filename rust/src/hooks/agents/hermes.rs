use super::super::{HookMode, install_project_rules, resolve_binary_path};

const HERMES_BLOCK_START: &str = "<!-- lean-ctx -->";
const HERMES_BLOCK_END: &str = "<!-- /lean-ctx -->";

fn hermes_marked_block(mode: HookMode) -> String {
    let body = if mode == HookMode::Replace {
        hermes_replace_rules_content()
    } else {
        hermes_rules_content()
    };
    format!("{HERMES_BLOCK_START}\n{body}{HERMES_BLOCK_END}")
}

/// Produce Hermes rules content: canonical shared rules followed by
/// Hermes-specific extras (available tools, multi-agent notes).
/// The canonical section uses markers so the injection layer can update it;
/// Hermes extras sit after END_MARK and are preserved as user content.
pub(super) fn hermes_rules_content() -> String {
    let base = crate::core::rules_canonical::render(
        true,
        crate::core::rules_canonical::Wrapper::Shared,
        crate::core::config::CompressionLevel::Off,
    );
    format!("{base}\n")
}

pub(crate) fn install_hermes_hook_with_mode(global: bool, mode: HookMode) {
    let Some(home) = crate::core::home::resolve_home_dir() else {
        tracing::error!("Cannot resolve home directory");
        return;
    };

    let binary = resolve_binary_path();
    let config_path = home.join(".hermes/config.yaml");
    let target = crate::core::editor_registry::EditorTarget {
        name: "Hermes Agent",
        agent_key: "hermes".to_string(),
        config_path: config_path.clone(),
        detect_path: home.join(".hermes"),
        config_type: crate::core::editor_registry::ConfigType::HermesYaml,
    };

    match crate::core::editor_registry::write_config_with_options(
        &target,
        &binary,
        crate::core::editor_registry::WriteOptions {
            overwrite_invalid: true,
        },
    ) {
        Ok(res) => match res.action {
            crate::core::editor_registry::WriteAction::Created => {
                eprintln!(
                    "  \x1b[32m✓\x1b[0m Hermes Agent MCP configured at ~/.hermes/config.yaml"
                );
            }
            crate::core::editor_registry::WriteAction::Updated => {
                eprintln!("  \x1b[32m✓\x1b[0m Hermes Agent MCP updated at ~/.hermes/config.yaml");
            }
            crate::core::editor_registry::WriteAction::Already => {
                eprintln!("  Hermes Agent MCP already configured at ~/.hermes/config.yaml");
            }
        },
        Err(e) => {
            tracing::error!("Failed to configure Hermes Agent MCP: {e}");
        }
    }

    install_hermes_rules(&home, mode);
    if !global {
        install_project_hermes_rules(mode);
        install_project_rules();
    }
}

fn install_hermes_rules(home: &std::path::Path, mode: HookMode) {
    let rules_path = home.join(".hermes/HERMES.md");
    if let Some(parent) = rules_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let block = hermes_marked_block(mode);
    crate::marked_block::upsert(
        &rules_path,
        HERMES_BLOCK_START,
        HERMES_BLOCK_END,
        &block,
        false,
        "lean-ctx rules in ~/.hermes/HERMES.md",
    );
}

fn hermes_replace_rules_content() -> String {
    let base = crate::core::rules_canonical::render(
        true,
        crate::core::rules_canonical::Wrapper::Shared,
        crate::core::config::CompressionLevel::Off,
    );
    format!(
        "{base}\n\
         ## Replace Mode — native tools denied\n\
         Native Read/Grep/Glob/Bash are denied. Use ONLY ctx_* MCP tools.\n"
    )
}

fn install_project_hermes_rules(mode: HookMode) {
    let Ok(cwd) = std::env::current_dir() else {
        return;
    };
    let rules_path = cwd.join(".hermes.md");
    let block = hermes_marked_block(mode);
    crate::marked_block::upsert(
        &rules_path,
        HERMES_BLOCK_START,
        HERMES_BLOCK_END,
        &block,
        false,
        "lean-ctx rules in .hermes.md",
    );
}

#[cfg(test)]
mod tool_name_tests {
    use super::*;

    /// Replace mode denies the native tools, so a wrong tool list there leaves
    /// the agent with nothing that works. Both blocks must name only
    /// registered tools.
    #[test]
    fn hermes_blocks_name_only_registered_tools() {
        for (label, text) in [
            ("shared", hermes_rules_content()),
            ("replace", hermes_replace_rules_content()),
        ] {
            let dead = crate::server::registry::unregistered_tool_mentions(&text);
            assert!(dead.is_empty(), "hermes {label} block names {dead:?}");
        }
    }
}
