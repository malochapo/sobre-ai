//! `lean-ctx pricing` — the explicit, user-requested fetch of live model prices.
//!
//! sobre values saved tokens with an embedded price table. `refresh` downloads
//! current prices from the public OpenRouter and LiteLLM catalogs into the local
//! cache, which the proxy and `gain` read from then on. Nothing else fetches
//! prices: no background refresh, no download at proxy start.

use crate::core::gain::live_pricing;

pub(crate) fn cmd_pricing(rest: &[String]) {
    match rest.first().map(String::as_str) {
        Some("refresh") => {
            println!("Fetching model prices from openrouter.ai and the LiteLLM price map…");
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    eprintln!("could not start the async runtime: {e}");
                    std::process::exit(1);
                }
            };
            match runtime.block_on(live_pricing::refresh_now(&reqwest::Client::new())) {
                Ok(n) => println!("Cached {n} model price entries."),
                Err(e) => {
                    eprintln!("Price refresh failed: {e:#}");
                    std::process::exit(1);
                }
            }
        }
        Some("status") | None => {
            live_pricing::ensure_loaded();
            match live_pricing::status() {
                Some((fetched_at, n)) => {
                    let age_h = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs())
                        .saturating_sub(fetched_at)
                        / 3600;
                    println!("Live prices: {n} entries, fetched {age_h}h ago.");
                }
                None => println!(
                    "Live prices: none cached — using the embedded table. \
                     Run `lean-ctx pricing refresh` to fetch current prices."
                ),
            }
        }
        Some(other) => {
            eprintln!(
                "Unknown subcommand `{other}`.\n\nUsage:\n  \
                 lean-ctx pricing status    Show the cached live price table\n  \
                 lean-ctx pricing refresh   Fetch current prices (network)"
            );
            std::process::exit(1);
        }
    }
}
