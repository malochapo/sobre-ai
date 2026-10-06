// Auto-split from the former monolithic writers.rs. Grouped by operation
// (install) + shared helpers; behavior is unchanged.

use super::types::{ConfigType, EditorTarget};

mod install;
mod shared;

pub use shared::auto_approve_tools;
// The router below dispatches to every install writer; a glob keeps the
// dispatch table readable and lets the test module reach them via `super::*`.
#[allow(clippy::wildcard_imports)]
use install::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAction {
    Created,
    Updated,
    Already,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct WriteOptions {
    pub overwrite_invalid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteResult {
    pub action: WriteAction,
    pub note: Option<String>,
}

pub fn write_config(target: &EditorTarget, binary: &str) -> Result<WriteResult, String> {
    write_config_with_options(target, binary, WriteOptions::default())
}

pub fn write_config_with_options(
    target: &EditorTarget,
    binary: &str,
    opts: WriteOptions,
) -> Result<WriteResult, String> {
    if let Some(parent) = target.config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    match target.config_type {
        ConfigType::McpJson => write_mcp_json(target, binary, opts),
        ConfigType::Zed => write_zed_config(target, binary, opts),
        ConfigType::Codex => write_codex_config(target, binary),
        ConfigType::VsCodeMcp => write_vscode_mcp(target, binary, opts),
        ConfigType::CopilotCli => write_copilot_cli(target, binary, opts),
        ConfigType::OpenCode => write_opencode_config(target, binary, opts),
        ConfigType::GeminiSettings => write_gemini_settings(target, binary, opts),
    }
}

#[cfg(test)]
mod tests;
