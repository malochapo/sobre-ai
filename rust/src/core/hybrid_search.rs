//! Hybrid search combining BM25 (lexical) with dense vector search.
//!
//! Uses Reciprocal Rank Fusion (RRF) to merge BM25 and dense embeddings into
//! one ranked list.
//!
//! Formula: score(d) = Σ 1/(k + rank_i(d))
//! where k=60 (standard constant), and i ranges over retrieval methods.
//!
//! Reference: Cormack, Clarke & Buettcher (2009), "Reciprocal Rank Fusion
//! outperforms Condorcet and individual Rank Learning Methods"

#[cfg(any(test, feature = "dense"))]
use std::collections::HashMap;

#[cfg(feature = "dense")]
use super::bm25_index::BM25Index;
use super::bm25_index::{ChunkKind, SearchResult};

#[cfg(feature = "dense")]
use super::embeddings::EmbeddingEngine;
#[cfg(feature = "dense")]
use super::hnsw::FlatEmbeddings;

#[cfg(any(test, feature = "dense"))]
const RRF_K: f64 = 60.0;

/// Default weights for standard RRF: equal contribution per ranking (`Σ weight/(k+r)` with weight=1).
#[cfg(any(test, feature = "dense"))]
const DEFAULT_BM25_WEIGHT: f64 = 1.0;
#[cfg(any(test, feature = "dense"))]
const DEFAULT_DENSE_WEIGHT: f64 = 1.0;

/// Hybrid search weights and candidate counts (built-in, not configurable).
#[cfg(any(test, feature = "dense"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HybridConfig {
    pub bm25_weight: f64,
    pub dense_weight: f64,
    pub bm25_candidates: usize,
    pub dense_candidates: usize,
    /// Weight for SPLADE expansion signal. 0.0 disables SPLADE.
    #[serde(default = "default_splade_weight")]
    pub splade_weight: f64,
}

#[cfg(any(test, feature = "dense"))]
fn default_splade_weight() -> f64 {
    0.5
}

#[cfg(any(test, feature = "dense"))]
impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            bm25_weight: DEFAULT_BM25_WEIGHT,
            dense_weight: DEFAULT_DENSE_WEIGHT,
            bm25_candidates: 75,
            dense_candidates: 75,
            splade_weight: 0.5,
        }
    }
}

/// Fuse two ranked result lists using Reciprocal Rank Fusion.
#[cfg(any(test, feature = "dense"))]
pub fn reciprocal_rank_fusion(
    bm25_results: &[SearchResult],
    dense_results: &[DenseSearchResult],
    config: &HybridConfig,
    top_k: usize,
) -> Vec<HybridResult> {
    let mut scores: HashMap<String, HybridResult> = HashMap::new();

    for (rank, result) in bm25_results.iter().enumerate() {
        let key = result_key(&result.file_path, result.start_line);
        let rrf_score = config.bm25_weight / (RRF_K + rank as f64 + 1.0);

        let entry = scores.entry(key).or_insert_with(|| HybridResult {
            file_path: result.file_path.clone(),
            symbol_name: result.symbol_name.clone(),
            kind: result.kind.clone(),
            start_line: result.start_line,
            end_line: result.end_line,
            snippet: result.snippet.clone(),
            rrf_score: 0.0,
            dense_score: None,
            bm25_rank: None,
            dense_rank: None,
        });
        entry.rrf_score += rrf_score;
        entry.bm25_rank = Some(rank + 1);
    }

    for (rank, result) in dense_results.iter().enumerate() {
        let key = result_key(&result.file_path, result.start_line);
        let rrf_score = config.dense_weight / (RRF_K + rank as f64 + 1.0);

        let entry = scores.entry(key).or_insert_with(|| HybridResult {
            file_path: result.file_path.clone(),
            symbol_name: result.symbol_name.clone(),
            kind: result.kind.clone(),
            start_line: result.start_line,
            end_line: result.end_line,
            snippet: result.snippet.clone(),
            rrf_score: 0.0,
            dense_score: None,
            bm25_rank: None,
            dense_rank: None,
        });
        entry.rrf_score += rrf_score;
        entry.dense_score = Some(result.similarity);
        entry.dense_rank = Some(rank + 1);
    }

    let mut results: Vec<HybridResult> = scores.into_values().collect();
    results.sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(top_k);
    results
}

/// Run hybrid search: BM25 + dense retrieval with RRF fusion + post-RRF reranking.
/// Falls back to BM25-only if embedding engine is not available.
#[cfg(feature = "dense")]
pub fn hybrid_search(
    query: &str,
    index: &BM25Index,
    engine: Option<&EmbeddingEngine>,
    chunk_embeddings: Option<&FlatEmbeddings>,
    top_k: usize,
    config: &HybridConfig,
) -> Vec<HybridResult> {
    let bm25_results = index.search(query, config.bm25_candidates);

    let dense_results = match (engine, chunk_embeddings) {
        (Some(eng), Some(embeddings)) => dense_search(
            query,
            eng,
            &index.chunks,
            embeddings,
            config.dense_candidates,
        ),
        _ => Vec::new(),
    };

    // Over-fetch candidates for reranking (5x top_k, capped at available)
    let candidate_count = (top_k * 5).min(config.bm25_candidates);

    let mut results = if dense_results.is_empty() {
        bm25_results
            .into_iter()
            .take(candidate_count)
            .map(HybridResult::from_bm25)
            .collect()
    } else {
        reciprocal_rank_fusion(&bm25_results, &dense_results, config, candidate_count)
    };

    super::search_reranking::rerank_pipeline(&mut results, query, top_k);
    results
}

/// Dense vector search over pre-computed chunk embeddings.
/// Uses O(n log k) binary-heap top-k selection for small indices, HNSW for large ones.
#[cfg(feature = "dense")]
fn dense_search(
    query: &str,
    engine: &EmbeddingEngine,
    chunks: &[super::bm25_index::CodeChunk],
    embeddings: &FlatEmbeddings,
    top_k: usize,
) -> Vec<DenseSearchResult> {
    let Ok(query_embedding) = engine.embed_query(query) else {
        return Vec::new();
    };

    // Threshold-gated ANN: exact SIMD brute force for small corpora, cached
    // HNSW (sub-linear) once the corpus is large enough to amortize graph build.
    let scored = super::ann_cache::topk(embeddings, &query_embedding, top_k);

    scored
        .into_iter()
        .filter_map(|(idx, sim)| {
            let chunk = chunks.get(idx)?;
            let snippet = chunk.content.lines().take(5).collect::<Vec<_>>().join("\n");
            Some(DenseSearchResult {
                similarity: sim,
                file_path: chunk.file_path.clone(),
                symbol_name: chunk.symbol_name.clone(),
                kind: chunk.kind.clone(),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                snippet,
            })
        })
        .collect()
}

#[cfg(any(test, feature = "dense"))]
fn result_key(file_path: &str, start_line: usize) -> String {
    format!("{file_path}:{start_line}")
}

/// Result from dense (embedding-based) search.
#[cfg(any(test, feature = "dense"))]
#[derive(Debug, Clone)]
pub struct DenseSearchResult {
    pub similarity: f32,
    pub file_path: String,
    pub symbol_name: String,
    pub kind: ChunkKind,
    pub start_line: usize,
    pub end_line: usize,
    pub snippet: String,
}

/// Fused result combining BM25 and dense scores.
#[derive(Debug, Clone)]
pub struct HybridResult {
    pub file_path: String,
    pub symbol_name: String,
    pub kind: ChunkKind,
    pub start_line: usize,
    pub end_line: usize,
    pub snippet: String,
    pub rrf_score: f64,
    pub dense_score: Option<f32>,
    pub bm25_rank: Option<usize>,
    pub dense_rank: Option<usize>,
}

impl HybridResult {
    pub fn from_bm25_public(result: SearchResult) -> Self {
        Self::from_bm25(result)
    }

    fn from_bm25(result: SearchResult) -> Self {
        Self {
            file_path: result.file_path,
            symbol_name: result.symbol_name,
            kind: result.kind,
            start_line: result.start_line,
            end_line: result.end_line,
            snippet: result.snippet,
            rrf_score: result.score,
            dense_score: None,
            bm25_rank: None,
            dense_rank: None,
        }
    }

    pub fn source_label(&self) -> &'static str {
        match (self.bm25_rank.is_some(), self.dense_rank.is_some()) {
            (true, true) => "hybrid",
            (true, false) => "bm25",
            (false, true) => "dense",
            (false, false) => "unknown",
        }
    }
}

/// Format hybrid results for display.
pub fn format_hybrid_results(results: &[HybridResult], compact: bool) -> String {
    if results.is_empty() {
        return "No results found.".to_string();
    }

    let mut out = String::new();
    for (i, r) in results.iter().enumerate() {
        if compact {
            out.push_str(&format!(
                "{}. {:.4} [{}] {}:{}-{} {:?} {}\n",
                i + 1,
                r.rrf_score,
                r.source_label(),
                r.file_path,
                r.start_line,
                r.end_line,
                r.kind,
                r.symbol_name,
            ));
        } else {
            // Prefix the separator inside each arm so an absent rank-source
            // (BM25-only or dense-only ranking) renders a clean "(score: X)".
            // The score is RRF only for hybrid results; otherwise it is the
            // BM25 score or the cosine similarity.
            let source_info = match (r.bm25_rank, r.dense_rank) {
                (Some(bm), Some(dn)) => format!(", bm25:#{bm} + dense:#{dn}"),
                (Some(bm), None) => format!(", bm25:#{bm}"),
                (None, Some(dn)) => format!(", dense:#{dn}"),
                _ => String::new(),
            };
            out.push_str(&format!(
                "\n--- Result {} (score: {:.4}{}) ---\n{} :: {} [{:?}] (L{}-{})\n{}\n",
                i + 1,
                r.rrf_score,
                source_info,
                r.file_path,
                r.symbol_name,
                r.kind,
                r.start_line,
                r.end_line,
                r.snippet,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bm25_result(file: &str, name: &str, line: usize, score: f64) -> SearchResult {
        SearchResult {
            chunk_idx: 0,
            score,
            file_path: file.to_string(),
            symbol_name: name.to_string(),
            kind: ChunkKind::Function,
            start_line: line,
            end_line: line + 10,
            snippet: format!("fn {name}() {{ }}"),
        }
    }

    fn make_dense_result(file: &str, name: &str, line: usize, sim: f32) -> DenseSearchResult {
        DenseSearchResult {
            similarity: sim,
            file_path: file.to_string(),
            symbol_name: name.to_string(),
            kind: ChunkKind::Function,
            start_line: line,
            end_line: line + 10,
            snippet: format!("fn {name}() {{ }}"),
        }
    }

    #[test]
    fn rrf_basic_fusion() {
        let bm25 = vec![
            make_bm25_result("a.rs", "alpha", 1, 5.0),
            make_bm25_result("b.rs", "beta", 1, 3.0),
            make_bm25_result("c.rs", "gamma", 1, 1.0),
        ];
        let dense = vec![
            make_dense_result("b.rs", "beta", 1, 0.95),
            make_dense_result("d.rs", "delta", 1, 0.90),
            make_dense_result("a.rs", "alpha", 1, 0.85),
        ];

        let config = HybridConfig::default();
        let results = reciprocal_rank_fusion(&bm25, &dense, &config, 10);

        assert!(!results.is_empty());

        let top = &results[0];
        assert!(
            top.bm25_rank.is_some() || top.dense_rank.is_some(),
            "top result should appear in at least one ranking"
        );

        let beta = results.iter().find(|r| r.symbol_name == "beta").unwrap();
        assert!(beta.bm25_rank.is_some() && beta.dense_rank.is_some());
        assert_eq!(beta.source_label(), "hybrid");
    }

    #[test]
    fn rrf_both_rankings_boost() {
        let bm25 = vec![
            make_bm25_result("a.rs", "only_bm25", 1, 5.0),
            make_bm25_result("b.rs", "both", 1, 3.0),
        ];
        let dense = vec![
            make_dense_result("c.rs", "only_dense", 1, 0.99),
            make_dense_result("b.rs", "both", 1, 0.90),
        ];

        let config = HybridConfig {
            bm25_weight: 0.5,
            dense_weight: 0.5,
            ..Default::default()
        };
        let results = reciprocal_rank_fusion(&bm25, &dense, &config, 10);

        let both = results.iter().find(|r| r.symbol_name == "both").unwrap();
        let only_bm25 = results
            .iter()
            .find(|r| r.symbol_name == "only_bm25")
            .unwrap();
        let only_dense = results
            .iter()
            .find(|r| r.symbol_name == "only_dense")
            .unwrap();

        assert!(
            both.rrf_score > only_bm25.rrf_score,
            "result in both rankings should score higher than BM25-only"
        );
        assert!(
            both.rrf_score > only_dense.rrf_score,
            "result in both rankings should score higher than dense-only"
        );
    }

    #[test]
    fn rrf_respects_top_k() {
        let bm25: Vec<SearchResult> = (0..20)
            .map(|i| make_bm25_result("a.rs", &format!("fn_{i}"), i * 10 + 1, 10.0 - i as f64))
            .collect();

        let results = reciprocal_rank_fusion(&bm25, &[], &HybridConfig::default(), 5);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn rrf_empty_inputs() {
        let results = reciprocal_rank_fusion(&[], &[], &HybridConfig::default(), 10);
        assert!(results.is_empty());
    }

    #[test]
    fn rrf_bm25_only() {
        let bm25 = vec![make_bm25_result("a.rs", "alpha", 1, 5.0)];
        let results = reciprocal_rank_fusion(&bm25, &[], &HybridConfig::default(), 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_label(), "bm25");
    }

    #[test]
    fn rrf_dense_only() {
        let dense = vec![make_dense_result("a.rs", "alpha", 1, 0.95)];
        let results = reciprocal_rank_fusion(&[], &dense, &HybridConfig::default(), 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].source_label(), "dense");
    }

    #[test]
    fn format_compact() {
        let results = vec![HybridResult {
            file_path: "auth.rs".into(),
            symbol_name: "validate".into(),
            kind: ChunkKind::Function,
            start_line: 10,
            end_line: 20,
            snippet: "fn validate() {}".into(),
            rrf_score: 0.0156,
            dense_score: Some(0.91),
            bm25_rank: Some(1),
            dense_rank: Some(2),
        }];
        let output = format_hybrid_results(&results, true);
        assert!(output.contains("[hybrid]"));
        assert!(output.contains("auth.rs"));
        assert!(output.contains("validate"));
    }

    #[test]
    fn format_verbose() {
        let results = vec![HybridResult {
            file_path: "auth.rs".into(),
            symbol_name: "validate".into(),
            kind: ChunkKind::Function,
            start_line: 10,
            end_line: 20,
            snippet: "fn validate() {}".into(),
            rrf_score: 0.0156,
            dense_score: Some(0.91),
            bm25_rank: Some(1),
            dense_rank: Some(2),
        }];
        let output = format_hybrid_results(&results, false);
        assert!(output.contains("bm25:#1 + dense:#2"));
    }

    #[test]
    fn format_verbose_no_dangling_comma_when_source_absent() {
        // BM25-only fallback (embeddings off) leaves both ranks None; the header
        // must read "(score: X)" with no dangling separator.
        let results = vec![HybridResult {
            file_path: "auth.rs".into(),
            symbol_name: "validate".into(),
            kind: ChunkKind::Function,
            start_line: 10,
            end_line: 20,
            snippet: "fn validate() {}".into(),
            rrf_score: 0.5898,
            dense_score: None,
            bm25_rank: None,
            dense_rank: None,
        }];
        let output = format_hybrid_results(&results, false);
        assert!(
            output.contains("(score: 0.5898)"),
            "absent rank-source must render a clean header, got: {output}"
        );
        assert!(!output.contains(", )"), "must not emit a dangling ', )'");
    }

    #[test]
    fn source_label_categories() {
        let mut r = HybridResult {
            file_path: String::new(),
            symbol_name: String::new(),
            kind: ChunkKind::Function,
            start_line: 0,
            end_line: 0,
            snippet: String::new(),
            rrf_score: 0.0,
            dense_score: None,
            bm25_rank: None,
            dense_rank: None,
        };

        r.bm25_rank = Some(1);
        r.dense_rank = Some(1);
        assert_eq!(r.source_label(), "hybrid");

        r.dense_rank = None;
        assert_eq!(r.source_label(), "bm25");

        r.bm25_rank = None;
        r.dense_rank = Some(1);
        assert_eq!(r.source_label(), "dense");
    }
}
