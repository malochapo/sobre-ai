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
/// calling path — but kicks off a one-time background load so the first
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

/// One-time background engine activation: warms the shared engine from
/// the installed model, so non-blocking callers start succeeding without any
/// hot path ever waiting. Policy gates (memory profile, model present) are
/// checked by the caller.
#[cfg(feature = "dense")]
fn ensure_engine_background() {
    use std::sync::atomic::{AtomicBool, Ordering};
    // Never spawn a detached model load in a short-lived process: a
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
