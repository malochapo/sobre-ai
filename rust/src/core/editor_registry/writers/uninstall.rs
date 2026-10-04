// Hermes YAML cleanup used when rewriting the Hermes config.

pub(super) fn remove_hermes_yaml_lean_ctx_block(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut skip = false;
    for line in content.lines() {
        if line.trim_start().starts_with("lean-ctx:")
            && (line.starts_with("  ") || line.starts_with('\t'))
        {
            skip = true;
            continue;
        }
        if skip {
            let indented = line.starts_with("    ") || line.starts_with("\t\t");
            let empty = line.trim().is_empty();
            if indented || empty {
                continue;
            }
            skip = false;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Mistral Vibe TOML removal
// ---------------------------------------------------------------------------
