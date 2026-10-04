use crate::core::memory_policy::MemoryPolicy;

pub(crate) fn load_policy_or_error() -> Result<MemoryPolicy, String> {
    let cfg = crate::core::config::Config::load();
    let path = crate::core::config::Config::path().map_or_else(
        || "~/.lean-ctx/config.toml".to_string(),
        |p| p.display().to_string(),
    );

    let policy = cfg
        .memory_policy_effective()
        .map_err(|e| format!("Error: invalid memory policy: {e}\nFix: edit {path}"))?;

    policy
        .validate()
        .map_err(|e| format!("Error: invalid memory policy: {e}\nFix: edit {path}"))?;

    Ok(policy)
}
