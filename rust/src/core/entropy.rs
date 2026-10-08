use std::collections::{HashMap, HashSet};
use std::io::Write;

use flate2::Compression;
use flate2::write::GzEncoder;

use super::tokens::{count_tokens, encode_tokens};

const BPE_ENTROPY_THRESHOLD: f64 = 1.0;

/// Result of entropy-based compression: output text, token counts, and techniques used.
#[derive(Debug)]
pub struct EntropyResult {
    pub output: String,
    pub original_tokens: usize,
    pub techniques: Vec<String>,
}

impl EntropyResult {}

/// Shannon entropy over already-encoded BPE token IDs (o200k_base).
pub fn token_entropy_from_ids(tokens: &[u32]) -> f64 {
    if tokens.is_empty() {
        return 0.0;
    }
    let total = tokens.len();
    let mut freq: HashMap<u32, usize> = HashMap::new();
    for &t in tokens {
        *freq.entry(t).or_default() += 1;
    }
    // Sum in a canonical (sorted-by-count) order: f64 addition is not
    // associative, so folding over HashMap iteration order — random per instance
    // — would make the entropy, and thus `density`/`entropy` mode output,
    // non-deterministic across processes and defeat prompt caching.
    let mut counts: Vec<usize> = freq.into_values().collect();
    counts.sort_unstable();
    counts.iter().fold(0.0_f64, |acc, &count| {
        let p = count as f64 / total as f64;
        acc - p * p.log2()
    })
}

/// Shannon entropy over BPE token IDs (o200k_base).
/// More LLM-relevant than character entropy since LLMs process BPE tokens.
pub fn token_entropy(text: &str) -> f64 {
    let tokens = encode_tokens(text);
    token_entropy_from_ids(&tokens)
}

/// Normalized Shannon entropy over encoded token IDs: H(X) / log₂(n), n = unique token count.
pub fn normalized_token_entropy_from_ids(tokens: &[u32]) -> f64 {
    if tokens.is_empty() {
        return 0.0;
    }
    let total = tokens.len();
    let mut freq: HashMap<u32, usize> = HashMap::new();
    for &t in tokens {
        *freq.entry(t).or_default() += 1;
    }
    let n_unique = freq.len();
    if n_unique <= 1 {
        return 0.0;
    }
    // Canonical summation order for determinism; see `token_entropy_from_ids`.
    let mut counts: Vec<usize> = freq.into_values().collect();
    counts.sort_unstable();
    let h = counts.iter().fold(0.0_f64, |acc, &count| {
        let p = count as f64 / total as f64;
        acc - p * p.log2()
    });
    let h_max = (n_unique as f64).log2();
    h / h_max
}

fn ngram_set(text: &str, n: usize) -> HashSet<Vec<String>> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < n {
        let mut set = HashSet::new();
        if !words.is_empty() {
            set.insert(words.iter().map(std::string::ToString::to_string).collect());
        }
        return set;
    }
    words
        .windows(n)
        .map(|w| w.iter().map(std::string::ToString::to_string).collect())
        .collect()
}

/// Kolmogorov complexity proxy: K(x) ≈ len(gzip(x)) / len(x).
/// Lower values = more compressible = more redundant.
pub fn kolmogorov_proxy(content: &str) -> f64 {
    if content.is_empty() {
        return 1.0;
    }
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(content.as_bytes()).ok();
    let compressed = encoder.finish().unwrap_or_default();
    compressed.len() as f64 / content.len() as f64
}

/// Compresses content by removing low-entropy lines and deduplicating patterns.
#[cfg(test)]
pub fn entropy_compress(content: &str) -> EntropyResult {
    entropy_compress_with_thresholds(content, BPE_ENTROPY_THRESHOLD, 0.7, &[])
}

/// Entropy compression with the opportunistic semantic redundancy filter
/// pinned OFF. The regular path uses the shared embedding engine
/// whenever it happens to be loaded, so its output depends on runtime state;
/// the property tests go through this entry point to stay reproducible.
#[cfg(test)]
pub fn entropy_compress_deterministic(content: &str) -> EntropyResult {
    entropy_compress_inner(content, BPE_ENTROPY_THRESHOLD, 0.7, &[], false, &[])
}

/// Entropy compression with file-type-adaptive thresholds and event emission.
/// `force_keep` lines (explicit `protect` tokens) survive verbatim; pass
/// `&[]` to keep the pre-protect behaviour byte-identical.
pub fn entropy_compress_adaptive(
    content: &str,
    path: &str,
    force_keep: &[String],
) -> EntropyResult {
    let thresholds = super::adaptive_thresholds::adaptive_thresholds(path, content);

    entropy_compress_with_thresholds(
        content,
        thresholds.bpe_entropy,
        thresholds.jaccard,
        force_keep,
    )
}

/// Like [`entropy_compress_adaptive`] but overrides the learned BPE-entropy
/// threshold (e.g. from the aggressiveness knob) while keeping the file-adaptive
/// jaccard. Pure function of its inputs. Higher `bpe_entropy` drops more
/// low-information lines.
pub fn entropy_compress_with_threshold(
    content: &str,
    path: &str,
    bpe_entropy: f64,
    force_keep: &[String],
) -> EntropyResult {
    let thresholds = super::adaptive_thresholds::adaptive_thresholds(path, content);
    entropy_compress_with_thresholds(content, bpe_entropy, thresholds.jaccard, force_keep)
}

/// Task-conditioned entropy compression: lines that would normally be dropped
/// for low entropy are kept if they contain task-relevant keywords.  This is
/// the Information Bottleneck proxy: we compress away only what is neither
/// surprising (high H) *nor* task-relevant (mentions goal concepts).
/// Falls back to pure entropy when `task_keywords` is empty.
pub fn entropy_compress_task_conditioned(
    content: &str,
    path: &str,
    task_keywords: &[String],
    force_keep: &[String],
) -> EntropyResult {
    let thresholds = super::adaptive_thresholds::adaptive_thresholds(path, content);

    entropy_compress_with_task(
        content,
        thresholds.bpe_entropy,
        thresholds.jaccard,
        task_keywords,
        force_keep,
    )
}

/// Real line embedder for the semantic redundancy filter: uses the
/// shared neural engine only when it is ALREADY loaded (never blocks a read
/// on a model load) and only for files small enough that per-line embedding
/// stays within the read-path latency budget. Embeddings are cached by line
/// hash so re-reads cost nothing.
#[cfg(feature = "dense")]
fn line_embedder(line_count: usize) -> impl Fn(&str) -> Option<Vec<f32>> {
    use std::collections::HashMap;
    use std::sync::Mutex;

    const MAX_LINES_FOR_SEMANTIC: usize = 400;
    const CACHE_CAP: usize = 8192;
    static LINE_EMBED_CACHE: Mutex<Option<HashMap<u64, Vec<f32>>>> = Mutex::new(None);

    let engine = if line_count <= MAX_LINES_FOR_SEMANTIC {
        crate::tools::sobre_knowledge::embeddings::embedding_engine_nonblocking()
    } else {
        None
    };

    move |line: &str| {
        let engine = engine?;
        if line.len() < 8 {
            return None;
        }
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&line, &mut hasher);
        let key = std::hash::Hasher::finish(&hasher);

        if let Ok(mut guard) = LINE_EMBED_CACHE.lock()
            && let Some(hit) = guard.get_or_insert_with(HashMap::new).get(&key)
        {
            return Some(hit.clone());
        }
        let emb = engine.embed(line).ok()?;
        if let Ok(mut guard) = LINE_EMBED_CACHE.lock() {
            let map = guard.get_or_insert_with(HashMap::new);
            if map.len() >= CACHE_CAP {
                map.clear();
            }
            map.insert(key, emb.clone());
        }
        Some(emb)
    }
}

#[cfg(not(feature = "dense"))]
fn line_embedder(_line_count: usize) -> impl Fn(&str) -> Option<Vec<f32>> {
    |_: &str| None
}

fn entropy_compress_with_task(
    content: &str,
    entropy_threshold: f64,
    jaccard_threshold: f64,
    task_keywords: &[String],
    force_keep: &[String],
) -> EntropyResult {
    entropy_compress_inner(
        content,
        entropy_threshold,
        jaccard_threshold,
        task_keywords,
        true,
        force_keep,
    )
}

fn entropy_compress_inner(
    content: &str,
    entropy_threshold: f64,
    jaccard_threshold: f64,
    task_keywords: &[String],
    semantic: bool,
    force_keep: &[String],
) -> EntropyResult {
    let original_tokens = count_tokens(content);
    let mut lines: Vec<&str> = content.lines().collect();
    let mut techniques = Vec::new();

    let kw_lower: Vec<String> = task_keywords.iter().map(|k| k.to_lowercase()).collect();
    let original_count = lines.len();
    let mut task_rescued = 0usize;
    // Semantic redundancy: when the embedding engine is already
    // loaded, kept lines that embed near-identically to earlier kept lines
    // are dropped (MMR). Without the engine the closure returns None and the
    // decision path is byte-identical to the Zipf-only filter.
    // `semantic=false` (deterministic contract) requests an embedder above
    // the size cutoff, which never resolves an engine.
    let embed = line_embedder(if semantic { original_count } else { usize::MAX });
    let mut scoring_ctx = super::surprise::ScoringCtx::new();
    lines.retain(|line| {
        let trimmed = line.trim();
        // Explicit protect tokens win over every lossy heuristic: a line
        // containing one is force-kept verbatim before any threshold is applied.
        if super::protect::line_is_protected(line, force_keep) {
            return true;
        }
        if super::surprise::should_keep_line_semantic(
            trimmed,
            entropy_threshold,
            &embed,
            &mut scoring_ctx,
        ) {
            return true;
        }
        // Task-conditioned rescue: keep low-entropy lines that mention task keywords.
        if !kw_lower.is_empty() {
            let lower = trimmed.to_lowercase();
            if kw_lower.iter().any(|kw| lower.contains(kw.as_str())) {
                task_rescued += 1;
                return true;
            }
        }
        false
    });
    let removed = original_count - lines.len();
    if removed > 0 || task_rescued > 0 {
        let mut msg = format!("⊘ {removed} low-entropy lines (BPE H<{entropy_threshold:.2})");
        if task_rescued > 0 {
            msg.push_str(&format!(" [+{task_rescued} task-rescued]"));
        }
        techniques.push(msg);
    }

    let blocks = extract_blocks(&lines);
    let groups = find_pattern_groups(&blocks, jaccard_threshold);
    let mut dedup_count = 0;
    for group in &groups {
        if group.len() > 1 {
            dedup_count += group.len() - 1;
        }
    }
    if dedup_count > 0 {
        techniques.push(format!("⊘ {dedup_count} duplicate patterns (J≥0.7)"));
    }

    let mut result: Vec<String> = Vec::new();
    let mut skip_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for group in &groups {
        if group.len() > 1 {
            for &idx in &group[1..] {
                // Protected lines are a hard keep: never dedup them away,
                // not just exempt them from the entropy threshold above.
                if !super::protect::line_is_protected(lines[idx], force_keep) {
                    skip_indices.insert(idx);
                }
            }
        }
    }
    for (i, line) in lines.iter().enumerate() {
        if !skip_indices.contains(&i) {
            result.push(line.to_string());
        }
    }

    let mut collapsed = Vec::new();
    let mut blank_count = 0;
    for line in &result {
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                collapsed.push(line.clone());
            }
        } else {
            blank_count = 0;
            collapsed.push(line.clone());
        }
    }
    let output = collapsed.join("\n");
    let compressed_tokens = count_tokens(&output);

    // Safeguard: BPE re-tokenization of a line subset can, for tiny adversarial
    // inputs, exceed the original token count (e.g. a dropped trailing newline
    // merges differently). Never inflate — fall back to the original verbatim.
    let final_output = if compressed_tokens > original_tokens {
        content.to_string()
    } else {
        output
    };

    EntropyResult {
        output: final_output,
        original_tokens,
        techniques,
    }
}

fn entropy_compress_with_thresholds(
    content: &str,
    entropy_threshold: f64,
    jaccard_threshold: f64,
    force_keep: &[String],
) -> EntropyResult {
    entropy_compress_with_task(
        content,
        entropy_threshold,
        jaccard_threshold,
        &[],
        force_keep,
    )
}

/// Walk kept lines in source order; when a kept line has unclosed
/// call/index delimiters `()` `[]`, force-keep subsequent lines until
/// balance is restored. Prevents partial call expressions like
/// `logger.info(` surviving without their arguments.
///
/// Intentionally ignores `{}` — block braces wrap large code bodies and
/// force-keeping entire function blocks would defeat the density target.
fn delimiter_balance_force_keep(
    lines: &[&str],
    scored: &[(usize, f64, usize)],
    keep: &mut [bool],
    used: &mut usize,
    kept_count: &mut usize,
) {
    let tok_for =
        |idx: usize| -> usize { scored.iter().find(|(i, _, _)| *i == idx).map_or(1, |s| s.2) };

    // Phase 1: identify multi-line delimiter spans (opener_line, closer_line).
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut open_stack: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        for ch in line.chars() {
            match ch {
                '(' | '[' => open_stack.push(i),
                ')' | ']' => {
                    if let Some(opener) = open_stack.pop()
                        && opener != i
                    {
                        spans.push((opener, i));
                    }
                }
                _ => {}
            }
        }
    }

    // Phase 2: if ANY line in a span is kept, force-keep all lines in it.
    // This handles both directions: opener->continuation and continuation->opener.
    for &(start, end) in &spans {
        let any_kept = (start..=end).any(|i| keep[i]);
        if any_kept {
            for (i, kept) in keep
                .iter_mut()
                .enumerate()
                .skip(start)
                .take(end - start + 1)
            {
                if !*kept {
                    *kept = true;
                    *used += tok_for(i);
                    *kept_count += 1;
                }
            }
        }
    }
}

/// Budget-based compression to a target density (SDE principle: aim for a
/// *target* information density instead of maximum compression).
///
/// Keeps the highest-entropy lines — in original order — until the BPE token
/// budget `original_tokens * target` is exhausted. Deterministic: ties break
/// on line index. `target` is clamped to [0.05, 1.0]; the top-scored line is
/// always kept so output is never empty for non-empty input.
pub fn entropy_compress_to_density(content: &str, target: f64) -> EntropyResult {
    let target = target.clamp(0.05, 1.0);
    let original_tokens = count_tokens(content);
    if content.is_empty() || original_tokens == 0 {
        return EntropyResult {
            output: String::new(),
            original_tokens,
            techniques: vec![format!("density target={target:.2} (empty input)")],
        };
    }

    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let budget = ((original_tokens as f64) * target).ceil() as usize;

    let lines: Vec<&str> = content.lines().collect();
    let mut scored: Vec<(usize, f64, usize)> = lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let trimmed = l.trim();
            // +1 approximates the newline token; keeps the per-line sum close
            // to the whole-file count so the budget is meaningful.
            let toks = count_tokens(trimmed).max(1);
            (i, token_entropy(trimmed), toks)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });

    let mut keep = vec![false; lines.len()];
    let mut used = 0usize;
    let mut kept_count = 0usize;
    // The single most informative line is always kept, even if it alone
    // exceeds the budget — a density target that drops the highest-signal
    // line would be self-defeating. Everything else fills greedily.
    if let Some(&(idx, _, toks)) = scored.first() {
        keep[idx] = true;
        used += toks;
        kept_count += 1;
    }
    for &(idx, _h, toks) in scored.iter().skip(1) {
        if used + toks > budget {
            continue;
        }
        keep[idx] = true;
        used += toks;
        kept_count += 1;
    }

    // Delimiter Balance Guard: when a kept line opens unclosed
    // delimiters ( [ {, force-keep subsequent lines until the balance
    // is restored. This prevents `logger.info(` from being kept while
    // its arguments are dropped, leaving syntactically broken output.
    delimiter_balance_force_keep(&lines, &scored, &mut keep, &mut used, &mut kept_count);

    let mut out_lines: Vec<&str> = Vec::with_capacity(kept_count);
    for (i, line) in lines.iter().enumerate() {
        if keep[i] {
            out_lines.push(line);
        }
    }
    let output = out_lines.join("\n");

    let dropped = lines.len() - kept_count;
    EntropyResult {
        output,
        original_tokens,
        techniques: vec![format!(
            "density target={target:.2} budget={budget} tok, kept {kept_count}/{} lines (⊘ {dropped})",
            lines.len()
        )],
    }
}

/// Per-line entropy statistics for a block of content.
#[derive(Debug)]
pub struct EntropyAnalysis {
    pub avg_entropy: f64,
    pub low_entropy_count: usize,
    pub high_entropy_count: usize,
    pub total_lines: usize,
}

/// Analyzes per-line BPE token entropy, counting low/high entropy lines.
pub fn analyze_entropy(content: &str) -> EntropyAnalysis {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    let mut sum = 0.0;
    let mut low = 0;
    let mut high = 0;
    let mut counted = 0;

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let h = token_entropy(trimmed);
        sum += h;
        counted += 1;
        if h < BPE_ENTROPY_THRESHOLD {
            low += 1;
        }
        if h > 3.0 {
            high += 1;
        }
    }

    EntropyAnalysis {
        avg_entropy: if counted > 0 {
            sum / counted as f64
        } else {
            0.0
        },
        low_entropy_count: low,
        high_entropy_count: high,
        total_lines: total,
    }
}

struct Block {
    content: String,
}

fn extract_blocks(lines: &[&str]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current = String::new();

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() && !current.is_empty() {
            blocks.push(Block {
                content: current.clone(),
            });
            current.clear();
        } else if !trimmed.is_empty() {
            current.push_str(trimmed);
            current.push('\n');
        }
    }

    if !current.is_empty() {
        blocks.push(Block { content: current });
    }

    blocks
}

fn find_pattern_groups(blocks: &[Block], threshold: f64) -> Vec<Vec<usize>> {
    // Exact n-gram Jaccard, but with precomputed n-gram sets per block to avoid
    // rebuilding allocations per pair. Includes a size-ratio impossibility check
    // (max possible Jaccard is |A|/|B| for |A|<=|B|).
    let sets: Vec<HashSet<Vec<String>>> = blocks.iter().map(|b| ngram_set(&b.content, 2)).collect();
    let sizes: Vec<usize> = sets.iter().map(std::collections::HashSet::len).collect();

    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut assigned: HashSet<usize> = HashSet::new();

    for i in 0..blocks.len() {
        if assigned.contains(&i) {
            continue;
        }
        let mut group = vec![i];
        for j in (i + 1)..blocks.len() {
            if assigned.contains(&j) {
                continue;
            }
            let size_i = sizes[i];
            let size_j = sizes[j];
            let min_sz = size_i.min(size_j);
            let max_sz = size_i.max(size_j);
            if max_sz > 0 && (min_sz as f64) < (threshold * max_sz as f64) {
                continue;
            }
            let inter = sets[i].intersection(&sets[j]).count();
            let union = size_i + size_j - inter;
            if union > 0 && (inter as f64 / union as f64) >= threshold {
                group.push(j);
                assigned.insert(j);
            }
        }
        if group.len() > 1 {
            assigned.insert(i);
        }
        groups.push(group);
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_force_keeps_lines_in_entropy() {
        // A block of identical low-information lines that entropy + dedup would
        // normally collapse to a single survivor (or drop entirely).
        let mut content = String::new();
        for _ in 0..15 {
            content.push_str("boilerplate noise line\n");
        }
        let baseline =
            entropy_compress_inner(&content, BPE_ENTROPY_THRESHOLD, 0.7, &[], false, &[]);
        let protected = entropy_compress_inner(
            &content,
            BPE_ENTROPY_THRESHOLD,
            0.7,
            &[],
            false,
            &["boilerplate".to_string()],
        );
        let baseline_hits = baseline.output.matches("boilerplate").count();
        let protected_hits = protected.output.matches("boilerplate").count();
        // Every protected line survives verbatim — a hard keep over both the
        // entropy threshold and the dedup pass.
        assert_eq!(
            protected_hits, 15,
            "all protected lines must survive: {}",
            protected.output
        );
        assert!(
            protected_hits >= baseline_hits,
            "protect must keep at least as many lines as the baseline"
        );
    }

    #[test]
    fn empty_force_keep_is_byte_identical() {
        // The protect feature must not perturb the unprotected path.
        let content = "fn a() {}\n// note\nlet x = 1;\nlet x = 1;\n// note\n";
        let a = entropy_compress_inner(content, BPE_ENTROPY_THRESHOLD, 0.7, &[], false, &[]);
        let b = entropy_compress_deterministic(content);
        assert_eq!(a.output, b.output);
    }

    #[test]
    fn entropy_compress_produces_output() {
        let content = "fn main() {\n    println!(\"hello\");\n}\n\n// comment\n// another comment\n\nfn helper() {\n    let x = 42;\n}\n";
        let result = entropy_compress(content);
        assert!(!result.output.is_empty(), "should produce non-empty output");
        assert!(count_tokens(&result.output) <= result.original_tokens);
    }

    #[test]
    fn token_entropy_empty_is_zero() {
        assert_eq!(token_entropy(""), 0.0);
    }

    #[test]
    fn token_entropy_single_repeated_token() {
        assert_eq!(token_entropy("}"), 0.0);
    }

    #[test]
    fn token_entropy_higher_for_diverse_code() {
        let diverse = "let result = compute_something(x, y, z);";
        let repetitive = "aaaa aaaa aaaa aaaa";
        assert!(
            token_entropy(diverse) > token_entropy(repetitive),
            "diverse code should have higher BPE token entropy"
        );
    }

    #[test]
    fn kolmogorov_empty_is_one() {
        assert_eq!(kolmogorov_proxy(""), 1.0);
    }

    #[test]
    fn kolmogorov_repetitive_is_low() {
        let repetitive = "aaa\n".repeat(1000);
        let k = kolmogorov_proxy(&repetitive);
        assert!(
            k < 0.1,
            "highly repetitive text should compress well: K={k}"
        );
    }

    #[test]
    fn kolmogorov_diverse_is_higher() {
        let repetitive = "aaa\n".repeat(500);
        let diverse = (0..500)
            .map(|i| format!("line_{i}_unique_content_{}", i * 17 % 97))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            kolmogorov_proxy(&diverse) > kolmogorov_proxy(&repetitive),
            "diverse content should have higher K than repetitive"
        );
    }

    fn density_fixture() -> String {
        (0..120)
            .map(|i| {
                if i % 3 == 0 {
                    format!(
                        "fn compute_value_{i}(input: &str, flags: u32) -> Result<Output, Error> {{"
                    )
                } else if i % 3 == 1 {
                    format!("    let intermediate_{i} = transform(input, flags ^ {i});")
                } else {
                    "}".to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn density_respects_token_budget() {
        let content = density_fixture();
        let orig = count_tokens(&content);
        for target in [0.3, 0.5, 0.7] {
            let r = entropy_compress_to_density(&content, target);
            let actual = count_tokens(&r.output) as f64 / orig as f64;
            // Tolerance of 0.15: the delimiter balance guard may
            // force-keep call/index continuation lines beyond the budget.
            assert!(
                actual <= target + 0.15,
                "target {target}: actual density {actual:.2} exceeds budget"
            );
            assert!(!r.output.is_empty());
        }
    }

    #[test]
    fn density_is_deterministic() {
        let content = density_fixture();
        let a = entropy_compress_to_density(&content, 0.4);
        let b = entropy_compress_to_density(&content, 0.4);
        assert_eq!(a.output, b.output);
    }

    #[test]
    fn density_target_one_keeps_everything() {
        let content = density_fixture();
        let r = entropy_compress_to_density(&content, 1.0);
        assert_eq!(r.output, content);
    }

    #[test]
    fn density_clamps_out_of_range_target() {
        let content = density_fixture();
        let low = entropy_compress_to_density(&content, 0.0);
        assert!(!low.output.is_empty(), "clamped to 0.05, never empty");
        let high = entropy_compress_to_density(&content, 5.0);
        assert_eq!(high.output, content, "clamped to 1.0 keeps all");
    }

    #[test]
    fn density_empty_input() {
        let r = entropy_compress_to_density("", 0.5);
        assert!(r.output.is_empty());
    }

    #[test]
    fn density_delimiter_balance_guard() {
        // When `logger.info(` is kept, its continuation lines must
        // also survive so the output is syntactically valid.
        let content = [
            "import logging",
            "logger = logging.getLogger(__name__)",
            "logger.info(",
            "    'Processing batch %d of %d',",
            "    batch_idx,",
            "    total_batches,",
            ")",
            "result = process(data)",
        ]
        .join("\n");
        let r = entropy_compress_to_density(&content, 0.5);
        // If `logger.info(` is kept, all lines through `)` must be too.
        if r.output.contains("logger.info(") {
            assert!(
                r.output.contains(')'),
                "delimiter guard must keep closing paren: {}",
                r.output
            );
            assert!(
                r.output.contains("batch_idx"),
                "delimiter guard must keep continuation args: {}",
                r.output
            );
        }
    }

    #[test]
    fn density_delimiter_backward_propagation() {
        // Regression: when a high-entropy continuation line inside a
        // multi-line call is kept by the entropy scorer, the opener line
        // must also be force-kept (backward propagation through the span).
        //
        // Uses many low-entropy padding lines to force the budget to drop the
        // opener while the high-entropy format string survives on its own merit.
        let mut lines: Vec<&str> = Vec::new();
        lines.push("import logging");
        lines.push("logger = logging.getLogger(__name__)");
        lines.push("");
        // Low-entropy padding to push density budget down
        lines.extend(std::iter::repeat_n("    x = 1", 20));
        lines.push("    logger.info(");
        lines.push("        \"Processed item %s status=%s result=%s region=%s\",");
        lines.push("        item.id,");
        lines.push("        item.status,");
        lines.push("        result.summary,");
        lines.push("        region_name");
        lines.push("    )");
        // More padding
        lines.extend(std::iter::repeat_n("    y = 2", 10));
        let content = lines.join("\n");
        let r = entropy_compress_to_density(&content, 0.3);
        // The format string has high entropy and will be kept.
        // The span guard must then force-keep the opener + closer.
        if r.output.contains("Processed item") {
            assert!(
                r.output.contains("logger.info("),
                "backward propagation: opener must be kept when format string is kept.\nGot: {}",
                r.output
            );
            assert!(
                r.output.contains(')'),
                "backward propagation: closer must also be kept.\nGot: {}",
                r.output
            );
        }
        // Also verify: if logger.info( is kept, all args through ) are kept
        if r.output.contains("logger.info(") {
            assert!(
                r.output.contains("item.id"),
                "forward propagation: args must be kept when opener is kept.\nGot: {}",
                r.output
            );
        }
    }

    #[test]
    fn density_prefers_high_entropy_lines() {
        let content =
            "}\n}\n}\nlet complex_result = compute_unique_hash(seed, nonce, payload);\n}\n}";
        let r = entropy_compress_to_density(content, 0.6);
        assert!(
            r.output.contains("compute_unique_hash"),
            "high-entropy line must survive: {}",
            r.output
        );
    }

    // Property/fuzz tests go through the deterministic entry point: the
    // semantic-redundancy filter would otherwise depend on whether the
    // embedding engine happens to be loaded.
    proptest::proptest! {
        #[test]
        fn output_is_subset_of_lines(content in "[a-zA-Z0-9(){};= \n]{10,800}") {
            let result = entropy_compress_deterministic(&content);
            for line in result.output.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                proptest::prop_assert!(
                    content.contains(line.trim()),
                    "entropy output line not found in original: {:?}",
                    line
                );
            }
        }

        #[test]
        fn tokens_not_greater(content in "[a-zA-Z0-9(){};= \n]{10,600}") {
            let result = entropy_compress_deterministic(&content);
            let compressed = count_tokens(&result.output);
            proptest::prop_assert!(
                compressed <= result.original_tokens,
                "compressed {} > original {}",
                compressed,
                result.original_tokens
            );
        }

        #[test]
        fn no_panic_on_arbitrary_utf8(content in "\\PC{0,1000}") {
            let _ = entropy_compress_deterministic(&content);
        }
    }
}
