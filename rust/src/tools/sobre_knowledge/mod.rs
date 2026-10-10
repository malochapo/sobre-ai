#[cfg(feature = "dense")]
use crate::core::embeddings::EmbeddingEngine;

use crate::core::knowledge::ProjectKnowledge;
use crate::core::memory_policy::MemoryPolicy;
use crate::core::session::SessionState;
pub(crate) mod embeddings;
mod remember;
pub(crate) use remember::*;
mod search;
pub(crate) use search::*;

mod consolidate;
pub(crate) use consolidate::*;
#[cfg(test)]
mod tests;

/// Dispatches the knowledge actions that need no tool-level arguments
/// beyond facts (`gotcha` and `consolidate` are handled by the tool).
#[allow(clippy::too_many_arguments)]
pub fn handle(
    project_root: &str,
    action: &str,
    category: Option<&str>,
    key: Option<&str>,
    value: Option<&str>,
    query: Option<&str>,
    session_id: &str,
    confidence: Option<f32>,
    mode: Option<&str>,
    as_of: Option<&str>,
) -> String {
    match action {
        "remember" => handle_remember(project_root, category, key, value, session_id, confidence),
        "recall" => handle_recall(project_root, category, query, session_id, mode, as_of),
        "search" => handle_search(project_root, query),
        "status" => handle_status(project_root),
        "remove" => handle_remove(project_root, category, key),
        _ => format!(
            "Unknown action: {action}. Use: remember, recall, search, status, remove, consolidate, gotcha"
        ),
    }
}

fn handle_status(project_root: &str) -> String {
    let Some(knowledge) = ProjectKnowledge::load(project_root) else {
        return "No knowledge stored for this project yet. Use sobre_knowledge(action=\"remember\") to start.".to_string();
    };

    let current_facts = knowledge.facts.iter().filter(|f| f.is_current()).count();
    let archived_facts = knowledge.facts.len() - current_facts;

    let mut out = format!(
        "Project Knowledge: {} active facts ({} archived), {} history entries\n",
        current_facts,
        archived_facts,
        knowledge.history.len()
    );
    out.push_str(&format!(
        "Last updated: {}\n",
        knowledge.updated_at.format("%Y-%m-%d %H:%M UTC")
    ));

    let rooms = knowledge.list_rooms();
    if !rooms.is_empty() {
        out.push_str("Rooms: ");
        let room_strs: Vec<String> = rooms.iter().map(|(c, n)| format!("{c}({n})")).collect();
        out.push_str(&room_strs.join(", "));
        out.push('\n');
    }

    out.push_str(&knowledge.format_summary());
    out
}

fn handle_remove(project_root: &str, category: Option<&str>, key: Option<&str>) -> String {
    let Some(cat) = category else {
        return "Error: category is required for remove".to_string();
    };
    let Some(k) = key else {
        return "Error: key is required for remove".to_string();
    };
    // A project with no knowledge has nothing to remove: write nothing.
    if ProjectKnowledge::load(project_root).is_none() {
        return format!("No fact found: [{cat}] {k}");
    }
    let policy = crate::core::memory_policy::MemoryPolicy::default();
    // Read-modify-write under the cross-process lock. The embedding
    // index is a separate store, so it is synced afterwards from the committed
    // knowledge — mirroring handle_remember.
    let (knowledge, removed) = match ProjectKnowledge::mutate_locked(project_root, |knowledge| {
        if knowledge.remove_fact(cat, k) {
            let _ = knowledge.run_memory_lifecycle(&policy);
            true
        } else {
            false
        }
    }) {
        Ok(pair) => pair,
        Err(e) => return format!("ERROR: [{cat}] {k} was not removed: {e}"),
    };

    if !removed {
        return format!("No fact found: [{cat}] {k}");
    }

    #[cfg(feature = "dense")]
    {
        // Serialize the embedding side-car under the same per-project lock as
        // the fact removal and compact against fresh on-disk knowledge, so a
        // concurrent `remember` cannot clobber it.
        ProjectKnowledge::with_project_lock(project_root, || {
            if let Some(mut idx) = crate::core::knowledge_embedding::KnowledgeEmbeddingIndex::load(
                &knowledge.project_hash,
            ) {
                idx.remove(cat, k);
                let fresh = ProjectKnowledge::load(project_root);
                let kref = fresh.as_ref().unwrap_or(&knowledge);
                crate::core::knowledge_embedding::compact_against_knowledge(
                    &mut idx, kref, &policy,
                );
                let _ = idx.save();
            }
        });
    }
    #[cfg(not(feature = "dense"))]
    let _ = &knowledge;

    format!("Removed [{cat}] {k}")
}
