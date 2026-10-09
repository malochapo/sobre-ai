use crate::core::cache::SessionCache;
use crate::core::tokens::count_tokens;
use crate::tools::CrpMode;
use crate::tools::sobre_read;

const DEFAULT_MAX_MULTI_READ_BYTES: usize = 512 * 1024;

fn max_multi_read_bytes() -> usize {
    DEFAULT_MAX_MULTI_READ_BYTES
}

pub fn handle_with_task_fresh(
    cache: &mut SessionCache,
    paths: &[String],
    mode: &str,
    fresh: bool,
    crp_mode: CrpMode,
    task: Option<&str>,
) -> String {
    let n = paths.len();
    if n == 0 {
        return "Read 0 files | 0 tokens saved".to_string();
    }

    let max_bytes = max_multi_read_bytes();
    let mut sections: Vec<String> = Vec::with_capacity(n);
    let mut total_saved: usize = 0;
    let mut total_original: usize = 0;
    let mut accumulated_bytes: usize = 0;
    let mut files_read = 0usize;
    let mut truncated = false;

    let mut capped: Vec<&str> = Vec::new();
    for path in paths {
        // Instruction files are read whole when no mode was picked; a mode the
        // agent picked is honoured.
        let effective_mode = if mode == "auto" && sobre_read::is_instruction_file(path) {
            "full"
        } else {
            mode
        };
        if truncated {
            capped.push(path);
            continue;
        }
        let chunk = if fresh {
            sobre_read::handle_fresh_with_task(cache, path, effective_mode, crp_mode, task)
        } else {
            sobre_read::handle_with_task(cache, path, effective_mode, crp_mode, task)
        };
        let chunk_bytes = chunk.len();
        if accumulated_bytes > 0 && accumulated_bytes + chunk_bytes > max_bytes {
            // This file was read but is not sent: forget the read, so a later
            // sobre_read returns its content instead of an [unchanged] stub.
            cache.invalidate(path);
            truncated = true;
            capped.push(path);
            continue;
        }
        let original = cache.get(path).map_or(0, |e| e.original_tokens);
        let sent = count_tokens(&chunk);
        total_original = total_original.saturating_add(original);
        total_saved = total_saved.saturating_add(original.saturating_sub(sent));
        accumulated_bytes += chunk_bytes;
        sections.push(chunk);
        files_read += 1;
    }

    let body = sections.join("\n---\n");
    let summary = if truncated {
        format!(
            "Read {files_read}/{n} files | {total_saved} tokens saved\n\
             ⚠ Output capped at {max_bytes} bytes. Not read: {}. \
             Read them with sobre_read(path=…) or a smaller paths list.",
            capped.join(", ")
        )
    } else if total_saved > 0 {
        format!("Read {n} files | {total_saved} tokens saved")
    } else {
        format!("Read {n} files")
    };
    format!("{body}\n---\n{summary}")
}
