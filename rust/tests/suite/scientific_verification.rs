//! Properties of the scoring functions behind compression: the Kolmogorov
//! proxy and the safeguard that refuses over-compression.

use sobre::core::entropy::kolmogorov_proxy;

// ═══════════════════════════════════════════════════════════════════
// 1. KOLMOGOROV PROXY — mathematical properties
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
// 2. SAFEGUARD RATIO — rate-distortion boundary
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
