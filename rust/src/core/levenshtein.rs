//! Shared edit-distance "did you mean" helper.
//!
//! One policy for every typo-suggestion surface: CLI commands
//! (`cli/dispatch/suggest.rs`), config keys (`cli/config_cmd.rs`) and MCP
//! tool names (`server/dispatch`), so all callers agree on what is close
//! enough. The distance is `strsim`'s optimal string alignment over Unicode
//! scalar values: inserting, deleting or replacing a character, or swapping
//! two adjacent ones, costs one, so `inti` is one edit from `init`.

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
        .map(|cand| (cand, strsim::osa_distance(input, cand)))
        .filter(|&(_, dist)| dist <= budget)
        .min_by_key(|&(_, dist)| dist)
        .map(|(cand, _)| cand)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A swap of two adjacent letters is one edit, as is a replacement.
    #[test]
    fn basic_distances() {
        assert_eq!(closest("abc", ["abc"]), Some("abc"));
        assert_eq!(closest("abd", ["abc"]), Some("abc"));
        assert_eq!(closest("inti", ["init"]), Some("init"));
        assert_eq!(closest("udpate", ["update"]), Some("update"));
        // Three edits exceed a six-letter name's budget of two.
        assert_eq!(closest("kitten", ["sitting"]), None);
        // The nearer candidate wins.
        assert_eq!(closest("udpate", ["upgrade", "update"]), Some("update"));
    }

    #[test]
    fn unicode_scalars_not_bytes() {
        // `é` is two bytes but one character: one edit within budget.
        assert_eq!(closest("héllo", ["hello"]), Some("hello"));
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
