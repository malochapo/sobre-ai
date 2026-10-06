mod claude;
mod codex;
mod copilot;
mod cursor;
mod gemini;
mod opencode;
mod shared;
mod windsurf;

pub(crate) use claude::CLAUDE_MD_BLOCK_START;
pub(super) use claude::{
    install_claude_hook_config, install_claude_hook_scripts, install_claude_hook_with_mode,
    install_claude_permissions_deny_replace, install_claude_project_hooks,
};
pub use codex::install_codex_hook;
pub(super) use copilot::install_copilot_hook;
pub use cursor::install_cursor_hook;
pub(super) use cursor::{
    install_cursor_deny_hook, install_cursor_hook_config, install_cursor_hook_scripts,
    install_cursor_hook_with_mode,
};
pub(super) use gemini::{
    install_gemini_deny_hook, install_gemini_hook, install_gemini_hook_config,
    install_gemini_hook_scripts,
};
pub(super) use opencode::install_opencode_hook;
pub(super) use windsurf::{
    install_windsurf_hooks, install_windsurf_hooks_replace, install_windsurf_rules,
};
