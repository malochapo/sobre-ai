use lsp_types::Uri;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use super::backend::LspBackend;
use super::client::{LspClient, file_path_to_uri};
use super::config::{
    LspServerConfig, check_server_available, default_servers, language_for_extension,
};

static BACKENDS: std::sync::LazyLock<Mutex<HashMap<String, Box<dyn LspBackend>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn resolve_config_for_language(language: &str) -> LspServerConfig {
    let servers = default_servers();
    servers.get(language).cloned().unwrap_or(LspServerConfig {
        command: format!("{language}-language-server"),
        args: vec![],
    })
}

/// Starts the language server for `language` found on PATH.
fn select_backend(language: &str, project_root: &str) -> Result<Box<dyn LspBackend>, String> {
    // Backing A: rust-analyzer (today's behavior).
    let config = resolve_config_for_language(language);
    if super::config::find_binary_in_path(&config.command).is_none()
        && !Path::new(&config.command).is_file()
    {
        check_server_available(language)?;
    }
    let root_uri = file_path_to_uri(project_root)?;
    let client = LspClient::start(&config, &root_uri)?;
    Ok(Box::new(client) as Box<dyn LspBackend>)
}

pub fn with_backend<F, R>(file_path: &str, project_root: &str, f: F) -> Result<R, String>
where
    F: FnOnce(&mut dyn LspBackend, &str) -> Result<R, String>,
{
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let language = language_for_extension(ext).ok_or_else(|| {
        format!(
            "No LSP server configured for extension '.{ext}'. Supported: rs, ts, tsx, js, py, go"
        )
    })?;

    let mut backends = BACKENDS.lock().map_err(|e| e.to_string())?;

    if !backends.contains_key(language) {
        let backend = select_backend(language, project_root)?;
        backends.insert(language.to_string(), backend);
    }

    let backend = backends
        .get_mut(language)
        .ok_or_else(|| format!("LSP backend for '{language}' not available"))?;

    f(backend.as_mut(), language)
}

pub fn open_file(file_path: &str, project_root: &str) -> Result<Uri, String> {
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    language_for_extension(ext).ok_or_else(|| {
        format!(
            "No LSP server configured for extension '.{ext}'. Supported: rs, ts, tsx, js, py, go"
        )
    })?;

    let content = std::fs::read_to_string(file_path)
        .map_err(|e| format!("Cannot read '{file_path}': {e}"))?;

    let uri = file_path_to_uri(file_path)?;

    with_backend(file_path, project_root, |backend, language| {
        backend.open_file(&uri, language, &content)?;
        Ok(uri.clone())
    })
}

#[cfg(test)]
pub(crate) fn seed_stub_backend(language: &str, backend: Box<dyn LspBackend>) {
    if let Ok(mut backends) = BACKENDS.lock() {
        backends.insert(language.to_string(), backend);
    }
}

/// Serializes tests that seed [`BACKENDS`] stubs: `BACKENDS` is process-global,
/// so two parallel tests seeding the same language would race (one stub
/// overwrites the other between seed and use). Hold this for the whole test.
#[cfg(test)]
pub(crate) fn stub_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
