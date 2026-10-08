//! Shared edit-distance "did you mean" helper (#712).
//!
//! One implementation for every typo-suggestion surface: CLI commands
//! (`cli/dispatch/suggest.rs`), config keys (`cli/config_cmd.rs`) and MCP
//! tool names (`server/dispatch`). Wagner-Fischer over Unicode scalar values
//! with a single rolling row — candidate sets are tiny (dozens of names), so
//! O(a·b) time per pair is irrelevant; what matters is that all callers agree
//! on distances.

/// Edit distance where inserting, deleting or replacing a character, or
/// swapping two adjacent ones, costs one (optimal string alignment): `inti`
/// is one edit from `init`.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev2: Vec<usize> = vec![0; b.len() + 1];
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for i in 0..a.len() {
        curr[0] = i + 1;
        for j in 0..b.len() {
            let cost = usize::from(a[i] != b[j]);
            let mut d = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
            if i > 0 && j > 0 && a[i] == b[j - 1] && a[i - 1] == b[j] {
                d = d.min(prev2[j - 1] + 1);
            }
            curr[j + 1] = d;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// The closest candidate within a length-scaled edit budget (one edit for
/// short names, roughly a third of the length for longer ones), or `None`
/// when nothing is near enough to suggest with confidence. Ties resolve to
/// the first candidate in iteration order.
pub fn closest<'a, I>(input: &str, candidates: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let budget = (input.chars().count() / 3).max(1);
    candidates
        .into_iter()
        .map(|cand| (cand, levenshtein(input, cand)))
        .filter(|&(_, dist)| dist <= budget)
        .min_by_key(|&(_, dist)| dist)
        .map(|(cand, _)| cand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_distances() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", "abc"), 0);
        assert_eq!(levenshtein("abc", "abd"), 1);
        // A swap of two adjacent letters is one edit.
        assert_eq!(levenshtein("udpate", "update"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
    }

    #[test]
    fn unicode_scalars_not_bytes() {
        assert_eq!(levenshtein("héllo", "hello"), 1);
    }

    #[test]
    fn closest_respects_length_scaled_budget() {
        let tools = ["sobre_read", "sobre_search", "sobre_shell", "sobre_tree"];
        assert_eq!(closest("sobre_raed", tools), Some("sobre_read"));
        assert_eq!(closest("sobre_serach", tools), Some("sobre_search"));
        // Distance beyond the budget → no confident suggestion.
        assert_eq!(closest("completely_else", tools), None);
        assert_eq!(closest("", tools), None);
    }
}
