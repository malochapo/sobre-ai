use super::ranking::sort_fact_for_output;
use super::types::{ConsolidatedInsight, KnowledgeFact, ProjectKnowledge};
use crate::core::memory_policy::MemoryPolicy;

impl ProjectKnowledge {
    /// Record one consolidated insight, then losslessly reclaim history to its
    /// capacity headroom. Returns the number of older insights archived (0 when
    /// under cap).
    pub fn consolidate(
        &mut self,
        summary: &str,
        session_ids: Vec<String>,
        policy: &MemoryPolicy,
    ) -> Result<usize, String> {
        self.history.push(ConsolidatedInsight {
            summary: summary.to_string(),
            from_sessions: session_ids,
            timestamp: chrono::Utc::now(),
        });

        let archived = crate::core::memory_capacity::reclaim_store(
            crate::core::memory_archive::MemoryStore::History,
            &self.project_hash,
            &mut self.history,
            policy.knowledge.max_history,
            policy.lifecycle.reclaim_headroom_pct,
            |a, b| {
                b.timestamp
                    .cmp(&a.timestamp)
                    .then_with(|| b.summary.cmp(&a.summary))
            },
        )?;
        self.updated_at = chrono::Utc::now();
        Ok(archived.len())
    }

    pub fn format_summary(&self) -> String {
        let mut out = String::new();
        let current_facts: Vec<&KnowledgeFact> =
            self.facts.iter().filter(|f| f.is_current()).collect();

        if !current_facts.is_empty() {
            out.push_str("PROJECT KNOWLEDGE:\n");
            let mut rooms: Vec<(String, usize)> = self.list_rooms();
            rooms.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

            let total_rooms = rooms.len();
            rooms.truncate(crate::core::budgets::KNOWLEDGE_SUMMARY_ROOMS_LIMIT);

            for (cat, _count) in rooms {
                out.push_str(&format!("  [{cat}]\n"));

                let mut facts_in_cat: Vec<&KnowledgeFact> = current_facts
                    .iter()
                    .copied()
                    .filter(|f| f.category == cat)
                    .collect();
                facts_in_cat.sort_by(|a, b| sort_fact_for_output(a, b));

                let total_in_cat = facts_in_cat.len();
                facts_in_cat.truncate(crate::core::budgets::KNOWLEDGE_SUMMARY_FACTS_PER_ROOM_LIMIT);

                for f in facts_in_cat {
                    let key = crate::core::sanitize::neutralize_metadata(&f.key);
                    let val = crate::core::sanitize::neutralize_metadata(&f.value);
                    out.push_str(&format!(
                        "    {}: {} (confidence: {:.0}%)\n",
                        key,
                        val,
                        f.confidence * 100.0
                    ));
                }
                if total_in_cat > crate::core::budgets::KNOWLEDGE_SUMMARY_FACTS_PER_ROOM_LIMIT {
                    out.push_str(&format!(
                        "    … +{} more\n",
                        total_in_cat - crate::core::budgets::KNOWLEDGE_SUMMARY_FACTS_PER_ROOM_LIMIT
                    ));
                }
            }

            if total_rooms > crate::core::budgets::KNOWLEDGE_SUMMARY_ROOMS_LIMIT {
                out.push_str(&format!(
                    "  … +{} more rooms\n",
                    total_rooms - crate::core::budgets::KNOWLEDGE_SUMMARY_ROOMS_LIMIT
                ));
            }
        }

        if out.is_empty() {
            out
        } else {
            crate::core::sanitize::fence_content("project_knowledge", out.trim_end())
        }
    }

    pub fn format_wakeup(&self) -> String {
        let current_facts: Vec<&KnowledgeFact> = self
            .facts
            .iter()
            .filter(|f| f.is_current() && f.confidence >= 0.7)
            .collect();

        if current_facts.is_empty() {
            return String::new();
        }

        // Theta-gamma chunking: salience-ordered top-K facts grouped
        // into 4±1-sized thematic chunks; shared headers amortize category
        // prefixes (token savings) and prime related facts (recall).
        let mut top_facts: Vec<&KnowledgeFact> = current_facts;
        top_facts.sort_by(|a, b| sort_fact_for_output(a, b));
        top_facts.truncate(20);

        let clusters = super::chunking::cluster_facts(&top_facts);
        crate::core::sanitize::fence_content(
            "project_facts_wakeup",
            &super::chunking::render_chunked(&clusters),
        )
    }
}
