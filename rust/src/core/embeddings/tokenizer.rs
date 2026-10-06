//! Tokenization through the model's own HuggingFace `tokenizer.json`.

#[derive(Debug, Clone)]
pub struct TokenizedInput {
    pub input_ids: Vec<i32>,
    pub attention_mask: Vec<i32>,
    pub token_type_ids: Vec<i32>,
}

#[cfg(feature = "embeddings")]
pub struct Tokenizer {
    inner: tokenizers::Tokenizer,
    add_special_tokens: bool,
}

#[cfg(feature = "embeddings")]
impl Tokenizer {
    /// Load `tokenizer.json`, truncating at `max_len` and never padding
    /// (batches are padded to their own longest input). Static-embedding
    /// models (model2vec) take no special tokens.
    pub fn from_file(
        path: &std::path::Path,
        max_len: usize,
        add_special_tokens: bool,
    ) -> anyhow::Result<Self> {
        let mut inner = tokenizers::Tokenizer::from_file(path)
            .map_err(|e| anyhow::anyhow!("invalid {}: {e}", path.display()))?;
        Self::configure(&mut inner, max_len)?;
        Ok(Self {
            inner,
            add_special_tokens,
        })
    }

    #[cfg(test)]
    fn from_json(json: &str, max_len: usize) -> anyhow::Result<Self> {
        let mut inner: tokenizers::Tokenizer = json
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid tokenizer.json: {e}"))?;
        Self::configure(&mut inner, max_len)?;
        Ok(Self {
            inner,
            add_special_tokens: true,
        })
    }

    fn configure(inner: &mut tokenizers::Tokenizer, max_len: usize) -> anyhow::Result<()> {
        inner.with_padding(None);
        inner
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: max_len,
                ..Default::default()
            }))
            .map_err(|e| anyhow::anyhow!("tokenizer truncation: {e}"))?;
        Ok(())
    }

    pub fn encode(&self, text: &str) -> anyhow::Result<TokenizedInput> {
        let enc = self
            .inner
            .encode(text, self.add_special_tokens)
            .map_err(|e| anyhow::anyhow!("tokenize: {e}"))?;
        let ids = |v: &[u32]| v.iter().map(|&x| x as i32).collect::<Vec<_>>();
        Ok(TokenizedInput {
            input_ids: ids(enc.get_ids()),
            attention_mask: ids(enc.get_attention_mask()),
            token_type_ids: ids(enc.get_type_ids()),
        })
    }
}

#[cfg(all(test, feature = "embeddings"))]
mod tests {
    use super::*;

    fn wordpiece_json() -> String {
        r###"{
          "version": "1.0",
          "normalizer": {"type": "BertNormalizer", "clean_text": true, "handle_chinese_chars": true, "strip_accents": null, "lowercase": true},
          "pre_tokenizer": {"type": "BertPreTokenizer"},
          "post_processor": {"type": "TemplateProcessing",
            "single": [{"SpecialToken": {"id": "[CLS]", "type_id": 0}}, {"Sequence": {"id": "A", "type_id": 0}}, {"SpecialToken": {"id": "[SEP]", "type_id": 0}}],
            "pair": [{"Sequence": {"id": "A", "type_id": 0}}],
            "special_tokens": {"[CLS]": {"id": "[CLS]", "ids": [1], "tokens": ["[CLS]"]}, "[SEP]": {"id": "[SEP]", "ids": [2], "tokens": ["[SEP]"]}}},
          "model": {"type": "WordPiece", "unk_token": "[UNK]", "continuing_subword_prefix": "##", "max_input_chars_per_word": 100,
            "vocab": {"[UNK]": 0, "[CLS]": 1, "[SEP]": 2, "get": 3, "##user": 4, "##name": 5, "(": 6, ")": 7}}
        }"###.to_string()
    }

    fn bpe_json() -> String {
        r#"{
          "version": "1.0",
          "pre_tokenizer": {"type": "ByteLevel", "add_prefix_space": false, "trim_offsets": true, "use_regex": true},
          "model": {"type": "BPE", "unk_token": null, "fuse_unk": false, "byte_fallback": false,
            "vocab": {"f": 0, "n": 1, "Ġ": 2, "m": 3, "a": 4, "i": 5, "fn": 6, "Ġm": 7, "Ġma": 8, "Ġmai": 9, "Ġmain": 10},
            "merges": ["f n", "Ġ m", "Ġm a", "Ġma i", "Ġmai n"]}
        }"#.to_string()
    }

    #[test]
    fn wordpiece_adds_special_tokens_and_subwords() {
        let tok = Tokenizer::from_json(&wordpiece_json(), 32).unwrap();
        let enc = tok.encode("getUserName()").unwrap();
        assert_eq!(enc.input_ids, vec![1, 3, 4, 5, 6, 7, 2]);
        assert_eq!(enc.attention_mask, vec![1; 7]);
        assert_eq!(enc.token_type_ids, vec![0; 7]);
    }

    #[test]
    fn bpe_uses_merges_not_wordpiece() {
        let tok = Tokenizer::from_json(&bpe_json(), 32).unwrap();
        assert_eq!(tok.encode("fn main").unwrap().input_ids, vec![6, 10]);
    }

    #[test]
    fn truncates_without_padding() {
        let tok = Tokenizer::from_json(&wordpiece_json(), 4).unwrap();
        let enc = tok.encode("getUserName()").unwrap();
        assert_eq!(enc.input_ids, vec![1, 3, 4, 2]);
        let short = tok.encode("get").unwrap();
        assert_eq!(short.input_ids.len(), 3);
    }
}
