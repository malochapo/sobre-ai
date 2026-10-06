//! Shell rc/profile proxy env exports.

use std::path::Path;

use crate::marked_block;

use super::claude::anthropic_api_key_available;
use super::util::{ANTHROPIC_OMITTED_NOTE, PROXY_ENV_END, PROXY_ENV_START, is_proxy_reachable};

pub(crate) fn install_shell_exports(home: &Path, port: u16, quiet: bool) {
    if !is_proxy_reachable(port) {
        if !quiet {
            println!("  Skipping shell proxy exports (proxy not running on port {port})");
        }
        return;
    }

    let base = format!("http://127.0.0.1:{port}");
    // OpenAI SDK convention: the base URL INCLUDES the `/v1` prefix (default is
    // `https://api.openai.com/v1`); clients append bare endpoints like `/responses`.
    // Without `/v1`, OpenCode's ChatGPT-OAuth plugin fails to recognize Responses-API
    // requests (it matches on `/v1/responses`) and OAuth traffic leaks to the platform
    // API with the wrong credential ("Missing scopes: api.responses.write", #366).
    // Anthropic and Gemini SDKs expect a bare origin instead — they append `/v1/...`
    // / `/v1beta/...` themselves.
    let openai_base = format!("{base}/v1");

    // Only route Claude through the proxy when an API key is available; a Pro/Max
    // subscription must keep talking to api.anthropic.com directly (see
    // `anthropic_api_key_available`).
    let include_anthropic = anthropic_api_key_available(home);

    let posix_anthropic = if include_anthropic {
        format!(r#"export ANTHROPIC_BASE_URL="{base}""#)
    } else {
        format!("# {ANTHROPIC_OMITTED_NOTE}")
    };
    let posix_block = format!(
        r#"{PROXY_ENV_START}
{posix_anthropic}
export OPENAI_BASE_URL="{openai_base}"
export GEMINI_API_BASE_URL="{base}"
{PROXY_ENV_END}"#
    );

    for rc in &[home.join(".zshrc"), home.join(".bashrc")] {
        if rc.exists() {
            let label = format!(
                "proxy env in ~/{}",
                rc.file_name().unwrap_or_default().to_string_lossy()
            );
            marked_block::upsert(
                rc,
                PROXY_ENV_START,
                PROXY_ENV_END,
                &posix_block,
                quiet,
                &label,
            );
        }
    }

    let fish_config = home.join(".config/fish/config.fish");
    if fish_config.exists() {
        let fish_anthropic = if include_anthropic {
            format!(r#"set -gx ANTHROPIC_BASE_URL "{base}""#)
        } else {
            format!("# {ANTHROPIC_OMITTED_NOTE}")
        };
        let fish_block = format!(
            r#"{PROXY_ENV_START}
{fish_anthropic}
set -gx OPENAI_BASE_URL "{openai_base}"
set -gx GEMINI_API_BASE_URL "{base}"
{PROXY_ENV_END}"#
        );
        marked_block::upsert(
            &fish_config,
            PROXY_ENV_START,
            PROXY_ENV_END,
            &fish_block,
            quiet,
            "proxy env in ~/.config/fish/config.fish",
        );
    }

    let ps_profile =
        dirs::home_dir().map(|h| crate::shell::platform::resolve_powershell_profile_path(&h));
    if let Some(ref ps) = ps_profile
        && ps.exists()
    {
        let ps_anthropic = if include_anthropic {
            format!(r#"$env:ANTHROPIC_BASE_URL = "{base}""#)
        } else {
            format!("# {ANTHROPIC_OMITTED_NOTE}")
        };
        let ps_block = format!(
            r#"{PROXY_ENV_START}
{ps_anthropic}
$env:OPENAI_BASE_URL = "{openai_base}"
$env:GEMINI_API_BASE_URL = "{base}"
{PROXY_ENV_END}"#
        );
        marked_block::upsert(
            ps,
            PROXY_ENV_START,
            PROXY_ENV_END,
            &ps_block,
            quiet,
            "proxy env in PowerShell profile",
        );
    }
}
