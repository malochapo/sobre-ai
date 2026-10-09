//! A chunk of non-file content for the BM25 index: the code-health hotspot
//! notes (`health://…`), ingested next to the code chunks through
//! `From<ContentChunk> for CodeChunk`.

use super::bm25_index::{ChunkKind, CodeChunk};

/// A content chunk addressed by a `provider://resource/id` URI.
#[derive(Debug, Clone)]
pub struct ContentChunk {
    pub file_path: String,
    pub symbol_name: String,
    pub kind: ChunkKind,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub tokens: Vec<String>,
    pub token_count: usize,

    /// URIs or file paths that this chunk references.
    pub references: Vec<String>,

    /// Provider-specific structured metadata.
    pub metadata: Option<serde_json::Value>,
}

impl ContentChunk {
    pub fn from_provider(
        provider_id: &str,
        resource_type: &str,
        item_id: &str,
        title: &str,
        kind: ChunkKind,
        content: String,
        references: Vec<String>,
        metadata: Option<serde_json::Value>,
    ) -> Self {
        let tokens = super::bm25_index::tokenize_for_index(&content);
        let token_count = tokens.len();
        Self {
            file_path: format!("{provider_id}://{resource_type}/{item_id}"),
            symbol_name: title.to_string(),
            kind,
            start_line: 0,
            end_line: 0,
            content,
            tokens,
            token_count,
            references,
            metadata,
        }
    }
}

impl From<ContentChunk> for CodeChunk {
    fn from(c: ContentChunk) -> Self {
        Self {
            file_path: c.file_path,
            symbol_name: c.symbol_name,
            kind: c.kind,
            start_line: c.start_line,
            end_line: c.end_line,
            content: c.content,
            tokens: c.tokens,
            token_count: c.token_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_chunk_to_code_chunk_roundtrip() {
        let cc = ContentChunk::from_provider(
            "github",
            "issues",
            "123",
            "Auth token expiry",
            ChunkKind::Other,
            "Token expires after 1h".into(),
            vec!["src/auth.rs".into()],
            None,
        );

        assert_eq!(cc.file_path, "github://issues/123");

        let code_chunk: CodeChunk = cc.into();
        assert_eq!(code_chunk.file_path, "github://issues/123");
        assert_eq!(code_chunk.symbol_name, "Auth token expiry");
    }
}
