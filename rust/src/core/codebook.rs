use std::collections::HashMap;

/// Identify semantically duplicate blocks across files.
/// IDF is computed over the full file corpus for accurate weighting.
pub fn find_semantic_duplicates(
    files: &[(String, String)],
    threshold: f64,
) -> Vec<(String, String, f64)> {
    let corpus: Vec<&str> = files.iter().map(|(_, c)| c.as_str()).collect();
    let idf = compute_idf(&corpus);
    let vectors: Vec<HashMap<&str, f64>> =
        files.iter().map(|(_, c)| tfidf_vector(c, &idf)).collect();

    let mut duplicates = Vec::new();

    for i in 0..files.len() {
        for j in (i + 1)..files.len() {
            let sim = cosine_from_vectors(&vectors[i], &vectors[j]);
            if sim >= threshold {
                duplicates.push((files[i].0.clone(), files[j].0.clone(), sim));
            }
        }
    }

    duplicates.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    duplicates
}

fn compute_idf<'a>(corpus: &[&'a str]) -> HashMap<&'a str, f64> {
    let n = corpus.len() as f64;
    if n == 0.0 {
        return HashMap::new();
    }

    let mut doc_freq: HashMap<&str, usize> = HashMap::new();
    for doc in corpus {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for word in doc.split_whitespace() {
            if seen.insert(word) {
                *doc_freq.entry(word).or_insert(0) += 1;
            }
        }
    }

    doc_freq
        .into_iter()
        .map(|(term, df)| (term, (n / (1.0 + df as f64)).ln() + 1.0))
        .collect()
}

fn tfidf_vector<'a>(doc: &'a str, idf: &HashMap<&str, f64>) -> HashMap<&'a str, f64> {
    let words: Vec<&str> = doc.split_whitespace().collect();
    let total = words.len() as f64;
    if total == 0.0 {
        return HashMap::new();
    }

    let mut tf: HashMap<&str, f64> = HashMap::new();
    for word in &words {
        *tf.entry(word).or_insert(0.0) += 1.0;
    }
    for val in tf.values_mut() {
        *val /= total;
    }

    tf.into_iter()
        .map(|(term, tf_val)| {
            let idf_val = idf.get(term).copied().unwrap_or(1.0);
            (term, tf_val * idf_val)
        })
        .collect()
}

fn cosine_from_vectors(a: &HashMap<&str, f64>, b: &HashMap<&str, f64>) -> f64 {
    let all_terms: std::collections::HashSet<&&str> = a.keys().chain(b.keys()).collect();
    if all_terms.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0;
    let mut mag_a = 0.0;
    let mut mag_b = 0.0;

    for term in &all_terms {
        let va = a.get(*term).copied().unwrap_or(0.0);
        let vb = b.get(*term).copied().unwrap_or(0.0);
        dot += va * vb;
        mag_a += va * va;
        mag_b += vb * vb;
    }

    let magnitude = (mag_a * mag_b).sqrt();
    if magnitude < f64::EPSILON {
        return 0.0;
    }

    dot / magnitude
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_duplicates_detects_similar_files() {
        let files = vec![
            (
                "a.rs".to_string(),
                "fn main() { let x = 1; let y = 2; println!(x + y); }".to_string(),
            ),
            (
                "b.rs".to_string(),
                "fn main() { let x = 1; let y = 2; println!(x + y); }".to_string(),
            ),
            (
                "c.rs".to_string(),
                "completely different content here with no overlap at all".to_string(),
            ),
        ];

        let dups = find_semantic_duplicates(&files, 0.8);
        assert_eq!(dups.len(), 1);
        assert!(dups[0].2 > 0.99);
    }
}
