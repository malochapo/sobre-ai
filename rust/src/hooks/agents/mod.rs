mod claude;
mod codex;
mod copilot;
mod cursor;
mod gemini;
mod opencode;
mod shared;
mod windsurf;

pub(super) use claude::{
    install_claude_hook, install_claude_hook_config, install_claude_hook_scripts,
};
pub use codex::install_codex_hook;
pub(super) use copilot::install_copilot_hook;
pub use cursor::install_cursor_hook;
pub(super) use cursor::{install_cursor_hook_config, install_cursor_hook_scripts};
pub(super) use gemini::{
    install_gemini_hook, install_gemini_hook_config, install_gemini_hook_scripts,
};
pub(super) use opencode::install_opencode_hook;
pub(super) use windsurf::{install_windsurf, install_windsurf_hooks};
