use std::io::Read;

use crate::core::cache::SessionCache;
use crate::core::compress_preview;
use crate::tools::{CrpMode, impl_compress};

pub(crate) fn cmd_compress(args: &[String]) {
    // `compress diff <file|->` previews what a compressor would emit (#984).
    if args.first().map(String::as_str) == Some("diff") {
        cmd_compress_diff(&args[1..]);
        return;
    }

    let signatures = args.iter().any(|a| a == "--signatures" || a == "-s");
    let json = args.iter().any(|a| a == "--json");

    #[cfg(unix)]
    {
        #[cfg(unix)]
        if let Some(out) = crate::daemon_client::try_daemon_tool_call_blocking_text(
            "ctx_compress",
            Some(serde_json::json!({
                "include_signatures": signatures
            })),
        ) {
            if json {
                let payload = serde_json::json!({ "output": out });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| out.clone())
                );
            } else {
                println!("{out}");
            }
            return;
        }
    }

    let cache = build_cli_cache();
    let out = impl_compress::handle(&cache, signatures, CrpMode::Off);

    if json {
        let payload = serde_json::json!({ "output": out });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| out.clone())
        );
    } else {
        println!("{out}");
    }
}

/// `lean-ctx compress diff [<file>|-] [--shell "<command>"] [--json]`
///
/// Side-by-side preview of the read (or shell) compression pipeline: original
/// vs the bytes lean-ctx would emit, with token/byte accounting and the diff.
/// Reads `-`/no-arg from stdin. `--shell "<cmd>"` previews the shell pipeline,
/// treating the input as that command's output.
fn cmd_compress_diff(args: &[String]) {
    let json = args.iter().any(|a| a == "--json");
    let shell_cmd = flag_value(args, "--shell");
    let target = first_positional(args);

    let (content, ext) = match target.as_deref() {
        None | Some("-") => {
            let mut buf = String::new();
            if std::io::stdin().read_to_string(&mut buf).is_err() {
                eprintln!("compress diff: failed to read stdin");
                std::process::exit(1);
            }
            (buf, None)
        }
        Some(path) => match std::fs::read_to_string(path) {
            Ok(c) => (c, compress_preview::ext_of(path)),
            Err(e) => {
                eprintln!("compress diff: cannot read {path}: {e}");
                std::process::exit(1);
            }
        },
    };

    let preview = match shell_cmd {
        Some(cmd) => compress_preview::preview_shell(&cmd, &content),
        None => compress_preview::preview_read(&content, ext.as_deref()),
    };

    if json {
        let payload = serde_json::json!({
            "pipeline": preview.pipeline.label(),
            "original_tokens": preview.original_tokens,
            "compressed_tokens": preview.compressed_tokens,
            "saved_tokens": preview.saved_tokens(),
            "saved_pct": preview.saved_pct(),
            "original_bytes": preview.original_bytes(),
            "compressed_bytes": preview.compressed_bytes(),
            "diff": preview.diff(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| preview.render())
        );
    } else {
        println!("{}", preview.render());
    }
}

/// Value for `--flag value` or `--flag=value`; `None` if absent.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let prefix = format!("{flag}=");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
        if let Some(v) = a.strip_prefix(&prefix) {
            return Some(v.to_string());
        }
    }
    None
}

/// First non-flag argument, skipping the `--shell <value>` pair.
fn first_positional(args: &[String]) -> Option<String> {
    let mut skip_next = false;
    for a in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if a == "--shell" {
            skip_next = true;
            continue;
        }
        if a.starts_with("--") {
            continue;
        }
        return Some(a.clone());
    }
    None
}

fn build_cli_cache() -> SessionCache {
    let mut cache = SessionCache::new();

    if let Some(session) = crate::core::session::SessionState::load_latest() {
        for ft in &session.files_touched {
            if let Ok(content) = std::fs::read_to_string(&ft.path) {
                cache.store(&ft.path, &content);
            }
        }
    }

    cache
}
