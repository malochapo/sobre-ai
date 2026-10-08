const MAX_METADATA_LEN: usize = 200;

pub fn neutralize_metadata(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(MAX_METADATA_LEN));
    let mut count = 0usize;
    for ch in input.chars() {
        if count >= MAX_METADATA_LEN {
            out.push('…');
            break;
        }
        if (ch as u32) < 0x20 && ch != '\n' && ch != '\t' && ch != '\r' {
            continue;
        }
        match ch {
            '<' => out.push('‹'),
            '>' => out.push('›'),
            '`' => out.push('\''),
            _ => out.push(ch),
        }
        count += 1;
    }
    out
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// `project_knowledge` → `project knowledge`.
fn safe_label(label: &str) -> String {
    let words: String = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();
    let words = words.split_whitespace().collect::<Vec<_>>().join(" ");
    if words.is_empty() {
        "notes".to_string()
    } else {
        words
    }
}

/// Wraps stored notes so the agent reads them as data, not instructions: an
/// opening and a closing line carrying the same random token, which the notes
/// themselves cannot forge.
pub fn fence_content(label: &str, content: &str) -> String {
    let label = safe_label(label);
    let mut bytes = [0u8; 4];
    let _ = getrandom::fill(&mut bytes);
    let token = to_hex(&bytes);
    format!("[stored {label}, {token}]\n{content}\n[end of stored {label}, {token}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutralize_replaces_angle_and_backticks() {
        let s = "<tag>`code`</tag>";
        let out = neutralize_metadata(s);
        assert!(out.contains('‹'));
        assert!(out.contains('›'));
        assert!(!out.contains('`'));
    }

    #[test]
    fn fence_wraps_with_one_token_in_plain_words() {
        let out = fence_content("project_knowledge", "hello");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        let token = lines[0]
            .strip_prefix("[stored project knowledge, ")
            .and_then(|t| t.strip_suffix(']'))
            .expect("opening line");
        assert_eq!(lines[1], "hello");
        assert_eq!(
            lines[2],
            format!("[end of stored project knowledge, {token}]")
        );
    }
}
