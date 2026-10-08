use sobre::core::entropy::kolmogorov_proxy;

// ═══════════════════════════════════════════════════════════════════
// 1. SHANNON ENTROPY — mathematical invariants
// ═══════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════
// 2. NORMALIZED ENTROPY — must be in [0, 1]
// ═══════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════
// 3. TOKEN ENTROPY vs CHARACTER ENTROPY — relationship
// ═══════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════
// 4. KOLMOGOROV PROXY — mathematical properties
// ═══════════════════════════════════════════════════════════════════

#[test]
fn kolmogorov_bounds() {
    let text = "hello world, this is a test of Kolmogorov complexity estimation";
    let k = kolmogorov_proxy(text);
    assert!(k > 0.0, "K(x) must be positive for non-empty");
    assert!(
        k <= 2.0,
        "K(x) = gzip/raw should be ≤ ~2.0 (gzip overhead for short strings)"
    );
}

#[test]
fn kolmogorov_monotonic_with_redundancy() {
    let redundant = "abcabc".repeat(100);
    let random_like: String = (0..600)
        .map(|i| char::from(b'a' + (((i * 7 + 13) % 26) as u8)))
        .collect();
    let k_red = kolmogorov_proxy(&redundant);
    let k_rand = kolmogorov_proxy(&random_like);
    assert!(
        k_red < k_rand,
        "redundant text should have lower K: {k_red:.3} vs {k_rand:.3}"
    );
}

#[test]
fn kolmogorov_invariant_under_repetition() {
    let base = "fn process(data: &[u8]) -> Result<Output, Error> { Ok(Output::default()) }\n";
    let k1 = kolmogorov_proxy(base);
    let repeated = base.repeat(50);
    let k50 = kolmogorov_proxy(&repeated);
    assert!(
        k50 < k1,
        "repeating content should decrease K: K(1)={k1:.3}, K(50)={k50:.3}"
    );
}

// ═══════════════════════════════════════════════════════════════════
// 5. JACCARD SIMILARITY — metric space axioms
// ═══════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════
// 7. COMBINED ATTENTION — geometric mean properties
// ═══════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════
// 8. ATTENTION EFFICIENCY — percentage bounds
// ═══════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════
// 9. SYMBOL MAP ROI — break-even analysis
// ═══════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════
// 10. INFORMATION BOTTLENECK — task relevance filtering
// ═══════════════════════════════════════════════════════════════════

#[test]
fn ib_filter_preserves_task_relevant_lines() {
    use sobre::core::task_relevance::information_bottleneck_filter;

    let mut lines = Vec::new();
    for i in 0..100 {
        if i == 10 || i == 50 {
            lines.push(format!(
                "pub fn validate_token(t: &str) -> bool {{ /* line {i} */ }}"
            ));
        } else {
            lines.push(format!("let unrelated_{i} = compute_{i}(x);"));
        }
    }
    let content = lines.join("\n");
    let result = information_bottleneck_filter(&content, &["validate_token".to_string()], 0.3, &[]);

    assert!(
        result.contains("validate_token"),
        "IB filter must preserve task-relevant lines"
    );
    let result_lines = result.lines().count();
    assert!(
        result_lines < 100,
        "IB filter should reduce lines: {result_lines} < 100"
    );
}

#[test]
fn ib_filter_reduces_more_for_repetitive_content() {
    use sobre::core::task_relevance::{adaptive_ib_budget, information_bottleneck_filter};

    let repetitive = "let x = compute(a);\n".repeat(100);
    let diverse = (0..100).fold(String::new(), |mut s, i| {
        use std::fmt::Write;
        let _ = writeln!(s, "let var_{i} = func_{i}(arg_{i});");
        s
    });

    let budget_rep = adaptive_ib_budget(&repetitive, 0.5);
    let budget_div = adaptive_ib_budget(&diverse, 0.5);

    assert!(
        budget_rep < budget_div,
        "repetitive content should get lower IB budget: {budget_rep:.3} < {budget_div:.3}"
    );

    let kw = vec!["compute".to_string()];
    let filtered_rep = information_bottleneck_filter(&repetitive, &kw, 0.3, &[]);
    let filtered_div = information_bottleneck_filter(&diverse, &kw, 0.3, &[]);

    eprintln!(
        "[IB adaptive] repetitive: {}→{} lines, diverse: {}→{} lines",
        100,
        filtered_rep.lines().count(),
        100,
        filtered_div.lines().count()
    );
}

// ═══════════════════════════════════════════════════════════════════
// 11. SAFEGUARD RATIO — rate-distortion boundary
// ═══════════════════════════════════════════════════════════════════

#[test]
fn safeguard_prevents_over_compression() {
    use sobre::core::compressor::safeguard_ratio;
    let original = "fn main() {\n".repeat(50);
    let over_compressed = "x";
    let result = safeguard_ratio(&original, over_compressed);
    assert_eq!(
        result, original,
        "safeguard must return original when ratio < 0.05 on small output"
    );
}

#[test]
fn safeguard_allows_good_compression() {
    use sobre::core::compressor::safeguard_ratio;
    let original = "fn main() {\n    let x = compute();\n    println!(x);\n}\n".repeat(10);
    let compressed = "fn main() { let x = compute(); println!(x); }\n".repeat(10);
    let result = safeguard_ratio(&original, &compressed);
    assert_eq!(
        result, compressed,
        "safeguard must allow reasonable compression"
    );
}

// ═══════════════════════════════════════════════════════════════════
// 12. COST MODEL — economic sanity checks
// ═══════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════
// 13. INTEGRATED SCIENTIFIC AUDIT
// ═══════════════════════════════════════════════════════════════════
