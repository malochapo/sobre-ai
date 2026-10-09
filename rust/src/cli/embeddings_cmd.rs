//! `sobre embeddings` — semantic-embedding runtime management.
//!
//! The embedding engine loads ONNX Runtime dynamically. `provision` performs
//! the **explicit, consent-gated** download of the official CPU runtime into
//! the managed artifact layout; `status` shows what the resolver would use.
//! Nothing here runs implicitly — the policy is "no silent
//! download at first tool call, ever".

use crate::cli::args::EmbeddingsCommand;
#[cfg(feature = "dense")]
use crate::core::artifacts_exec::ort_provision;

/// Download everything local embeddings need: the official ONNX Runtime and
/// the configured model. The one place either is fetched; `index build` uses
/// what is installed. A build without embeddings downloads nothing.
pub(crate) fn provision(force: bool) -> Result<(), String> {
    #[cfg(not(feature = "dense"))]
    {
        let _ = force;
        Err(
            "this build has no local embeddings, so there is nothing to download \
             (they come with the sobre-dense release build, or a build with --features dense)"
                .to_string(),
        )
    }
    #[cfg(feature = "dense")]
    {
        println!(
            "Fetching the official ONNX Runtime {} (CPU) from microsoft/onnxruntime…",
            ort_provision::ORT_VERSION
        );
        let path = ort_provision::provision(force)?;
        println!("Installed: {}", path.display());
        println!("Fetching the embedding model from HuggingFace…");
        crate::core::embeddings::EmbeddingEngine::ensure_downloaded()
            .map_err(|e| format!("model download failed: {e}"))?;
        println!("Model installed.");
        println!(
            "Embeddings now work — semantic search picks this runtime up automatically \
             (ORT_DYLIB_PATH still overrides it)."
        );
        Ok(())
    }
}

pub(crate) fn cmd_embeddings(command: Option<&EmbeddingsCommand>) {
    match command {
        Some(EmbeddingsCommand::Provision { force }) => {
            if let Err(e) = provision(*force) {
                eprintln!("Provisioning failed: {e}");
                std::process::exit(1);
            }
        }
        Some(EmbeddingsCommand::Status) | None => {
            // Nothing to provision here, so nothing to report but that.
            #[cfg(not(feature = "dense"))]
            println!(
                "This build has no local embeddings: they come with the sobre-dense \
                 release build, or a build with --features dense."
            );
            #[cfg(feature = "dense")]
            {
                println!("{}", ort_provision::status_line());
                println!(
                    "This build requires ONNX Runtime >= 1.{}.x (ort api level).",
                    ort::MINOR_VERSION
                );
                if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
                    println!("ORT_DYLIB_PATH override active: {p}");
                }
                println!(
                    "{}",
                    crate::core::ort_execution_providers::execution_provider_status()
                );
                println!(
                    "{}",
                    crate::core::ort_execution_providers::execution_provider_help()
                );
            }
        }
    }
}
