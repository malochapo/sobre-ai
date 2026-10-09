//! Thresholds for `entropy` mode: a hand-set table per file extension,
//! nudged by how well the content gzips.

use std::path::Path;

use super::entropy::kolmogorov_proxy;

/// Lines whose BPE entropy is below `bpe_entropy` may be dropped; paragraphs
/// whose bigram Jaccard similarity reaches `jaccard` count as duplicates.
#[derive(Debug, Clone)]
pub struct CompressionThresholds {
    pub bpe_entropy: f64,
    pub jaccard: f64,
}

impl Default for CompressionThresholds {
    fn default() -> Self {
        Self {
            bpe_entropy: 1.0,
            jaccard: 0.7,
        }
    }
}

static LANGUAGE_THRESHOLDS: &[(&str, CompressionThresholds)] = &[
    // Python: English-like syntax, significant whitespace → higher entropy baseline
    (
        "py",
        CompressionThresholds {
            bpe_entropy: 1.2,
            jaccard: 0.65,
        },
    ),
    // Rust: Repetitive keywords (fn, pub, impl, let, mut) → lower threshold catches more
    (
        "rs",
        CompressionThresholds {
            bpe_entropy: 0.85,
            jaccard: 0.72,
        },
    ),
    // TypeScript/JavaScript: Type annotations are predictable
    (
        "ts",
        CompressionThresholds {
            bpe_entropy: 0.95,
            jaccard: 0.68,
        },
    ),
    (
        "tsx",
        CompressionThresholds {
            bpe_entropy: 0.95,
            jaccard: 0.68,
        },
    ),
    (
        "js",
        CompressionThresholds {
            bpe_entropy: 1.0,
            jaccard: 0.68,
        },
    ),
    (
        "jsx",
        CompressionThresholds {
            bpe_entropy: 1.0,
            jaccard: 0.68,
        },
    ),
    // Go: Verbose but highly structured → aggressive threshold
    (
        "go",
        CompressionThresholds {
            bpe_entropy: 0.9,
            jaccard: 0.72,
        },
    ),
    // Java/Kotlin: Very verbose, lots of boilerplate
    (
        "java",
        CompressionThresholds {
            bpe_entropy: 0.8,
            jaccard: 0.65,
        },
    ),
    (
        "kt",
        CompressionThresholds {
            bpe_entropy: 0.85,
            jaccard: 0.68,
        },
    ),
    // C/C++: Headers are highly repetitive
    (
        "c",
        CompressionThresholds {
            bpe_entropy: 0.9,
            jaccard: 0.7,
        },
    ),
    (
        "h",
        CompressionThresholds {
            bpe_entropy: 0.75,
            jaccard: 0.65,
        },
    ),
    (
        "cpp",
        CompressionThresholds {
            bpe_entropy: 0.9,
            jaccard: 0.7,
        },
    ),
    (
        "hpp",
        CompressionThresholds {
            bpe_entropy: 0.75,
            jaccard: 0.65,
        },
    ),
    // Ruby: English-like, high entropy
    (
        "rb",
        CompressionThresholds {
            bpe_entropy: 1.15,
            jaccard: 0.65,
        },
    ),
    // Config/data files: highly repetitive
    (
        "json",
        CompressionThresholds {
            bpe_entropy: 0.6,
            jaccard: 0.6,
        },
    ),
    (
        "yaml",
        CompressionThresholds {
            bpe_entropy: 0.7,
            jaccard: 0.62,
        },
    ),
    (
        "yml",
        CompressionThresholds {
            bpe_entropy: 0.7,
            jaccard: 0.62,
        },
    ),
    (
        "toml",
        CompressionThresholds {
            bpe_entropy: 0.7,
            jaccard: 0.62,
        },
    ),
    (
        "xml",
        CompressionThresholds {
            bpe_entropy: 0.6,
            jaccard: 0.6,
        },
    ),
    // Markdown/docs: natural language, high entropy
    (
        "md",
        CompressionThresholds {
            bpe_entropy: 1.3,
            jaccard: 0.6,
        },
    ),
    // CSS: very repetitive selectors/properties
    (
        "css",
        CompressionThresholds {
            bpe_entropy: 0.7,
            jaccard: 0.6,
        },
    ),
    (
        "scss",
        CompressionThresholds {
            bpe_entropy: 0.75,
            jaccard: 0.62,
        },
    ),
    // SQL: repetitive keywords
    (
        "sql",
        CompressionThresholds {
            bpe_entropy: 0.8,
            jaccard: 0.65,
        },
    ),
    // Shell scripts
    (
        "sh",
        CompressionThresholds {
            bpe_entropy: 1.0,
            jaccard: 0.68,
        },
    ),
    (
        "bash",
        CompressionThresholds {
            bpe_entropy: 1.0,
            jaccard: 0.68,
        },
    ),
    // Swift/C#
    (
        "swift",
        CompressionThresholds {
            bpe_entropy: 0.9,
            jaccard: 0.68,
        },
    ),
    (
        "cs",
        CompressionThresholds {
            bpe_entropy: 0.85,
            jaccard: 0.65,
        },
    ),
    // PHP
    (
        "php",
        CompressionThresholds {
            bpe_entropy: 0.95,
            jaccard: 0.68,
        },
    ),
];

/// The thresholds for the file's extension, or the defaults.
fn thresholds_for_path(path: &str) -> CompressionThresholds {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    LANGUAGE_THRESHOLDS
        .iter()
        .find(|(e, _)| *e == ext)
        .map_or_else(CompressionThresholds::default, |(_, t)| t.clone())
}

/// Per-language thresholds, adjusted for how compressible this content is.
/// A pure function of `path` and `content`, so the same file always
/// compresses the same way.
pub fn adaptive_thresholds(path: &str, content: &str) -> CompressionThresholds {
    let mut base = thresholds_for_path(path);
    if content.len() > 500 {
        let k = kolmogorov_proxy(content);
        let k_adjustment = (k - 0.45) * 0.5;
        base.bpe_entropy = (base.bpe_entropy + k_adjustment).clamp(0.4, 2.0);
        base.jaccard = (base.jaccard - k_adjustment * 0.3).clamp(0.5, 0.85);
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_has_lower_threshold_than_python() {
        let rs = thresholds_for_path("src/main.rs");
        let py = thresholds_for_path("src/main.py");
        assert!(rs.bpe_entropy < py.bpe_entropy);
    }

    #[test]
    fn json_has_lowest_threshold() {
        let json = thresholds_for_path("config.json");
        let rs = thresholds_for_path("main.rs");
        assert!(json.bpe_entropy < rs.bpe_entropy);
    }

    #[test]
    fn unknown_ext_uses_default() {
        let t = thresholds_for_path("file.xyz");
        assert!((t.bpe_entropy - 1.0).abs() < f64::EPSILON);
    }
}
