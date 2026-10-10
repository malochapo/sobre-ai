//! Model download from HuggingFace Hub.
//!
//! Downloads the selected ONNX embedding model, its `tokenizer.json` and its
//! sentence-transformers metadata on explicit request only
//! (`sobre embeddings provision`, or `init`'s question). Files are cached
//! per-model under the models directory and only downloaded once.
//!
//! Supply-chain integrity: after the first successful download a
//! `model.lock.json` records the SHA-256 of every artifact (trust-on-first-use).
//! Any later re-download of the same file must reproduce the pinned hash —
//! an upstream repo that silently swaps bytes under the same revision fails hard.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::model_registry::{EmbeddingModel, POOLING_FILE, PROMPTS_FILE, SEQ_LEN_FILE};

/// Lockfile name storing `{ filename: sha256-hex }` per model directory.
const LOCKFILE: &str = "model.lock.json";

/// Local names of the files a model needs before it can load.
pub const MODEL_FILE: &str = "model.onnx";
pub const TOKENIZER_FILE: &str = "tokenizer.json";

/// Smallest plausible ONNX export (model2vec models are a few MB).
const MODEL_MIN_BYTES: u64 = 1_000_000;
const TOKENIZER_MIN_BYTES: u64 = 1_000;

struct DownloadFile {
    /// Repo paths tried in order; the first one found is saved.
    sources: Vec<&'static str>,
    local_name: &'static str,
    min_bytes: u64,
    /// Metadata files are optional: a repo without them keeps the defaults.
    required: bool,
}

/// Ensure all required model files are present, downloading if necessary.
/// Returns the model directory path on success.
pub fn ensure_model(model_dir: &Path, model: &EmbeddingModel) -> anyhow::Result<PathBuf> {
    let files = download_files();

    if files
        .iter()
        .filter(|f| f.required)
        .all(|f| model_dir.join(f.local_name).exists())
    {
        return Ok(model_dir.to_path_buf());
    }

    let name = model.name();
    tracing::info!(
        "Embedding model '{name}' not found, downloading to {}",
        model_dir.display()
    );
    std::fs::create_dir_all(model_dir)?;

    let mut lock = read_lockfile(model_dir);

    for file in &files {
        let local_path = model_dir.join(file.local_name);
        if local_path.exists() {
            let meta = std::fs::metadata(&local_path)?;
            if meta.len() >= file.min_bytes {
                tracing::debug!("{} already present ({} bytes)", file.local_name, meta.len());
                continue;
            }
            tracing::warn!(
                "{} exists but too small ({} < {}), re-downloading",
                file.local_name,
                meta.len(),
                file.min_bytes
            );
        }

        let mut fetched = false;
        let mut last_err = None;
        for source in &file.sources {
            match download_file(
                &model.file_url(source),
                file.local_name,
                file.min_bytes,
                model_dir,
            ) {
                Ok(()) => {
                    fetched = true;
                    break;
                }
                Err(e) => last_err = Some(e),
            }
        }
        if !fetched {
            if file.required {
                return Err(last_err
                    .unwrap_or_else(|| anyhow::anyhow!("no source for {}", file.local_name))
                    .context(format!(
                        "model '{name}' must ship {} and tokenizer.json",
                        file.sources.join(" or ")
                    )));
            }
            tracing::debug!("{} not in '{name}', using defaults", file.local_name);
            continue;
        }

        let actual = sha256_file(&model_dir.join(file.local_name))?;
        match lock.get(file.local_name) {
            Some(pinned) if pinned != &actual => {
                let _ = std::fs::remove_file(model_dir.join(file.local_name));
                anyhow::bail!(
                    "SHA-256 mismatch for {} of model '{name}': pinned {pinned}, got {actual}. \
                     Upstream content changed under the same revision — refusing the file. \
                     If this is intentional, delete {} and re-download.",
                    file.local_name,
                    model_dir.join(LOCKFILE).display()
                );
            }
            Some(_) => {}
            None => {
                lock.insert(file.local_name.to_string(), actual);
            }
        }
    }

    write_lockfile(model_dir, &lock)?;

    tracing::info!("Embedding model '{name}' ready at {}", model_dir.display());
    Ok(model_dir.to_path_buf())
}

fn download_files() -> Vec<DownloadFile> {
    let required = |sources, local_name, min_bytes| DownloadFile {
        sources,
        local_name,
        min_bytes,
        required: true,
    };
    let optional = |source, local_name| DownloadFile {
        sources: vec![source],
        local_name,
        min_bytes: 1,
        required: false,
    };
    vec![
        required(
            vec!["onnx/model.onnx", "model.onnx"],
            MODEL_FILE,
            MODEL_MIN_BYTES,
        ),
        required(vec!["tokenizer.json"], TOKENIZER_FILE, TOKENIZER_MIN_BYTES),
        optional("1_Pooling/config.json", POOLING_FILE),
        optional("config_sentence_transformers.json", PROMPTS_FILE),
        optional("sentence_bert_config.json", SEQ_LEN_FILE),
    ]
}

fn download_file(
    url: &str,
    local_name: &str,
    min_bytes: u64,
    model_dir: &Path,
) -> anyhow::Result<()> {
    let local_path = model_dir.join(local_name);
    let tmp_path = model_dir.join(format!("{local_name}.tmp"));

    tracing::info!("Downloading {local_name} ...");

    // Bounds connection setup only: a large but progressing download is
    // never cut off.
    let agent = crate::core::http_client::ureq_agent_with_timeouts(
        Some(Duration::from_secs(30)),
        Some(Duration::from_secs(30)),
        Some(Duration::from_secs(60)),
    );
    let response = agent
        .get(url)
        .header("User-Agent", format!("sobre/{}", crate::VERSION))
        .call()
        .map_err(|e| anyhow::anyhow!("Failed to download {url}: {e}"))?;

    let status = response.status();
    if status != 200 {
        anyhow::bail!("Download of {local_name} returned HTTP {status}");
    }

    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let mut body = response.into_body().into_reader();
    let mut out = std::fs::File::create(&tmp_path)?;
    let mut buf = vec![0u8; 65536];
    let mut total: u64 = 0;
    let mut last_report: u64 = 0;

    loop {
        let n = body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buf[..n])?;
        total += n as u64;

        if total - last_report > 1_000_000 {
            if let Some(cl) = content_length {
                let pct = (total as f64 / cl as f64 * 100.0) as u32;
                tracing::info!(
                    "  {local_name} — {:.1}MB / {:.1}MB ({pct}%)",
                    total as f64 / 1_048_576.0,
                    cl as f64 / 1_048_576.0,
                );
            } else {
                tracing::info!(
                    "  {local_name} — {:.1}MB downloaded",
                    total as f64 / 1_048_576.0
                );
            }
            last_report = total;
        }
    }
    drop(out);

    if total < min_bytes {
        let _ = std::fs::remove_file(&tmp_path);
        anyhow::bail!(
            "Downloaded {local_name} is too small ({total} bytes, expected >= {min_bytes})",
        );
    }

    std::fs::rename(&tmp_path, &local_path)?;
    tracing::info!("  {local_name} — {:.1}MB saved", total as f64 / 1_048_576.0);

    Ok(())
}

/// The SHA-256 of a file as lowercase hex.
fn sha256_file(path: &Path) -> anyhow::Result<String> {
    crate::core::artifacts_exec::binhash::sha256_file(path).map_err(anyhow::Error::msg)
}

fn read_lockfile(model_dir: &Path) -> std::collections::BTreeMap<String, String> {
    std::fs::read_to_string(model_dir.join(LOCKFILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_lockfile(
    model_dir: &Path,
    lock: &std::collections::BTreeMap<String, String>,
) -> anyhow::Result<()> {
    if lock.is_empty() {
        return Ok(());
    }
    let json = serde_json::to_string_pretty(lock)?;
    std::fs::write(model_dir.join(LOCKFILE), json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_files_come_first_and_urls_are_https() {
        let files = download_files();
        assert_eq!(files[0].local_name, MODEL_FILE);
        assert_eq!(files[1].local_name, TOKENIZER_FILE);
        assert!(files[..2].iter().all(|f| f.required));
        assert!(files[2..].iter().all(|f| !f.required));
        let model = EmbeddingModel::parse("org/model@pin").unwrap();
        for f in &files {
            for source in &f.sources {
                assert!(
                    model
                        .file_url(source)
                        .starts_with("https://huggingface.co/org/model/resolve/pin/")
                );
            }
        }
    }

    #[test]
    fn sha256_and_lockfile_roundtrip() {
        let dir = std::env::temp_dir().join("sobre_test_lockfile_v1");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let f = dir.join("model.onnx");
        std::fs::write(&f, b"abc").unwrap();
        let h = sha256_file(&f).unwrap();
        assert_eq!(
            h,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );

        let mut lock = std::collections::BTreeMap::new();
        lock.insert("model.onnx".to_string(), h);
        write_lockfile(&dir, &lock).unwrap();
        assert_eq!(read_lockfile(&dir), lock);

        // Corrupt lockfile degrades to empty (TOFU re-pin), never a panic.
        std::fs::write(dir.join(LOCKFILE), "not json").unwrap();
        assert!(read_lockfile(&dir).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_lockfile_is_not_written() {
        let dir = std::env::temp_dir().join("sobre_test_lockfile_empty_v1");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_lockfile(&dir, &std::collections::BTreeMap::new()).unwrap();
        assert!(!dir.join(LOCKFILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
