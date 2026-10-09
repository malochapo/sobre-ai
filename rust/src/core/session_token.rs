/// Auto-generated session tokens for the proxy.
///
/// Security principle: Least Action — minimize attack surface by always having
/// a token, even when the user hasn't explicitly configured one. The token is
/// written to a file with restrictive permissions so authorized local clients
/// can discover it.
use std::path::PathBuf;

const TOKEN_BYTES: usize = 32;
const TOKEN_FILE: &str = "session_token";

/// Generate a cryptographically random hex token.
pub fn generate_token() -> String {
    let mut buf = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut buf).expect("getrandom failed");
    crate::core::hasher::hex_encode(&buf)
}

/// Resolve or generate the proxy/HTTP session token.
///
/// Priority:
/// 1. Explicit env var (user override)
/// 2. Existing token file (persistence across restarts)
/// 3. Generate new + write to file
pub fn resolve_proxy_token(env_var: &str) -> String {
    if let Ok(val) = std::env::var(env_var)
        && !val.trim().is_empty()
    {
        return val.trim().to_string();
    }

    let token_path = token_file_path();
    if let Some(Ok(existing)) = token_path.as_ref().map(std::fs::read_to_string) {
        let t = existing.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }

    let token = generate_token();
    if let Some(path) = &token_path {
        write_token_file(path, &token);
    }
    token
}

/// Write token to file with restrictive permissions (0600 on Unix).
fn write_token_file(path: &PathBuf, token: &str) {
    if let Some(parent) = path.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        tracing::error!("Failed to create token directory {}: {e}", parent.display());
        return;
    }
    if let Err(e) = std::fs::write(path, token) {
        tracing::error!("Failed to write session token to {}: {e}", path.display());
        return;
    }
    set_restrictive_permissions(path);
}

fn token_file_path() -> Option<PathBuf> {
    Some(
        crate::core::data_dir::sobre_data_dir()
            .ok()?
            .join(TOKEN_FILE),
    )
}

#[cfg(unix)]
fn set_restrictive_permissions(path: &PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(0o600);
    let _ = std::fs::set_permissions(path, perms);
}

#[cfg(not(unix))]
fn set_restrictive_permissions(_path: &PathBuf) {
    // Windows: rely on user-scoped directories
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_token_is_64_hex_chars() {
        let token = generate_token();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generated_tokens_are_unique() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
    }
}
