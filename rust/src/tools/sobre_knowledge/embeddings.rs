//! Embedding engine access + status/reset/reindex handlers.
//! Split out of `sobre_knowledge/mod.rs`; `use super::*` re-imports parent items.

// Everything in this file that needs the parent's items is `embeddings`-gated.
#[cfg(feature = "dense")]
#[allow(clippy::wildcard_imports)]
use super::*;

#[cfg(feature = "dense")]
pub(crate) fn embedding_engine() -> Option<&'static EmbeddingEngine> {
    embedding_engine_impl(false)
}

/// Non-blocking: returns engine only if already loaded. Never blocks the
/// calling path — but kicks off a one-time background load (#551) so the first
/// semantic need activates the engine for later calls. Nothing is downloaded:
/// the model must already be installed.
#[cfg(feature = "dense")]
pub(crate) fn embedding_engine_nonblocking() -> Option<&'static EmbeddingEngine> {
    embedding_engine_impl(true)
}

#[cfg(feature = "dense")]
pub(crate) fn embedding_engine_impl(nonblocking: bool) -> Option<&'static EmbeddingEngine> {
    if !EmbeddingEngine::is_available() {
        return None;
    }
    if nonblocking {
        let engine = crate::core::embeddings::try_shared_engine();
        if engine.is_none() {
            ensure_engine_background();
        }
        engine
    } else {
        crate::core::embeddings::shared_engine()
    }
}

/// One-time background engine activation (#551): warms the shared engine from
/// the installed model, so non-blocking callers start succeeding without any
/// hot path ever waiting. Policy gates (memory profile, model present) are
/// checked by the caller.
#[cfg(feature = "dense")]
fn ensure_engine_background() {
    use std::sync::atomic::{AtomicBool, Ordering};
    // Never spawn a detached model load in a short-lived process (#519): a
    // loader thread still running when the process returns from `main` races
    // libonnxruntime's static-destructor teardown → SIGSEGV in onnx::OpSchema.
    // This also keeps the process-global shared engine untouched in tests
    // (races assertions).
    // `background_load_allowed` covers unit tests (cfg!(test)) AND integration/
    // bench/doctest binaries (which link the lib with cfg(test) = false).
    if !crate::core::embeddings::background_load_allowed() {
        return;
    }
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| {
        // `shared_engine` runs the ONNX load inside the OnceLock init;
        // concurrent blocking callers simply wait on the same init instead of
        // duplicating work.
        let loaded = crate::core::embeddings::shared_engine().is_some();
        tracing::info!("embedding engine background activation finished (loaded={loaded})");
    });
}

pub(crate) fn handle_embeddings_status(project_root: &str) -> String {
    #[cfg(feature = "dense")]
    {
        let knowledge = ProjectKnowledge::load_or_create(project_root);
        let model_available = EmbeddingEngine::is_available();

        let entries = crate::core::knowledge_embedding::KnowledgeEmbeddingIndex::load(
            &knowledge.project_hash,
        )
        .map_or(0, |i| i.entries.len());

        let path = crate::core::data_dir::sobre_data_dir()
            .ok()
            .map(|d| {
                d.join("knowledge")
                    .join(&knowledge.project_hash)
                    .join("embeddings.json")
            })
            .map_or_else(|| "<unknown>".to_string(), |p| p.display().to_string());

        format!(
            "Knowledge embeddings: model={}, index_entries={}, path={path}",
            if model_available {
                "present"
            } else {
                "missing"
            },
            entries
        )
    }
    #[cfg(not(feature = "dense"))]
    {
        let _ = project_root;
        "ERR: dense feature not enabled".to_string()
    }
}

pub(crate) fn handle_embeddings_reset(project_root: &str) -> String {
    #[cfg(feature = "dense")]
    {
        let knowledge = ProjectKnowledge::load_or_create(project_root);
        match crate::core::knowledge_embedding::reset(&knowledge.project_hash) {
            Ok(()) => "Embeddings index reset.".to_string(),
            Err(e) => format!("Embeddings reset failed: {e}"),
        }
    }
    #[cfg(not(feature = "dense"))]
    {
        let _ = project_root;
        "ERR: dense feature not enabled".to_string()
    }
}

pub(crate) fn handle_embeddings_reindex(project_root: &str) -> String {
    #[cfg(feature = "dense")]
    {
        if ProjectKnowledge::load(project_root).is_none() {
            return "No knowledge stored for this project yet.".to_string();
        }
        let policy = crate::core::memory_policy::MemoryPolicy::default();

        let Some(engine) = embedding_engine() else {
            return "Embeddings model not installed. Run `sobre embeddings provision`, then re-run."
                    .to_string();
        };

        // Rebuild + save under the per-project lock, reloading knowledge inside
        // it so a `remember` committed mid-reindex is included rather than
        // clobbered by a stale-snapshot rebuild (issue #412). The model is
        // fetched above, outside the lock, so its load never blocks writers.
        ProjectKnowledge::with_project_lock(project_root, || {
            let knowledge = ProjectKnowledge::load_or_create(project_root);
            let mut idx = crate::core::knowledge_embedding::KnowledgeEmbeddingIndex::new(
                &knowledge.project_hash,
            );

            let mut facts: Vec<&crate::core::knowledge::KnowledgeFact> =
                knowledge.facts.iter().filter(|f| f.is_current()).collect();
            facts.sort_by(|a, b| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| b.last_confirmed.cmp(&a.last_confirmed))
                    .then_with(|| a.category.cmp(&b.category))
                    .then_with(|| a.key.cmp(&b.key))
            });

            let max = policy.embeddings.max_facts;
            let mut embedded = 0usize;
            for f in facts.into_iter().take(max) {
                if crate::core::knowledge_embedding::embed_and_store(
                    &mut idx,
                    engine,
                    &f.category,
                    &f.key,
                    &f.value,
                )
                .is_ok()
                {
                    embedded += 1;
                }
            }

            crate::core::knowledge_embedding::compact_against_knowledge(
                &mut idx, &knowledge, &policy,
            );
            match idx.save() {
                Ok(()) => format!("Embeddings reindex ok (embedded {embedded} facts)."),
                Err(e) => format!("Embeddings reindex failed: {e}"),
            }
        })
    }
    #[cfg(not(feature = "dense"))]
    {
        let _ = project_root;
        "ERR: dense feature not enabled".to_string()
    }
}
