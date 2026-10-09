//! Semantic knowledge search + fact formatting/salience helpers.
//! Split out of `sobre_knowledge/mod.rs`; `use super::*` re-imports parent items.

#[allow(clippy::wildcard_imports)]
use super::*;
#[cfg(feature = "dense")]
use chrono::Utc;

/// Searches this project's current knowledge facts (superseded and archived
/// ones are left out) and the findings and decisions of its saved sessions.
/// Nothing from another project is read.
pub(crate) fn handle_search(project_root: &str, query: Option<&str>) -> String {
    let Some(q) = query else {
        return "Error: query is required for search".to_string();
    };
    let q_lower = q.to_lowercase();
    let terms: Vec<&str> = q_lower.split_whitespace().collect();
    if terms.is_empty() {
        return "Error: query is required for search".to_string();
    }
    let relevance = |text: &str| {
        let text = text.to_lowercase();
        let hits = terms.iter().filter(|t| text.contains(**t)).count();
        hits as f32 / terms.len() as f32
    };

    // (category, key, value, confidence, relevance)
    let mut results: Vec<(String, String, String, f32, f32)> = Vec::new();

    if let Some(knowledge) = ProjectKnowledge::load(project_root) {
        for fact in knowledge.facts.iter().filter(|f| f.is_current()) {
            let r = relevance(&format!("{} {} {}", fact.category, fact.key, fact.value));
            if r > 0.0 {
                results.push((
                    fact.category.clone(),
                    fact.key.clone(),
                    fact.value.clone(),
                    fact.confidence,
                    r,
                ));
            }
        }
    }

    for session in SessionState::all_for_project_root(project_root) {
        for finding in &session.findings {
            let r = relevance(&finding.summary);
            if r > 0.0 {
                results.push((
                    "session-finding".to_string(),
                    session.id.clone(),
                    finding.summary.clone(),
                    0.6,
                    r,
                ));
            }
        }
        for decision in &session.decisions {
            let r = relevance(&decision.summary);
            if r > 0.0 {
                results.push((
                    "session-decision".to_string(),
                    session.id.clone(),
                    decision.summary.clone(),
                    0.7,
                    r,
                ));
            }
        }
    }

    if results.is_empty() {
        return format!("No results for '{q}' in this project's knowledge and sessions.");
    }

    results.sort_by(|a, b| {
        b.4.partial_cmp(&a.4)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    results.truncate(crate::core::budgets::KNOWLEDGE_SEARCH_LIMIT);

    let mut out = format!(
        "Search '{q}' in this project's knowledge and sessions ({} results):\n",
        results.len()
    );
    for (cat, key, value, conf, _relevance) in &results {
        out.push_str(&format!(
            "  [{cat}/{key}] {value} (conf: {:.0}%)\n",
            conf * 100.0
        ));
    }
    out
}

#[cfg(feature = "dense")]
pub(crate) struct SemanticHit {
    pub(crate) category: String,
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) score: f32,
    pub(crate) semantic_score: f32,
    pub(crate) confidence_score: f32,
}

#[cfg(feature = "dense")]
pub(crate) fn apply_retrieval_signals_from_hits(
    knowledge: &mut ProjectKnowledge,
    hits: &[SemanticHit],
) {
    let now = Utc::now();
    for s in hits {
        for f in &mut knowledge.facts {
            if !f.is_current() {
                continue;
            }
            if f.category == s.category && f.key == s.key {
                f.retrieval_count = f.retrieval_count.saturating_add(1);
                f.last_retrieved = Some(now);
                break;
            }
        }
    }
}

#[cfg(feature = "dense")]
pub(crate) fn format_semantic_facts(query: &str, hits: &[SemanticHit]) -> String {
    if hits.is_empty() {
        return format!("No facts matching '{query}'.");
    }
    let mut out = format!("Semantic recall '{query}' (showing {}):\n", hits.len());
    for s in hits {
        out.push_str(&format!(
            "  [{}/{}]: {} (score: {:.0}%, sem: {:.0}%, conf: {:.0}%)\n",
            s.category,
            s.key,
            s.value,
            s.score * 100.0,
            s.semantic_score * 100.0,
            s.confidence_score * 100.0
        ));
    }
    out
}

pub(crate) fn format_facts_with_annotations(
    facts: &[crate::core::knowledge::KnowledgeFact],
    total: usize,
    category: Option<&str>,
) -> String {
    // Preserve the caller's order: recall_for_output / recall_by_category_for_output
    // already rank facts (exact-match precedence, relevance). Re-sorting here by
    // salience would discard that ranking. The formatter renders; it does not re-rank.
    let facts: Vec<&crate::core::knowledge::KnowledgeFact> = facts.iter().collect();

    let mut out = String::new();
    if let Some(cat) = category {
        out.push_str(&format!(
            "Facts [{cat}] (showing {}/{}):\n",
            facts.len(),
            total
        ));
    } else {
        out.push_str(&format!(
            "Matching facts (showing {}/{}):\n",
            facts.len(),
            total
        ));
    }
    for f in facts {
        let temporal = if f.is_current() { "" } else { " [archived]" };
        let rev = if f.revision_count > 1 {
            format!(" rev {}", f.revision_count)
        } else {
            String::new()
        };
        out.push_str(&format!(
            "  [{}/{}]: {} (quality: {:.0}%, confidence: {:.0}%, confirmed: {} x{}){rev}{temporal}\n",
            f.category,
            f.key,
            f.value,
            f.quality_score() * 100.0,
            f.confidence * 100.0,
            f.last_confirmed.format("%Y-%m-%d"),
            f.confirmation_count
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::core::knowledge::ProjectKnowledge;
    use crate::core::memory_policy::MemoryPolicy;

    // The formatter renders in the caller's order and must never re-rank: the
    // display must not re-sort by salience (which would put the high-salience
    // gotcha first). Feeding both orders proves no re-sort.
    #[test]
    fn format_preserves_caller_order_not_salience() {
        let policy = MemoryPolicy::default();
        let mut k = ProjectKnowledge::new("/tmp/test-fmt-preserve");
        k.remember(
            "gotcha",
            "src/a.rs:1",
            "race condition",
            "s1",
            0.95,
            &policy,
        );
        k.remember(
            "observation",
            "src/a.rs",
            "src/a.rs — gotcha: race condition",
            "s2",
            0.60,
            &policy,
        );
        let obs = k
            .facts
            .iter()
            .find(|f| f.category == "observation")
            .cloned()
            .expect("observation present");
        let gotcha = k
            .facts
            .iter()
            .find(|f| f.category == "gotcha")
            .cloned()
            .expect("gotcha present");

        let out = super::format_facts_with_annotations(&[obs.clone(), gotcha.clone()], 2, None);
        assert!(
            out.find("[observation/").unwrap() < out.find("[gotcha/").unwrap(),
            "observation passed first must stay first in the rendered output"
        );

        let out_rev = super::format_facts_with_annotations(&[gotcha, obs], 2, None);
        assert!(
            out_rev.find("[gotcha/").unwrap() < out_rev.find("[observation/").unwrap(),
            "formatter must preserve caller order, never impose its own salience sort"
        );
    }
}
