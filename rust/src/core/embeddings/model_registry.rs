//! Embedding model selection and per-model metadata.
//!
//! Every model is a HuggingFace repository that ships an ONNX export and a
//! `tokenizer.json`. The `embedding_model` key in `config.toml` names it,
//! either by alias (`minilm`, the default, or `nomic`) or as
//! `org/repo[@revision]` (an optional `hf:` prefix is accepted). Pooling,
//! query/document prompts and the sequence length come from the repo's
//! sentence-transformers metadata, so no model needs a hand-written entry.

use std::path::Path;

/// Short names for well-known models.
const ALIASES: &[(&str, &str)] = &[
    ("minilm", "sentence-transformers/all-MiniLM-L6-v2"),
    ("nomic", "nomic-ai/nomic-embed-text-v1.5"),
];

/// Longest sequence fed to the model, whatever the model allows: attention
/// cost grows with the square of the length.
const MAX_SEQ_LEN_CAP: usize = 512;

/// A HuggingFace embedding model, optionally pinned to a revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EmbeddingModel {
    pub repo: String,
    /// Tag, branch or commit. `None` resolves `main`.
    pub revision: Option<String>,
}

impl EmbeddingModel {
    pub fn default_model() -> Self {
        Self::parse(ALIASES[0].0).expect("default alias is valid")
    }

    /// Parse an alias or `[hf:]org/repo[@revision]`. `None` when the value
    /// is neither an alias nor a plausible repo id.
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if let Some((_, repo)) = ALIASES
            .iter()
            .find(|(alias, _)| alias.eq_ignore_ascii_case(value))
        {
            return Some(Self {
                repo: (*repo).to_string(),
                revision: None,
            });
        }
        let spec = value.strip_prefix("hf:").unwrap_or(value);
        let (repo, revision) = match spec.split_once('@') {
            Some((repo, rev)) => (repo.trim(), Some(rev.trim()).filter(|r| !r.is_empty())),
            None => (spec, None),
        };
        let mut parts = repo.split('/');
        let (owner, name) = (parts.next()?, parts.next()?);
        if parts.next().is_some()
            || owner.is_empty()
            || name.is_empty()
            || repo.chars().any(char::is_whitespace)
        {
            return None;
        }
        Some(Self {
            repo: repo.to_string(),
            revision: revision.map(str::to_string),
        })
    }

    /// Canonical name, also stored as the index `model_id`: changing the repo
    /// or the revision triggers a one-time re-index.
    pub fn name(&self) -> String {
        match &self.revision {
            Some(rev) => format!("{}@{rev}", self.repo),
            None => self.repo.clone(),
        }
    }

    /// Filesystem-safe directory name, unique per repo and revision.
    pub fn storage_dir_name(&self) -> String {
        self.name()
            .chars()
            .map(|c| match c {
                'a'..='z' | '0'..='9' | '-' | '.' => c,
                'A'..='Z' => c.to_ascii_lowercase(),
                _ => '-',
            })
            .collect()
    }

    /// Download URL of a file in the repo.
    pub fn file_url(&self, path: &str) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{path}",
            self.repo,
            self.revision.as_deref().unwrap_or("main")
        )
    }
}

/// The model named by `embedding_model` in `config.toml`, or the default.
pub fn resolve_model() -> EmbeddingModel {
    let configured = crate::core::config::Config::load().embedding_model;
    let Some(value) = configured.as_deref().filter(|v| !v.trim().is_empty()) else {
        return EmbeddingModel::default_model();
    };
    EmbeddingModel::parse(value).unwrap_or_else(|| {
        tracing::warn!(
            "embedding_model {value:?} is neither an alias ({}) nor org/repo[@revision]; \
             using the default",
            ALIASES
                .iter()
                .map(|(alias, _)| *alias)
                .collect::<Vec<_>>()
                .join(", ")
        );
        EmbeddingModel::default_model()
    })
}

/// How per-token hidden states become one vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pooling {
    Mean,
    Cls,
    LastToken,
}

/// What the model expects around inference, read from its metadata files.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelMeta {
    pub pooling: Pooling,
    pub query_prefix: Option<String>,
    pub document_prefix: Option<String>,
    pub max_seq_len: usize,
}

/// Local names of the sentence-transformers metadata files.
pub const POOLING_FILE: &str = "pooling_config.json";
pub const PROMPTS_FILE: &str = "config_sentence_transformers.json";
pub const SEQ_LEN_FILE: &str = "sentence_bert_config.json";

impl ModelMeta {
    /// Read the metadata stored next to the model. Missing or unreadable
    /// files fall back to mean pooling, no prompts and the length cap.
    pub fn load(model_dir: &Path) -> Self {
        let read = |name: &str| -> Option<serde_json::Value> {
            serde_json::from_str(&std::fs::read_to_string(model_dir.join(name)).ok()?).ok()
        };
        Self::from_json(
            read(POOLING_FILE).as_ref(),
            read(PROMPTS_FILE).as_ref(),
            read(SEQ_LEN_FILE).as_ref(),
        )
    }

    fn from_json(
        pooling: Option<&serde_json::Value>,
        prompts: Option<&serde_json::Value>,
        seq_len: Option<&serde_json::Value>,
    ) -> Self {
        let flag = |key: &str| {
            pooling
                .and_then(|p| p.get(key))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        };
        let pooling = if flag("pooling_mode_cls_token") {
            Pooling::Cls
        } else if flag("pooling_mode_lasttoken") {
            Pooling::LastToken
        } else {
            Pooling::Mean
        };
        let prompt = |keys: &[&str]| {
            let table = prompts.and_then(|p| p.get("prompts"))?;
            keys.iter()
                .find_map(|k| table.get(*k).and_then(serde_json::Value::as_str))
                .filter(|p| !p.is_empty())
                .map(str::to_string)
        };
        let max_seq_len = seq_len
            .and_then(|s| s.get("max_seq_length"))
            .and_then(serde_json::Value::as_u64)
            .map_or(MAX_SEQ_LEN_CAP, |n| (n as usize).clamp(1, MAX_SEQ_LEN_CAP));
        Self {
            pooling,
            query_prefix: prompt(&["query", "search_query"]),
            document_prefix: prompt(&["document", "passage", "search_document"]),
            max_seq_len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aliases_resolve_to_repos() {
        assert_eq!(
            EmbeddingModel::default_model().repo,
            "sentence-transformers/all-MiniLM-L6-v2"
        );
        assert_eq!(
            EmbeddingModel::parse("Nomic").unwrap().repo,
            "nomic-ai/nomic-embed-text-v1.5"
        );
    }

    #[test]
    fn repo_specs_parse_with_or_without_scheme() {
        let m = EmbeddingModel::parse("hf:jinaai/jina-embeddings-v2-base-code").unwrap();
        assert_eq!(m.repo, "jinaai/jina-embeddings-v2-base-code");
        assert_eq!(m.revision, None);
        let m = EmbeddingModel::parse(" BAAI/bge-small-en-v1.5@abc123 ").unwrap();
        assert_eq!(m.repo, "BAAI/bge-small-en-v1.5");
        assert_eq!(m.revision.as_deref(), Some("abc123"));
        assert_eq!(m.name(), "BAAI/bge-small-en-v1.5@abc123");
    }

    #[test]
    fn invalid_specs_are_rejected() {
        for bad in ["", "unknown", "org/", "/repo", "a/b/c", "org/re po", "hf:"] {
            assert_eq!(EmbeddingModel::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn storage_dirs_and_urls() {
        let m = EmbeddingModel::parse("Org/My_Model@v1.2").unwrap();
        assert_eq!(m.storage_dir_name(), "org-my-model-v1.2");
        assert_eq!(
            m.file_url("onnx/model.onnx"),
            "https://huggingface.co/Org/My_Model/resolve/v1.2/onnx/model.onnx"
        );
        let unpinned = EmbeddingModel::parse("org/model").unwrap();
        assert!(
            unpinned
                .file_url("tokenizer.json")
                .contains("/resolve/main/")
        );
        assert_ne!(unpinned.storage_dir_name(), m.storage_dir_name());
    }

    #[test]
    fn meta_defaults_without_files() {
        let meta = ModelMeta::from_json(None, None, None);
        assert_eq!(meta.pooling, Pooling::Mean);
        assert_eq!(meta.query_prefix, None);
        assert_eq!(meta.document_prefix, None);
        assert_eq!(meta.max_seq_len, MAX_SEQ_LEN_CAP);
    }

    #[test]
    fn meta_reads_sentence_transformers_files() {
        let pooling = json!({"pooling_mode_cls_token": true, "pooling_mode_mean_tokens": false});
        let prompts =
            json!({"prompts": {"query": "search_query: ", "document": "search_document: "}});
        let seq = json!({"max_seq_length": 256});
        let meta = ModelMeta::from_json(Some(&pooling), Some(&prompts), Some(&seq));
        assert_eq!(meta.pooling, Pooling::Cls);
        assert_eq!(meta.query_prefix.as_deref(), Some("search_query: "));
        assert_eq!(meta.document_prefix.as_deref(), Some("search_document: "));
        assert_eq!(meta.max_seq_len, 256);
    }

    #[test]
    fn meta_caps_length_and_reads_last_token_pooling() {
        let pooling = json!({"pooling_mode_lasttoken": true});
        let prompts = json!({"prompts": {"query": "", "passage": "passage: "}});
        let seq = json!({"max_seq_length": 8192});
        let meta = ModelMeta::from_json(Some(&pooling), Some(&prompts), Some(&seq));
        assert_eq!(meta.pooling, Pooling::LastToken);
        assert_eq!(meta.query_prefix, None);
        assert_eq!(meta.document_prefix.as_deref(), Some("passage: "));
        assert_eq!(meta.max_seq_len, MAX_SEQ_LEN_CAP);
    }
}
