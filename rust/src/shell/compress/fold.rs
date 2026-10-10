//! Folding by shape, for the post-run hook: the parts of a shell output whose
//! form is known whatever command printed them, so it works on a line that
//! chains several commands too, where no single compressor owns the output.
//!
//! How much folds depends on `compression_level`:
//!
//! - **Log runs**, at every level that compresses. Log lines are lines that
//!   start with a timestamp, after the optional `path:` and line number `grep`
//!   prints. Four or more in a row that differ only in their timestamps,
//!   numbers and ids keep their first and last line, and a marker says how
//!   many were folded between them.
//! - **Search headings**, from `Standard` (0.4) up. Consecutive
//!   `path:line:text` lines of one file print the path once, above them, as
//!   `rg --heading` does. No text is lost.
//! - **Long output**, at `Raw` (0.8 and up). An output of more than 150 lines
//!   keeps its first 60 and its last 30, unless a command of the line read a
//!   file: file content always stays whole.
//!
//! No rule changes the text of a line that is not a log line. Search headings
//! move a path, and long output drops lines whole, saying how many. Every
//! folded output ends with the hook's `[sobre: …]` label, which says where the
//! full output was kept.

use std::sync::OnceLock;

use regex::Regex;

use crate::core::config::CompressionLevel;
use crate::core::tokens::count_tokens;

/// The fewest log lines in a row worth folding: first + marker + last is three.
const LOG_RUN_MIN: usize = 4;
const LONG_OUTPUT_LINES: usize = 150;
const LONG_OUTPUT_HEAD: usize = 60;
const LONG_OUTPUT_TAIL: usize = 30;
/// Below this saving a fold is not worth its label (`engine::MIN_USEFUL_SAVINGS`).
const MIN_SAVING_TOKENS: usize = 100;

/// `text` folded for `level`, or `None` when no rule saves enough.
/// `reads_files`: a command of the line printed a file, so long output stays whole.
pub(crate) fn fold(text: &str, level: CompressionLevel, reads_files: bool) -> Option<String> {
    if level == CompressionLevel::Off {
        return None;
    }
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines = fold_log_runs(lines);
    if level >= CompressionLevel::Standard {
        lines = search_headings(lines);
    }
    if level >= CompressionLevel::Raw && !reads_files {
        lines = cut_long_output(lines);
    }
    let folded = lines.join("\n");
    let saving = count_tokens(text).saturating_sub(count_tokens(&folded));
    (saving >= MIN_SAVING_TOKENS).then_some(folded)
}

/// A log line's optional `grep` prefix and its timestamp.
fn log_prefix() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?:[\w./@+-]+[:-])?(?:\d+[:-])?\[?\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?\]?",
        )
        .expect("valid log prefix regex")
    })
}

/// What differs between two lines one log statement printed: numbers, hex ids, UUIDs.
fn variable_part() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)0x[0-9a-f]+|\b[0-9a-f]{7,}\b|[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}|\d+(?:\.\d+)?(?:e-?\d+)?")
            .expect("valid variable-part regex")
    })
}

/// The statement a log line comes from: the line without its prefix, timestamp
/// and variable parts. `None` for a line that is not a log line.
fn log_template(line: &str) -> Option<String> {
    let prefix = log_prefix().find(line)?;
    Some(
        variable_part()
            .replace_all(&line[prefix.end()..], "#")
            .into_owned(),
    )
}

fn fold_log_runs(lines: Vec<String>) -> Vec<String> {
    let templates: Vec<Option<String>> = lines.iter().map(|l| log_template(l)).collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let mut end = i + 1;
        if templates[i].is_some() {
            while end < lines.len() && templates[end] == templates[i] {
                end += 1;
            }
        }
        let run = end - i;
        if templates[i].is_some() && run >= LOG_RUN_MIN {
            out.push(lines[i].clone());
            out.push(format!("[sobre: {} similar log lines folded]", run - 2));
            out.push(lines[end - 1].clone());
        } else {
            out.extend(lines[i..end].iter().cloned());
        }
        i = end;
    }
    out
}

/// A `grep -n` result line: path, line number and text, with `:` after a
/// match's path and number and `-` after a context line's.
fn search_line() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^((?:\./)?[\w@.+-]+(?:/[\w@.+-]+)*\.\w+)([:-])(\d+)([:-])")
            .expect("valid search line regex")
    })
}

/// `(path, the line without its path)` for a result line whose two separators agree.
fn split_search_line(line: &str) -> Option<(&str, &str)> {
    let caps = search_line().captures(line)?;
    if caps[2] != caps[4] {
        return None;
    }
    let path = caps.get(1)?;
    Some((path.as_str(), &line[path.end() + 1..]))
}

fn search_headings(lines: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let Some((path, _)) = split_search_line(&lines[i]) else {
            out.push(lines[i].clone());
            i += 1;
            continue;
        };
        // The run of this file's lines; `--` separates context groups inside it.
        let mut end = i;
        let mut results = 0;
        while end < lines.len() {
            match split_search_line(&lines[end]) {
                Some((p, _)) if p == path => results += 1,
                None if lines[end] == "--" && end + 1 < lines.len() => {
                    if split_search_line(&lines[end + 1]).is_none_or(|(p, _)| p != path) {
                        break;
                    }
                }
                _ => break,
            }
            end += 1;
        }
        if results >= 2 {
            out.push(path.to_string());
            out.extend(lines[i..end].iter().map(|l| {
                split_search_line(l).map_or_else(|| l.clone(), |(_, rest)| rest.to_string())
            }));
        } else {
            out.extend(lines[i..end].iter().cloned());
        }
        i = end;
    }
    out
}

fn cut_long_output(lines: Vec<String>) -> Vec<String> {
    if lines.len() <= LONG_OUTPUT_LINES {
        return lines;
    }
    let omitted = lines.len() - LONG_OUTPUT_HEAD - LONG_OUTPUT_TAIL;
    let mut out = lines[..LONG_OUTPUT_HEAD].to_vec();
    out.push(format!("[sobre: {omitted} lines omitted]"));
    out.extend(lines[lines.len() - LONG_OUTPUT_TAIL..].iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler_log(n: usize) -> String {
        (0..n)
            .map(|i| {
                format!(
                    "2025-07-28T{:02}:{:02}:34.{:03}Z scheduler-46x2cb2h2-dq7xn DEBUG:function.elapsed.time.dispatch.report.scheduled.incident_report_delayed_reminders: 0.0{}8234:/usr/local/lib/python3.11/site-packages/dispatch/decorators.py:wrapper:147",
                    i / 12,
                    (i % 12) * 5,
                    i * 7 % 1000,
                    i
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn python_source() -> String {
        (0..60)
            .map(|i| {
                format!("    value_{i} = compute_something(argument_{i}, other_{i})  # step {i}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_run_of_one_log_statement_keeps_its_first_and_last_line() {
        let log = scheduler_log(40);
        let folded = fold(&log, CompressionLevel::Lite, false).expect("folds");
        let lines: Vec<&str> = folded.lines().collect();
        assert_eq!(lines.len(), 3, "{folded}");
        assert_eq!(lines[0], log.lines().next().unwrap());
        assert_eq!(lines[1], "[sobre: 38 similar log lines folded]");
        assert_eq!(lines[2], log.lines().last().unwrap());
    }

    #[test]
    fn grep_prefixes_and_other_lines_stay() {
        let grepped: String = scheduler_log(30)
            .lines()
            .enumerate()
            .map(|(n, l)| format!("{}:{l}\n", n * 3 + 10))
            .collect();
        let text =
            format!("  19852 api.log\n  27621 scheduler.log\n{grepped}ERROR: something else\n");
        let folded = fold(&text, CompressionLevel::Lite, false).expect("folds");
        assert!(
            folded.starts_with("  19852 api.log\n  27621 scheduler.log\n10:2025-07-28T00:00:34")
        );
        assert!(folded.contains("[sobre: 28 similar log lines folded]"));
        assert!(folded.ends_with("ERROR: something else"));
    }

    #[test]
    fn three_log_lines_and_different_statements_stay() {
        let mut lines: Vec<String> = scheduler_log(3).lines().map(str::to_string).collect();
        lines.push(
            "2025-07-28T03:00:00.000Z scheduler-46x2cb2h2-dq7xn ERROR:Case close reminder failed"
                .into(),
        );
        assert_eq!(fold_log_runs(lines.clone()), lines);
    }

    #[test]
    fn source_code_is_never_reshaped() {
        let source = python_source();
        for level in [
            CompressionLevel::Lite,
            CompressionLevel::Standard,
            CompressionLevel::Max,
            CompressionLevel::Raw,
        ] {
            assert_eq!(fold(&source, level, true), None, "{level:?}");
        }
    }

    #[test]
    fn search_headings_lose_nothing_and_start_at_standard() {
        let hits: String = (0..30)
            .map(|i| format!("src/dispatch/signal/service.py:{}:    if signal_filter.action == SignalFilterAction.snooze:  # {i}\n", 100 + i))
            .chain(["--\n".to_string(), "src/dispatch/signal/service.py-200-    return None\n".to_string()])
            .chain((0..30).map(|i| format!("src/dispatch/case/flows.py:{}:    case.status = CaseStatus.closed  # {i}\n", 10 + i)))
            .collect();
        assert_eq!(
            fold(&hits, CompressionLevel::Lite, false),
            None,
            "Lite leaves search results alone"
        );
        let folded = fold(&hits, CompressionLevel::Standard, false).expect("folds");
        // Rebuild the original from the heading form.
        let mut path = String::new();
        let mut rebuilt = String::new();
        for line in folded.lines() {
            if line.ends_with(".py") {
                path = line.to_string();
            } else if line == "--" {
                rebuilt.push_str("--\n");
            } else {
                let sep = if line
                    .split_once(':')
                    .is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()))
                {
                    ':'
                } else {
                    '-'
                };
                rebuilt.push_str(&format!("{path}{sep}{line}\n"));
            }
        }
        assert_eq!(rebuilt, hits);
    }

    #[test]
    fn a_single_hit_keeps_its_path() {
        let lines = vec![
            "src/a.py:1:x = 1".to_string(),
            "src/b.py:2:y = 2".to_string(),
        ];
        assert_eq!(search_headings(lines.clone()), lines);
    }

    #[test]
    fn long_output_is_cut_only_at_raw_and_never_for_file_content() {
        let listing: String = (0..400)
            .map(|i| format!("./src/dispatch/module_{i}/__init__.py\n"))
            .collect();
        assert_eq!(fold(&listing, CompressionLevel::Max, false), None);
        assert_eq!(fold(&listing, CompressionLevel::Raw, true), None);
        let cut = fold(&listing, CompressionLevel::Raw, false).expect("cuts");
        let lines: Vec<&str> = cut.lines().collect();
        assert_eq!(lines.len(), LONG_OUTPUT_HEAD + 1 + LONG_OUTPUT_TAIL);
        assert_eq!(lines[LONG_OUTPUT_HEAD], "[sobre: 310 lines omitted]");
        assert_eq!(lines.last(), Some(&"./src/dispatch/module_399/__init__.py"));
    }

    #[test]
    fn off_folds_nothing() {
        assert_eq!(fold(&scheduler_log(40), CompressionLevel::Off, false), None);
    }
}
