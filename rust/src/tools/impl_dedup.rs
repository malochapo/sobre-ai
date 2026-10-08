use std::collections::{HashMap, HashSet};

use crate::core::cache::{SessionCache, SharedBlock};
use crate::core::tokens::count_tokens;

/// Registers the 5-line blocks that appear in two or more cached files as
/// shared blocks, so later reads can point at the first copy.
pub(crate) fn apply(cache: &mut SessionCache) {
    let entries = cache.get_all_entries();
    if entries.len() < 2 {
        return;
    }

    let mut block_occurrences: HashMap<String, Vec<(String, usize)>> = HashMap::new();
    for (path, entry) in &entries {
        let Some(full_content) = entry.content() else {
            continue;
        };
        let lines: Vec<&str> = full_content.lines().collect();
        for (idx, chunk) in lines.chunks(5).enumerate() {
            if chunk.len() == 5 {
                let block = chunk.join("\n");
                let trimmed = block.trim().to_string();
                if !trimmed.is_empty() && count_tokens(&trimmed) > 10 {
                    block_occurrences
                        .entry(trimmed)
                        .or_default()
                        .push(((*path).clone(), idx * 5 + 1));
                }
            }
        }
    }

    let mut shared = Vec::new();
    for (content, occurrences) in &block_occurrences {
        let unique_files: HashSet<&str> = occurrences.iter().map(|(p, _)| p.as_str()).collect();
        if unique_files.len() >= 2 {
            let (canonical_path, start_line) = &occurrences[0];
            let ref_label = cache
                .file_ref_map()
                .get(canonical_path)
                .cloned()
                .unwrap_or_else(|| "F?".to_string());
            shared.push(SharedBlock {
                canonical_path: canonical_path.clone(),
                canonical_ref: ref_label,
                start_line: *start_line,
                end_line: start_line + 4,
                content: content.clone(),
            });
        }
    }

    cache.set_shared_blocks(shared);
}
