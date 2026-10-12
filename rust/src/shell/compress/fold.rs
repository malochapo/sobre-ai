//! Folding by shape, for the post-run hook: the parts of a shell output whose
//! form is known whatever command printed them, so it works on a line that
//! chains several commands too, where no single compressor owns the output.
//!
//! How much folds depends on `compression_level`:
//!
//! - **Log lines**, at every level that compresses. A log line starts, after
//!   the optional `path:` and line number `grep` prints, with a timestamp:
//!   ISO (`2025-08-03 12:00:01`, also after a level, as in `[INFO]
//!   [2025-08-03T12:00:01Z]`), syslog (`Aug  3 12:00:01`) or glog and klog
//!   (`I0803 12:00:01.123456`); or it is a JSON object with a time key. Only
//!   lines that are the same but for their timestamp fold together: an id, a
//!   count, an amount or a duration that differs keeps a line apart, since
//!   it may be what the agent is reading the log for. A line with a warning
//!   level or worse, or an error word (`warning`, `error`, `exception`,
//!   `failed`, …), never folds. Of the others, a line seen eight times or more
//!   in the output keeps its first and last occurrence, wherever the others
//!   were; a rarer one folds six or more in a row to their first and last. A
//!   marker says how many lines were folded.
//! - **Search headings**, from `Standard` (0.4) up. Consecutive
//!   `path:line:text` lines of one file print the path once, above them, as
//!   `rg --heading` does. No text is lost.
//! - **Long output**, at `Raw` (0.8 and up). An output of more than 150 lines
//!   keeps its first 60 and its last 30, unless a command of the line may
//!   print source: file content, a diff, blame or search results stay whole.
//!
//! No rule changes the text of a line that is not a log line. Search headings
//! move a path, and long output drops lines whole, saying how many. Every
//! folded output ends with the hook's `[sobre: …]` label, which says where the
//! full output was kept.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use crate::core::config::CompressionLevel;
use crate::core::tokens::count_tokens;

/// The fewest repeats of one log line in a row worth folding.
const LOG_RUN_MIN: usize = 6;
/// The fewest repeats of one log line in an output worth folding to their
/// first and last, wherever the others are.
const LOG_REPEAT_MIN: usize = 8;
const LONG_OUTPUT_LINES: usize = 150;
const LONG_OUTPUT_HEAD: usize = 60;
const LONG_OUTPUT_TAIL: usize = 30;
/// Below this saving a fold is not worth its label (`engine::MIN_USEFUL_SAVINGS`).
const MIN_SAVING_TOKENS: usize = 100;

/// `text` folded for `level`, or `None` when no rule saves enough.
/// `prints_source`: a command of the line may print source, so long output stays whole.
pub(crate) fn fold(text: &str, level: CompressionLevel, prints_source: bool) -> Option<String> {
    if level == CompressionLevel::Off {
        return None;
    }
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines = fold_log_lines(&lines);
    if level >= CompressionLevel::Standard {
        lines = search_headings(&lines);
    }
    if level >= CompressionLevel::Raw && !prints_source {
        lines = cut_long_output(lines);
    }
    let folded = lines.join("\n");
    let saving = count_tokens(text).saturating_sub(count_tokens(&folded));
    (saving >= MIN_SAVING_TOKENS).then_some(folded)
}

/// A log line: `grep`'s optional path and line number, then what the logger
/// printed (`log`), from its timestamp. `iso`, `syslog` and `glog` are the
/// timestamp; `signal` is glog's level letter for a warning or worse; `json`
/// opens a JSON line, whose timestamp is read by [`json_time`].
fn log_line() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"^(?:[\w./@+-]+[:-])?(?:\d+[:-])?(?P<log>",
            // ISO, after an optional level: `INFO 2025-08-03 12:00:01,123`,
            // `[INFO] [2025-08-03T12:00:01Z]`; then a space or a colon, since
            // after a comma or a pipe it is a table's first cell.
            r"(?:\[?(?i:trace|debug|info|notice|warn(?:ing)?|err(?:or)?|fatal|crit(?:ical)?|severe|alert|emerg|panic)\]?[ \t]+)?",
            r"\[?(?P<iso>\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?)\]?[\s:]",
            // syslog: `Aug  3 12:00:01 host app[123]:`
            r"|(?P<syslog>(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) [ \d]\d \d{2}:\d{2}:\d{2}(?:\.\d+)?) ",
            // glog and klog: `I0803 12:00:01.123456  1234 file.go:12]`
            r"|(?:I|(?P<signal>[WEF]))(?P<glog>\d{4} \d{2}:\d{2}:\d{2}\.\d{6})[ \t]+\d+ [\w.-]+:\d+\]",
            // a JSON object with a time key
            r#"|(?P<json>\{)\s*(?:".*)?"(?:ts|time|timestamp|@timestamp|t)"\s*:"#,
            ")",
        ))
        .expect("valid log line regex")
    })
}

/// What a log line is read for: a warning level or worse, or an error word.
/// Such a line never folds, however often its statement repeats: it is the
/// line an agent searching a log asked for. `"level": 40` and up is a warning
/// or worse to pino and bunyan; glog's level letter is read by [`log_line`].
fn signal() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"(?i)\b(?:warn|warning|error|err|fatal|crit|critical|severe|alert|emerg",
            r"|exception|traceback|fail|failed|failure|fails|panic)\b",
            r#"|"level"\s*:\s*[456]0\b"#,
        ))
        .expect("valid signal regex")
    })
}

/// A JSON log line's timestamp: the value of its time key.
fn json_time() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?P<key>"(?:ts|time|timestamp|@timestamp|t)"\s*:\s*)(?:"[^"]*"|-?\d+(?:\.\d+)?)"#,
        )
        .expect("valid JSON time regex")
    })
}

/// What a routine log line says: what the logger printed, its timestamp
/// masked, so that its repeats compare equal and nothing else does. `None` for
/// a line that is not a log line, and for a [`signal`] line, which never folds.
fn log_template(line: &str) -> Option<String> {
    let caps = log_line().captures(line)?;
    let log = caps.name("log")?;
    let logged = &line[log.start()..];
    if caps.name("signal").is_some() || signal().is_match(logged) {
        return None;
    }
    if caps.name("json").is_some() {
        return Some(json_time().replace_all(logged, "${key}#").into_owned());
    }
    let time = ["iso", "syslog", "glog"]
        .into_iter()
        .find_map(|name| caps.name(name))?;
    Some(format!(
        "{}#{}",
        &line[log.start()..time.start()],
        &line[time.end()..]
    ))
}

/// Routine log lines folded, every other line kept in its place. A line
/// repeated [`LOG_REPEAT_MIN`] times keeps its first and last occurrence, and
/// the marker after the first counts the others, wherever they were; a rarer
/// one folds a run of [`LOG_RUN_MIN`] repeats in a row to its first and last.
/// The first rule owns every repeat of its lines, so none is folded twice.
fn fold_log_lines(lines: &[String]) -> Vec<String> {
    let templates: Vec<Option<String>> = lines.iter().map(|l| log_template(l)).collect();
    // Each template's first line, last line and number of lines.
    let mut seen: HashMap<&str, (usize, usize, usize)> = HashMap::new();
    for (i, template) in templates.iter().enumerate() {
        if let Some(template) = template {
            let entry = seen.entry(template.as_str()).or_insert((i, i, 0));
            entry.1 = i;
            entry.2 += 1;
        }
    }
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let Some(template) = &templates[i] else {
            out.push(lines[i].clone());
            i += 1;
            continue;
        };
        let (first, last, count) = seen[template.as_str()];
        if count >= LOG_REPEAT_MIN {
            if i == first {
                out.push(lines[i].clone());
                // All in a row, they read as a run.
                out.push(if last - first + 1 == count {
                    format!("[sobre: {} similar log lines folded]", count - 2)
                } else {
                    format!(
                        "[sobre: {} more lines like the one above folded; the last one is kept below]",
                        count - 2
                    )
                });
            } else if i == last {
                out.push(lines[i].clone());
            }
            i += 1;
            continue;
        }
        let mut end = i + 1;
        while end < lines.len() && templates[end] == templates[i] {
            end += 1;
        }
        let run = end - i;
        if run >= LOG_RUN_MIN {
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

fn search_headings(lines: &[String]) -> Vec<String> {
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
                    "2025-07-28T{:02}:{:02}:34.{:03}Z scheduler-46x2cb2h2-dq7xn DEBUG:dispatch.scheduler: no delayed incident reports to remind",
                    i / 12,
                    (i % 12) * 5,
                    i * 7 % 1000
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One routine line, `i` its `i`th repeat.
    fn heartbeat(i: usize) -> String {
        format!(
            "2025-08-03T12:{:02}:{:02}.{:03}Z worker-7f9c2 INFO health: heartbeat ok, queue empty",
            i / 60,
            i % 60,
            i * 37 % 1000
        )
    }

    fn log(n: usize, line: impl Fn(usize) -> String) -> Vec<String> {
        (0..n).map(line).collect()
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
    fn a_run_of_one_log_line_keeps_its_first_and_last_repeat() {
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
            .collect::<Vec<_>>()
            .concat();
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
    fn a_run_folds_from_six_lines() {
        let mut lines: Vec<String> = scheduler_log(5).lines().map(str::to_string).collect();
        lines.push(
            "2025-07-28T03:00:00.000Z scheduler-46x2cb2h2-dq7xn INFO:Case close reminder sent"
                .into(),
        );
        assert_eq!(fold_log_lines(&lines), lines);
        let six: Vec<String> = scheduler_log(6).lines().map(str::to_string).collect();
        assert_eq!(
            fold_log_lines(&six),
            [
                six[0].clone(),
                "[sobre: 4 similar log lines folded]".into(),
                six[5].clone()
            ]
        );
    }

    #[test]
    fn signal_lines_never_fold() {
        let warnings = log(10, |i| {
            format!(
                "2025-08-03 12:00:{i:02},120 WARNING costsync.checks: account {i} is over its budget"
            )
        });
        assert_eq!(fold_log_lines(&warnings), warnings);
        for line in [
            "E0803 12:00:01.123456    4242 reflector.go:138] watch of *v1.Pod ended with: too old resource version",
            r#"{"ts":"2025-08-03T12:00:01Z","level":50,"msg":"upstream timed out"}"#,
            "[ERROR] [2025-08-03T12:00:01Z] sync: pull timed out",
            "Aug  3 12:00:01 web01 kernel: usb 1-1: device descriptor read/64, error -71",
            "2025-08-03 12:00:01,120 INFO sync: Traceback (most recent call last):",
        ] {
            assert_eq!(log_template(line), None, "{line}");
        }
        // An error among routine lines stays where it was.
        let mut lines = log(21, heartbeat);
        lines[10] = "2025-08-03T12:00:10.370Z worker-7f9c2 ERROR health: queue unreachable".into();
        assert_eq!(
            fold_log_lines(&lines),
            [
                lines[0].clone(),
                "[sobre: 18 more lines like the one above folded; the last one is kept below]"
                    .into(),
                lines[10].clone(),
                lines[20].clone()
            ]
        );
    }

    #[test]
    fn a_repeated_statement_keeps_its_first_and_last_line_wherever_the_others_are() {
        let jobs = [
            "backup", "reindex", "vacuum", "rotate", "export", "compact", "audit", "prune", "sync",
            "digest", "archive", "report",
        ];
        let job = |i: usize| {
            format!(
                "2025-08-03T12:00:{i:02}.500Z scheduler INFO jobs: {} started",
                jobs[i]
            )
        };
        let lines: Vec<String> = (0..jobs.len())
            .flat_map(|i| [heartbeat(i), job(i)])
            .collect();
        let mut expected = vec![
            heartbeat(0),
            "[sobre: 10 more lines like the one above folded; the last one is kept below]".into(),
        ];
        expected.extend((0..11).map(job));
        expected.extend([heartbeat(11), job(11)]);
        assert_eq!(fold_log_lines(&lines), expected);
    }

    #[test]
    fn each_log_format_folds() {
        let formats: [fn(usize) -> String; 6] = [
            |i| {
                format!(
                    "Aug  3 12:00:{i:02} web01 CRON[4100]: (root) CMD (run-parts /etc/cron.hourly)"
                )
            },
            |i| {
                format!(
                    "I0803 12:00:{i:02}.{:06}    4242 reflector.go:300] Watch close - *v1.Pod total 12 items received",
                    i * 7919
                )
            },
            |i| {
                format!(
                    r#"{{"level":"info","time":"2025-08-03T12:00:{i:02}.{:03}Z","msg":"request served","status":200}}"#,
                    i * 7
                )
            },
            |i| {
                format!(
                    "INFO 2025-08-03 12:00:{i:02},{:03} uvicorn.access: 10.0.0.7 - \"GET /health HTTP/1.1\" 200",
                    i * 7
                )
            },
            |i| format!("[INFO] [2025-08-03T12:00:{i:02}Z] sync: nothing to pull"),
            // The line number `grep` prints is not part of the line.
            |i| {
                format!(
                    "/var/log/syslog:{}:Aug  3 12:00:{i:02} web01 systemd[1]: Started session 4 of user deploy.",
                    100 + i * 4
                )
            },
        ];
        for line in formats {
            let lines = log(8, line);
            assert_eq!(
                fold_log_lines(&lines),
                [
                    lines[0].clone(),
                    "[sobre: 6 similar log lines folded]".into(),
                    lines[7].clone()
                ],
                "{}",
                lines[0]
            );
        }
    }

    /// Lines one statement printed with different values are different lines:
    /// the value may be what the agent searched the log for, as when it lists
    /// a sync's changes to find the first wrong one.
    #[test]
    fn lines_that_differ_by_more_than_their_timestamp_never_fold() {
        let diffs = log(30, |i| {
            format!(
                "[2025-09-02T18:20:04.{:03}+00:00] INFO costsync.diff - case {} classic {} -> {}",
                i * 13,
                5200 + i,
                15000 + i * 7,
                15100 + i * 11
            )
        });
        assert_eq!(fold_log_lines(&diffs), diffs);
        let served = log(30, |i| {
            format!(
                r#"{{"time":"2025-08-03T12:00:{i:02}Z","msg":"request served","duration_ms":{}}}"#,
                i * 3
            )
        });
        assert_eq!(fold_log_lines(&served), served);
        let watches = log(30, |i| {
            format!(
                "I0803 12:00:{i:02}.000000    4242 reflector.go:300] Watch close - *v1.Pod total {i} items received"
            )
        });
        assert_eq!(fold_log_lines(&watches), watches);
    }

    #[test]
    fn source_diffs_test_output_and_tables_are_not_log_lines() {
        let rust = r#"use std::time::Duration;

const STARTED: &str = "2025-08-03 12:00:01";

impl Retry {
    fn delay(&self, attempt: u32) -> Duration {
        log::info!("{} retry {attempt}", self.name);
        Duration::from_millis(100 * u64::from(attempt))
    }
}"#;
        let python = r#"import logging

logger = logging.getLogger(__name__)
LOG_FORMAT = "%(asctime)s %(levelname)s %(name)s: %(message)s"

def pull(records):
    for i, record in enumerate(records):
        logger.info("pulled record %d at %s", i, record["time"])
    return {"time": "2025-08-03T12:00:01Z", "count": len(records)}"#;
        // A log's diff too: each line starts with its `+`, `-` or space.
        let diff = r#"diff --git a/logs/sync.log b/logs/sync.log
index 3f2a1b4..9c8d7e6 100644
--- a/logs/sync.log
+++ b/logs/sync.log
@@ -1,3 +1,5 @@
 2025-08-03 12:00:01,120 INFO sync: pulled 3 records
-2025-08-03 12:00:02,120 INFO sync: pulled 4 records
+2025-08-03 12:00:02,120 INFO sync: pulled 5 records
+Aug  3 12:00:03 web01 sync[42]: pulled 6 records
+{"ts":"2025-08-03T12:00:04Z","msg":"pulled 7 records"}"#;
        let pytest = r"============================= test session starts ==============================
platform linux -- Python 3.12.3, pytest-8.3.2, pluggy-1.5.0
collected 3 items

tests/test_sync.py::test_pull PASSED                                     [ 33%]
tests/test_sync.py::test_push FAILED                                     [ 66%]
tests/test_sync.py::test_merge PASSED                                    [100%]
------------------------------ Captured log call -------------------------------
INFO     sync:sync.py:42 pulled 3 records
=========================== short test summary info ============================
FAILED tests/test_sync.py::test_push - AssertionError: 2025-08-03 12:00:01";
        let tables = r"| time | job | records |
|---|---|---|
| 2025-08-03 12:00:01 | sync | 3 |
time,job,records
2025-08-03 12:00:02,sync,4";
        for text in [rust, python, diff, pytest, tables] {
            for line in text.lines() {
                assert_eq!(log_template(line), None, "{line}");
            }
        }
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
            if line.starts_with("src/") {
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
        assert_eq!(search_headings(&lines), lines);
    }

    #[test]
    fn long_output_is_cut_only_at_raw_and_never_for_source() {
        let listing: String = (0..400)
            .map(|i| format!("./src/dispatch/module_{i}/__init__.py\n"))
            .collect::<Vec<_>>()
            .concat();
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
