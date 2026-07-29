//! `sobre gotchas` — local bug-memory inspection.
//!
//! Rescued verbatim from the deleted `cli/cloud.rs`, where it had no business
//! being: it reads `core::gotcha_tracker` off disk and never touches a network.

use crate::core;

pub fn cmd_gotchas(args: &[String]) {
    let action = args.first().map_or("list", std::string::String::as_str);
    let project_root = std::env::current_dir()
        .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string());

    match action {
        "list" | "ls" => {
            let store = core::gotcha_tracker::GotchaStore::load(&project_root);
            println!("{}", store.format_list());
        }
        "clear" => {
            let mut store = core::gotcha_tracker::GotchaStore::load(&project_root);
            let count = store.gotchas.len();
            store.clear();
            let _ = store.save(&project_root);
            println!("Cleared {count} gotchas.");
        }
        "export" => {
            let store = core::gotcha_tracker::GotchaStore::load(&project_root);
            match serde_json::to_string_pretty(&store.gotchas) {
                Ok(json) => println!("{json}"),
                Err(e) => tracing::error!("Export failed: {e}"),
            }
        }
        "stats" => {
            let store = core::gotcha_tracker::GotchaStore::load(&project_root);
            println!("Bug Memory Stats:");
            println!("  Active gotchas:      {}", store.gotchas.len());
            println!(
                "  Errors detected:     {}",
                store.stats.total_errors_detected
            );
            println!(
                "  Fixes correlated:    {}",
                store.stats.total_fixes_correlated
            );
            println!("  Bugs prevented:      {}", store.stats.total_prevented);
            println!("  Promoted to knowledge: {}", store.stats.gotchas_promoted);
            println!("  Decayed/archived:    {}", store.stats.gotchas_decayed);
            println!("  Session logs:        {}", store.error_log.len());
        }
        "reflect" | "ledger" => {
            let store = core::gotcha_tracker::GotchaStore::load(&project_root);
            println!("{}", core::gotcha_tracker::format_ledger(&store));
        }
        _ => {
            println!("Usage: lean-ctx gotchas [list|clear|export|stats|reflect]");
        }
    }
}
