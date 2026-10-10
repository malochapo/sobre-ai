use crate::core::patterns::compact_lines;
use std::collections::HashMap;

/// The AWS CLI's output, by command. A query (`describe-…`, `list-…`,
/// `get-…` and the other verbs `is_cloud_cli_query` lists) never reaches
/// here: the output policy keeps it verbatim.
pub fn compress(cmd: &str, output: &str) -> Option<String> {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Some("ok".to_string());
    }

    if cmd.contains("s3 ls") || cmd.contains("s3 cp") || cmd.contains("s3 sync") {
        return Some(compress_s3(cmd, trimmed));
    }
    if cmd.contains("logs") {
        return Some(compress_logs(trimmed));
    }

    Some(compact_json_or_text(trimmed, 15))
}

fn compress_s3(cmd: &str, output: &str) -> String {
    if cmd.contains("s3 ls") {
        let entries: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
        if entries.len() <= 20 {
            return entries.join("\n");
        }
        let dirs: Vec<&&str> = entries.iter().filter(|l| l.contains("PRE ")).collect();
        let files: Vec<&&str> = entries.iter().filter(|l| !l.contains("PRE ")).collect();
        return format!(
            "{} dirs, {} files\n{}",
            dirs.len(),
            files.len(),
            entries
                .iter()
                .take(15)
                .copied()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    let mut uploaded = 0u32;
    let mut copied = 0u32;
    for line in output.lines() {
        if line.contains("upload:") {
            uploaded += 1;
        }
        if line.contains("copy:") {
            copied += 1;
        }
    }
    if uploaded + copied == 0 {
        return compact_lines(output, 10);
    }
    let mut result = String::new();
    if uploaded > 0 {
        result.push_str(&format!("{uploaded} uploaded"));
    }
    if copied > 0 {
        if !result.is_empty() {
            result.push_str(", ");
        }
        result.push_str(&format!("{copied} copied"));
    }
    result
}

fn compress_logs(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= 20 {
        return output.to_string();
    }
    let mut deduped: HashMap<String, u32> = HashMap::new();
    for line in &lines {
        let key = line
            .split_whitespace()
            .skip(2)
            .collect::<Vec<_>>()
            .join(" ");
        if !key.is_empty() {
            *deduped.entry(key).or_insert(0) += 1;
        }
    }
    let mut sorted: Vec<_> = deduped.into_iter().collect();
    sorted.sort_by_key(|x| std::cmp::Reverse(x.1));
    let top: Vec<String> = sorted
        .iter()
        .take(15)
        .map(|(msg, count)| {
            if *count > 1 {
                format!("  ({count}x) {msg}")
            } else {
                format!("  {msg}")
            }
        })
        .collect();
    format!(
        "{} log entries (deduped to {}):\n{}",
        lines.len(),
        top.len(),
        top.join("\n")
    )
}

fn compact_json_or_text(text: &str, max: usize) -> String {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(text) {
        let keys = extract_top_keys(&val);
        if !keys.is_empty() {
            return format!("JSON: {{{}}}", keys.join(", "));
        }
    }
    compact_lines(text, max)
}

fn extract_top_keys(val: &serde_json::Value) -> Vec<String> {
    match val {
        serde_json::Value::Object(map) => map
            .keys()
            .take(20)
            .map(|k| {
                let v = &map[k];
                match v {
                    serde_json::Value::Array(a) => format!("{k}: [{} items]", a.len()),
                    serde_json::Value::Object(_) => format!("{k}: {{...}}"),
                    _ => format!("{k}: {v}"),
                }
            })
            .collect(),
        _ => vec![],
    }
}
