use ed25519_dalek::{Signer, SigningKey};
use std::path::{Path, PathBuf};

/// The agent identity every subsystem (heatmap, savings ledger, audit)
/// attributes traces to. sobre serves one person on one machine.
#[must_use]
pub fn current_agent_id() -> &'static str {
    "local"
}

pub fn get_or_create_keypair(agent_id: &str) -> Result<SigningKey, String> {
    let path = key_path(agent_id)?;
    if path.exists() {
        load_key(&path)
    } else {
        generate_and_save(agent_id)
    }
}

pub fn sign_bytes(agent_id: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    let key = get_or_create_keypair(agent_id)?;
    let sig = key.sign(data);
    Ok(sig.to_bytes().to_vec())
}

pub fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn key_path(agent_id: &str) -> Result<PathBuf, String> {
    let base = crate::core::data_dir::lean_ctx_data_dir()?;
    Ok(base.join("keys").join(format!("{agent_id}.key")))
}

fn pub_key_path(agent_id: &str) -> Result<PathBuf, String> {
    let base = crate::core::data_dir::lean_ctx_data_dir()?;
    Ok(base.join("keys").join(format!("{agent_id}.pub")))
}

fn generate_and_save(agent_id: &str) -> Result<SigningKey, String> {
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| format!("CSPRNG unavailable: {e}"))?;
    let signing_key = SigningKey::from_bytes(&seed);

    let key_file = key_path(agent_id)?;
    let pub_file = pub_key_path(agent_id)?;

    if let Some(parent) = key_file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir keys: {e}"))?;
    }

    std::fs::write(&key_file, signing_key.to_bytes()).map_err(|e| format!("write key: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        let _ = std::fs::set_permissions(&key_file, perms);
    }

    let pub_bytes = signing_key.verifying_key().to_bytes();
    std::fs::write(&pub_file, pub_bytes).map_err(|e| format!("write pub: {e}"))?;

    Ok(signing_key)
}

fn load_key(path: &Path) -> Result<SigningKey, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read key: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "invalid key file (expected 32 bytes)".to_string())?;
    Ok(SigningKey::from_bytes(&arr))
}
