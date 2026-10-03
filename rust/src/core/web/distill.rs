//! Extractive research-compression modes for prose and transcripts.
//!
//! These are deterministic, heuristic distillations — no LLM in the loop — so
//! they are cheap, reproducible, and safe to run inside a synchronous tool
//! handler. They turn a cleaned article or transcript into the high-signal
//! subset an agent actually needs:
//!
//! * [`facts_scored`] — sentences carrying factual signals (numbers, dates,
//!   entities), each with a confidence score.
//! * [`quotes_scored`] — the most central / query-relevant sentences, as
//!   evidence, each with a confidence score.
//! * [`transcript_summary`] — de-duplicated, filler-stripped spoken text.

use std::collections::HashSet;

const FILLER: &[&str] = &[
    "um",
    "uh",
    "erm",
    "hmm",
    "like",
    "basically",
    "actually",
    "literally",
    "honestly",
    "okay",
    "ok",
    "yeah",
    "right",
    "so",
    "well",
    "anyway",
    "anyways",
];

/// Condense a transcript: strip filler, drop near-duplicate runs, cap length.
pub fn transcript_summary(text: &str, max_chars: usize) -> String {
    let mut kept: Vec<String> = Vec::new();
    let mut total = 0usize;

    for sentence in split_sentences(text) {
        let cleaned = strip_filler(&sentence);
        let cleaned = cleaned.trim();
        if cleaned.chars().count() < 8 {
            continue;
        }
        if let Some(last) = kept.last()
            && jaccard(last, cleaned) > 0.8
        {
            continue;
        }
        if total + cleaned.len() > max_chars && !kept.is_empty() {
            break;
        }
        total += cleaned.len();
        kept.push(cleaned.to_string());
    }
    kept.join(" ")
}

/// Line-structure-preserving prose squeeze for the proxy tool-result funnel.
///
/// Unlike [`transcript_summary`] (which collapses everything into one paragraph),
/// this keeps paragraph/heading shape and only:
/// * collapses runs of blank lines to a single blank,
/// * drops a line that is a near-duplicate of a recently kept line
///   (boilerplate / nav repeats common in scraped pages),
/// * caps total length to `max_chars` with a truncation marker.
///
/// Filler-word stripping is intentionally *not* applied here: words like
/// "so" / "like" / "right" carry meaning in written prose and are only noise in
/// spoken transcripts.
pub fn squeeze_prose(text: &str, max_chars: usize) -> String {
    const RECENT: usize = 12;
    let mut out: Vec<String> = Vec::new();
    let mut recent: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut blank_run = 0u32;

    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run == 1 && !out.is_empty() {
                out.push(String::new());
            }
            continue;
        }
        blank_run = 0;

        let normalized = line.trim();
        if !is_protected_line(line) && recent.iter().any(|p| jaccard(p, normalized) > 0.9) {
            continue;
        }

        if total + line.len() > max_chars && !out.is_empty() {
            out.push("…[truncated]".to_string());
            break;
        }
        total += line.len();
        out.push(line.to_string());

        recent.push(normalized.to_string());
        if recent.len() > RECENT {
            recent.remove(0);
        }
    }

    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

/// Lines that must survive dedup: citations, links, headings and quote/list
/// markers carry attribution or structure even when textually similar.
pub(crate) fn is_protected_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("Source:")
        || t.starts_with("Site:")
        || t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("- [")
        || t.starts_with("> ")
        || t.starts_with('#')
        || t.starts_with("---")
}

// ── Sentence splitting ─────────────────────────────────────────────────────

/// Split text into trimmed, non-empty sentences across line boundaries.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut current = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            current.push(c);
            if matches!(c, '.' | '!' | '?') {
                let boundary = chars.peek().is_none_or(|n| n.is_whitespace());
                if boundary {
                    push_trimmed(&mut sentences, &current);
                    current.clear();
                }
            }
        }
        push_trimmed(&mut sentences, &current);
    }
    sentences
}

fn push_trimmed(acc: &mut Vec<String>, s: &str) {
    let trimmed = s.trim();
    if !trimmed.is_empty() {
        acc.push(trimmed.to_string());
    }
}

// ── Scoring ────────────────────────────────────────────────────────────────

// ── Word helpers ───────────────────────────────────────────────────────────

fn word_set(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn jaccard(a: &str, b: &str) -> f64 {
    let sa = word_set(a);
    let sb = word_set(b);
    if sa.is_empty() && sb.is_empty() {
        return 1.0;
    }
    let inter = sa.intersection(&sb).count() as f64;
    let union = sa.union(&sb).count() as f64;
    if union == 0.0 { 0.0 } else { inter / union }
}

fn strip_filler(sentence: &str) -> String {
    sentence
        .split_whitespace()
        .filter(|tok| {
            let core: String = tok
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase();
            !core.is_empty() && !FILLER.contains(&core.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_sentences_across_lines() {
        let text = "First sentence here. Second one follows!\nThird line stands alone?";
        let s = split_sentences(text);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0], "First sentence here.");
        assert_eq!(s[2], "Third line stands alone?");
    }

    #[test]
    fn transcript_summary_strips_filler_and_dupes() {
        let text = "Um so basically the model is really fast. \
                    Um so basically the model is really fast. \
                    Actually it scales to millions of requests.";
        let summary = transcript_summary(text, 500);
        assert!(!summary.to_lowercase().contains("basically"));
        // Near-duplicate second line is dropped.
        assert_eq!(summary.matches("the model is really fast").count(), 1);
        assert!(summary.contains("scales to millions"));
    }

    #[test]
    fn transcript_summary_respects_budget() {
        let text = "Alpha statement number one here. Beta statement number two here. \
                    Gamma statement number three here.";
        let summary = transcript_summary(text, 30);
        assert!(summary.len() <= 60, "got {} chars", summary.len());
        assert!(summary.contains("Alpha"));
    }

    #[test]
    fn squeeze_prose_dedupes_and_collapses_blanks() {
        let text = "Rust is a systems programming language focused on safety.\n\n\n\
                    Rust is a systems programming language focused on safety.\n\
                    It guarantees memory safety without a garbage collector.";
        let out = squeeze_prose(text, 10_000);
        // Near-duplicate line dropped.
        assert_eq!(out.matches("focused on safety").count(), 1);
        // Blank run collapsed to at most a single blank line.
        assert!(!out.contains("\n\n\n"));
        assert!(out.contains("memory safety"));
    }

    #[test]
    fn squeeze_prose_keeps_protected_lines() {
        let text = "- [Home](https://x.com)\n- [Home](https://x.com)\n\
                    > A quote that repeats.\n> A quote that repeats.";
        let out = squeeze_prose(text, 10_000);
        // Protected (link/quote) lines are never deduped away.
        assert_eq!(out.matches("[Home]").count(), 2);
        assert_eq!(out.matches("A quote that repeats").count(), 2);
    }

    #[test]
    fn squeeze_prose_caps_length() {
        let big = "This is a unique sentence number ";
        let text = (0..500)
            .map(|i| format!("{big}{i}."))
            .collect::<Vec<_>>()
            .join("\n");
        let out = squeeze_prose(&text, 400);
        assert!(out.contains("…[truncated]"));
        assert!(out.len() <= 600, "got {} chars", out.len());
    }
}
