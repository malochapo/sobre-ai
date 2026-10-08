mod claude;
mod codex;
mod copilot;
mod cursor;
mod gemini;
mod opencode;
mod windsurf;

pub(super) use claude::install_claude_hook_config;
pub use codex::install_codex_hook;
pub(super) use copilot::install_copilot_hooks;
pub(super) use cursor::install_cursor_hook_config;
pub(super) use gemini::install_gemini_hook_config;
pub(super) use opencode::install_opencode_plugin;
pub(super) use windsurf::install_windsurf_hooks;
