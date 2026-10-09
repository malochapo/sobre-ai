//! Pure line-model engine for anchored edits: split → validate
//! anchors against a single preimage → reject overlaps → splice bottom-up.
//!
//! Everything here is a pure function of `(content, ops)` so it is exhaustively
//! unit-testable without touching the filesystem; the I/O wrapper in
//! [`super`] handles the read/guard/atomic-write around it.

use crate::core::anchor;

use super::anchors::{AnchorMiss, AnchorOp};

/// Outcome of validating the ops against the preimage lines.
pub(crate) enum ResolveError {
    /// One or more anchors did not match the current file (staleness/drift).
    Conflict(Vec<AnchorMiss>),
    /// A structurally invalid op (out-of-range line, overlap, empty insert, …).
    Invalid(String),
}

/// A validated edit, normalized to a 0-based splice over the line vector.
#[derive(Clone, Debug)]
pub(crate) struct ResolvedEdit {
    /// 0-based index where existing lines are replaced / new lines inserted.
    start_idx: usize,
    /// Number of existing lines removed (0 for a pure insert).
    remove_count: usize,
    /// Replacement lines (logical, no separators); empty = deletion.
    new_lines: Vec<String>,
    /// 1-based inclusive span the op depends on, for overlap detection.
    lo: usize,
    hi: usize,
}

/// Split `content` into logical lines and each line's own terminator
/// (`"\r\n"`, `"\n"`, or `""` for a last line without one), so a file with
/// mixed line endings is rebuilt byte for byte. Lines mirror [`str::lines`]
/// so line numbers match `sobre_read(mode="anchored")`.
pub(crate) fn split_lines(content: &str) -> (Vec<String>, Vec<&'static str>) {
    content
        .split_inclusive('\n')
        .map(|piece| {
            if let Some(line) = piece.strip_suffix("\r\n") {
                (line.to_string(), "\r\n")
            } else if let Some(line) = piece.strip_suffix('\n') {
                (line.to_string(), "\n")
            } else {
                (piece.to_string(), "")
            }
        })
        .unzip()
}

/// Rebuild file content from `lines` and their terminators.
pub(crate) fn join_lines(lines: &[String], eols: &[&str]) -> String {
    let mut out = String::new();
    for (line, eol) in lines.iter().zip(eols) {
        out.push_str(line);
        out.push_str(eol);
    }
    out
}

/// Split a `new_text` payload into logical replacement lines.
///
/// One trailing line separator is stripped (a habitual `"foo\n"` means the
/// single line `foo`, not `foo` + a blank). `""` → no lines (delete); `"\n"` →
/// one blank line.
fn split_new_text(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    let trimmed = s
        .strip_suffix("\r\n")
        .or_else(|| s.strip_suffix('\n'))
        .unwrap_or(s);
    trimmed
        .split('\n')
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect()
}

/// Validate every op's anchors against `lines` (the single preimage) and
/// normalize them to splices. Collects *all* stale anchors before failing so the
/// model gets the complete picture in one round-trip. An anchor that no longer
/// matches its line is a conflict, never moved to another line with the same
/// hash: the agent sees the current lines and decides.
pub(crate) fn resolve_ops(
    lines: &[String],
    ops: &[AnchorOp],
) -> Result<Vec<ResolvedEdit>, ResolveError> {
    let len = lines.len();
    let mut misses: Vec<AnchorMiss> = Vec::new();
    let mut edits: Vec<ResolvedEdit> = Vec::new();

    let check = |line: usize, hash: &str, misses: &mut Vec<AnchorMiss>| {
        // 1-based; line is guaranteed ≥1 by the caller for anchored ops.
        match lines.get(line - 1) {
            Some(cur) if anchor::hash_matches(cur, hash) => {}
            Some(cur) => misses.push(AnchorMiss {
                line,
                expected: hash.to_string(),
                actual: anchor::line_hash(cur),
            }),
            None => misses.push(AnchorMiss {
                line,
                expected: hash.to_string(),
                actual: "<eof>".to_string(),
            }),
        }
    };

    for op in ops {
        match op {
            AnchorOp::SetLine {
                line,
                hash,
                new_text,
            } => {
                if *line > len {
                    return Err(ResolveError::Invalid(format!(
                        "line {line} is past end of file ({len} lines)"
                    )));
                }
                let before = misses.len();
                check(*line, hash, &mut misses);
                if misses.len() == before {
                    edits.push(ResolvedEdit {
                        start_idx: line - 1,
                        remove_count: 1,
                        new_lines: split_new_text(new_text),
                        lo: *line,
                        hi: *line,
                    });
                }
            }
            AnchorOp::ReplaceLines {
                start_line,
                start_hash,
                end_line,
                end_hash,
                new_text,
            } => {
                if let Err(e) = check_range(*start_line, *end_line, len) {
                    return Err(ResolveError::Invalid(e));
                }
                // Empty hash = "no conflict check requested". This
                // makes replace_lines usable without mode=anchored output.
                let before = misses.len();
                if !start_hash.is_empty() {
                    check(*start_line, start_hash, &mut misses);
                }
                if !end_hash.is_empty() {
                    check(*end_line, end_hash, &mut misses);
                }
                if misses.len() == before {
                    edits.push(ResolvedEdit {
                        start_idx: start_line - 1,
                        remove_count: end_line - start_line + 1,
                        new_lines: split_new_text(new_text),
                        lo: *start_line,
                        hi: *end_line,
                    });
                }
            }
            AnchorOp::Delete {
                start_line,
                start_hash,
                end_line,
                end_hash,
            } => {
                if let Err(e) = check_range(*start_line, *end_line, len) {
                    return Err(ResolveError::Invalid(e));
                }
                let before = misses.len();
                check(*start_line, start_hash, &mut misses);
                check(*end_line, end_hash, &mut misses);
                if misses.len() == before {
                    edits.push(ResolvedEdit {
                        start_idx: start_line - 1,
                        remove_count: end_line - start_line + 1,
                        new_lines: Vec::new(),
                        lo: *start_line,
                        hi: *end_line,
                    });
                }
            }
            // Handled by `run_io` before the preimage read; reaching the line
            // model with a Create means it was mixed into a batch.
            AnchorOp::Create { .. } => {
                return Err(ResolveError::Invalid(
                    "create cannot be combined with anchored ops (new files have no preimage)"
                        .to_string(),
                ));
            }
            AnchorOp::InsertAfter {
                line,
                hash,
                new_text,
            } => {
                if *line > len {
                    return Err(ResolveError::Invalid(format!(
                        "insert_after line {line} is past end of file ({len} lines); \
                         use line={len} to append"
                    )));
                }
                let new_lines = split_new_text(new_text);
                if new_lines.is_empty() {
                    return Err(ResolveError::Invalid(
                        "insert_after needs non-empty new_text (use delete to remove lines)"
                            .to_string(),
                    ));
                }
                let before = misses.len();
                if let Some(h) = hash
                    && *line > 0
                {
                    check(*line, h, &mut misses);
                }
                if misses.len() == before {
                    edits.push(ResolvedEdit {
                        start_idx: *line,
                        remove_count: 0,
                        new_lines,
                        lo: *line,
                        hi: *line,
                    });
                }
            }
        }
    }

    if !misses.is_empty() {
        return Err(ResolveError::Conflict(misses));
    }
    if let Some(overlap) = first_overlap(&edits) {
        return Err(ResolveError::Invalid(overlap));
    }
    Ok(edits)
}

fn check_range(start: usize, end: usize, len: usize) -> Result<(), String> {
    if start > end {
        return Err(format!("start_line {start} is after end_line {end}"));
    }
    if end > len {
        return Err(format!("end_line {end} is past end of file ({len} lines)"));
    }
    Ok(())
}

/// Reject a batch where two edits touch overlapping line spans — their combined
/// result would be order-dependent. The model should split them or merge into a
/// single `replace_lines`. Returns a human-readable message for the first clash.
fn first_overlap(edits: &[ResolvedEdit]) -> Option<String> {
    for (i, a) in edits.iter().enumerate() {
        for b in &edits[i + 1..] {
            if a.lo <= b.hi && b.lo <= a.hi {
                return Some(format!(
                    "overlapping edits: lines {}-{} and {}-{} touch the same region — \
                     split into separate calls or merge into one replace_lines",
                    a.lo, a.hi, b.lo, b.hi
                ));
            }
        }
    }
    None
}

/// Apply validated `edits` to `lines` and their terminators `eols`, bottom-up
/// so earlier indices stay valid. Non-overlap is guaranteed by
/// [`resolve_ops`], so descending order is exact. At one index, an
/// `insert_after` of the line above and an edit of the line below meet: the
/// edit goes first, so the insert lands above it whatever the order of the
/// ops.
///
/// Untouched lines keep their terminator. A new line takes the one of the
/// first line it replaces, or of the line it follows, or the file's most
/// common one; the file ends with a newline exactly when it did before.
#[must_use]
pub(crate) fn apply_edits(
    mut lines: Vec<String>,
    mut eols: Vec<&'static str>,
    mut edits: Vec<ResolvedEdit>,
) -> (Vec<String>, Vec<&'static str>) {
    let crlf = eols.iter().filter(|e| **e == "\r\n").count();
    let lf = eols.iter().filter(|e| **e == "\n").count();
    let common = if crlf > lf { "\r\n" } else { "\n" };
    let trailing = eols.last().is_none_or(|e| !e.is_empty());

    edits.sort_by_key(|e| (std::cmp::Reverse(e.start_idx), e.remove_count == 0));
    for e in edits {
        let end = e.start_idx + e.remove_count;
        let neighbour = if e.remove_count > 0 {
            eols.get(e.start_idx)
        } else {
            e.start_idx.checked_sub(1).and_then(|i| eols.get(i))
        };
        let eol = neighbour
            .copied()
            .filter(|e| !e.is_empty())
            .unwrap_or(common);
        let added = vec![eol; e.new_lines.len()];
        lines.splice(e.start_idx..end, e.new_lines);
        eols.splice(e.start_idx..end, added);
    }
    // Only the last line may lack a terminator: a line that was last and no
    // longer is gets one, and the new last line follows the file's ending.
    for eol in &mut eols {
        if eol.is_empty() {
            *eol = common;
        }
    }
    if !trailing && let Some(last) = eols.last_mut() {
        *last = "";
    }
    (lines, eols)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anc(_line: usize, content: &str) -> String {
        // The (line, hash) a model would have been shown for `content`; `_line`
        // is kept at call sites only to read like a real anchor reference.
        anchor::line_hash(content)
    }

    /// `apply_edits` on lines alone, every line ending in `\n`.
    fn apply_lf(lines: Vec<String>, edits: Vec<ResolvedEdit>) -> Vec<String> {
        let eols = vec!["\n"; lines.len()];
        apply_edits(lines, eols, edits).0
    }

    #[test]
    fn split_and_join_round_trip() {
        for content in [
            "a\nb\nc\n",
            "a\nb",
            "a\r\nb\r\n",
            "a\r\nb\nc",
            "",
            "\n",
            "x\ry\n",
        ] {
            let (lines, eols) = split_lines(content);
            assert_eq!(
                lines,
                content.lines().map(String::from).collect::<Vec<_>>(),
                "{content:?}"
            );
            assert_eq!(join_lines(&lines, &eols), content, "{content:?}");
        }
    }

    /// J5: an edit keeps every other line's ending; new lines take their
    /// neighbour's, and the file's last-line ending stays as it was.
    #[test]
    fn mixed_line_endings_survive_an_edit() {
        let content = "a\r\nb\nc\r\nd";
        let (lines, eols) = split_lines(content);
        let ops = vec![
            AnchorOp::SetLine {
                line: 2,
                hash: anc(2, "b"),
                new_text: "B1\nB2".into(),
            },
            AnchorOp::InsertAfter {
                line: 4,
                hash: Some(anc(4, "d")),
                new_text: "e".into(),
            },
        ];
        let edits = resolve_ops(&lines, &ops).ok().unwrap();
        let (lines, eols) = apply_edits(lines, eols, edits);
        assert_eq!(join_lines(&lines, &eols), "a\r\nB1\nB2\nc\r\nd\r\ne");
    }

    #[test]
    fn split_new_text_strips_one_trailing_newline() {
        assert_eq!(split_new_text("foo"), vec!["foo"]);
        assert_eq!(split_new_text("foo\n"), vec!["foo"]);
        assert_eq!(split_new_text("foo\nbar"), vec!["foo", "bar"]);
        assert_eq!(split_new_text("foo\nbar\n"), vec!["foo", "bar"]);
        assert_eq!(split_new_text(""), Vec::<String>::new());
        assert_eq!(split_new_text("\n"), vec![""]);
    }

    #[test]
    fn set_line_replaces_in_place() {
        let lines = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let ops = vec![AnchorOp::SetLine {
            line: 2,
            hash: anc(2, "b"),
            new_text: "B".to_string(),
        }];
        let edits = resolve_ops(&lines, &ops).ok().unwrap();
        let out = apply_lf(lines, edits);
        assert_eq!(out, vec!["a", "B", "c"]);
    }

    #[test]
    fn set_line_empty_text_deletes() {
        let lines = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let ops = vec![AnchorOp::SetLine {
            line: 2,
            hash: anc(2, "b"),
            new_text: String::new(),
        }];
        let edits = resolve_ops(&lines, &ops).ok().unwrap();
        assert_eq!(apply_lf(lines, edits), vec!["a", "c"]);
    }

    #[test]
    fn set_line_expands_to_multiple_lines() {
        let lines = vec!["a".to_string(), "b".to_string()];
        let ops = vec![AnchorOp::SetLine {
            line: 1,
            hash: anc(1, "a"),
            new_text: "x\ny\nz".to_string(),
        }];
        let edits = resolve_ops(&lines, &ops).ok().unwrap();
        assert_eq!(apply_lf(lines, edits), vec!["x", "y", "z", "b"]);
    }

    #[test]
    fn replace_lines_collapses_range() {
        let lines = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let ops = vec![AnchorOp::ReplaceLines {
            start_line: 2,
            start_hash: anc(2, "b"),
            end_line: 3,
            end_hash: anc(3, "c"),
            new_text: "X".to_string(),
        }];
        let edits = resolve_ops(&lines, &ops).ok().unwrap();
        assert_eq!(apply_lf(lines, edits), vec!["a", "X", "d"]);
    }

    #[test]
    fn insert_after_line_zero_prepends() {
        let lines = vec!["a".to_string(), "b".to_string()];
        let ops = vec![AnchorOp::InsertAfter {
            line: 0,
            hash: None,
            new_text: "// top".to_string(),
        }];
        let edits = resolve_ops(&lines, &ops).ok().unwrap();
        assert_eq!(apply_lf(lines, edits), vec!["// top", "a", "b"]);
    }

    #[test]
    fn insert_after_last_line_appends() {
        let lines = vec!["a".to_string(), "b".to_string()];
        let ops = vec![AnchorOp::InsertAfter {
            line: 2,
            hash: Some(anc(2, "b")),
            new_text: "c".to_string(),
        }];
        let edits = resolve_ops(&lines, &ops).ok().unwrap();
        assert_eq!(apply_lf(lines, edits), vec!["a", "b", "c"]);
    }

    #[test]
    fn stale_anchor_is_reported_as_conflict() {
        let lines = vec!["a".to_string(), "b".to_string()];
        let ops = vec![AnchorOp::SetLine {
            line: 2,
            hash: "ffff".to_string(), // wrong hash
            new_text: "B".to_string(),
        }];
        match resolve_ops(&lines, &ops) {
            Err(ResolveError::Conflict(misses)) => {
                assert_eq!(misses.len(), 1);
                assert_eq!(misses[0].line, 2);
                assert_eq!(misses[0].actual, anchor::line_hash("b"));
            }
            _ => panic!("expected a Conflict for the stale anchor"),
        }
    }

    #[test]
    fn all_stale_anchors_collected() {
        let lines = vec!["a".to_string(), "b".to_string()];
        let ops = vec![
            AnchorOp::SetLine {
                line: 1,
                hash: "0000".into(),
                new_text: "A".into(),
            },
            AnchorOp::SetLine {
                line: 2,
                hash: "1111".into(),
                new_text: "B".into(),
            },
        ];
        match resolve_ops(&lines, &ops) {
            Err(ResolveError::Conflict(misses)) => assert_eq!(misses.len(), 2),
            _ => panic!("expected both misses collected"),
        }
    }

    #[test]
    fn overlapping_edits_rejected() {
        let lines = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let ops = vec![
            AnchorOp::SetLine {
                line: 2,
                hash: anc(2, "b"),
                new_text: "B".into(),
            },
            AnchorOp::ReplaceLines {
                start_line: 1,
                start_hash: anc(1, "a"),
                end_line: 2,
                end_hash: anc(2, "b"),
                new_text: "X".into(),
            },
        ];
        match resolve_ops(&lines, &ops) {
            Err(ResolveError::Invalid(msg)) => assert!(msg.contains("overlapping")),
            _ => panic!("expected overlap rejection"),
        }
    }

    #[test]
    fn out_of_range_line_rejected() {
        let lines = vec!["a".to_string()];
        let ops = vec![AnchorOp::SetLine {
            line: 5,
            hash: "aa".into(),
            new_text: "x".into(),
        }];
        assert!(matches!(
            resolve_ops(&lines, &ops),
            Err(ResolveError::Invalid(_))
        ));
    }

    #[test]
    fn op_input_order_does_not_change_result() {
        // Determinism: a batch validated against one preimage must be a
        // pure function of the *set* of ops — input order is irrelevant because
        // application is sorted bottom-up. Guards against accidental
        // order-sensitivity creeping into resolve/apply.
        let lines: Vec<String> = ["1", "2", "3", "4", "5"]
            .iter()
            .map(ToString::to_string)
            .collect();
        let a = AnchorOp::SetLine {
            line: 1,
            hash: anc(1, "1"),
            new_text: "A\nA2".into(),
        };
        let b = AnchorOp::ReplaceLines {
            start_line: 3,
            start_hash: anc(3, "3"),
            end_line: 4,
            end_hash: anc(4, "4"),
            new_text: "B".into(),
        };
        let c = AnchorOp::InsertAfter {
            line: 5,
            hash: Some(anc(5, "5")),
            new_text: "C".into(),
        };

        let forward = apply_lf(
            lines.clone(),
            resolve_ops(&lines, &[a.clone(), b.clone(), c.clone()])
                .ok()
                .unwrap(),
        );
        let reversed = apply_lf(lines.clone(), resolve_ops(&lines, &[c, b, a]).ok().unwrap());
        assert_eq!(forward, reversed);
        assert_eq!(forward, vec!["A", "A2", "2", "B", "5", "C"]);
    }

    #[test]
    fn batch_bottom_up_keeps_line_numbers_valid() {
        // Two independent edits; applying top-down would shift the 2nd. Bottom-up
        // (handled by apply_edits) keeps both correct.
        let lines = vec![
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
            "5".to_string(),
        ];
        let ops = vec![
            AnchorOp::ReplaceLines {
                start_line: 1,
                start_hash: anc(1, "1"),
                end_line: 2,
                end_hash: anc(2, "2"),
                new_text: "A".into(), // 2 lines → 1 line (shifts later indices)
            },
            AnchorOp::SetLine {
                line: 5,
                hash: anc(5, "5"),
                new_text: "E".into(),
            },
        ];
        let edits = resolve_ops(&lines, &ops).ok().unwrap();
        assert_eq!(apply_lf(lines, edits), vec!["A", "3", "4", "E"]);
    }

    /// J2 (decision 1): an anchor whose line moved is a conflict, for every
    /// op, even when the anchored text sits unchanged one line below.
    #[test]
    fn a_shifted_anchor_is_a_conflict_never_a_move() {
        let lines: Vec<String> = ["intro", "filler", "start", "end", "tail"]
            .iter()
            .map(ToString::to_string)
            .collect();
        // Each op still points at lines 2-3, where "start"/"end" were before
        // a line was inserted above them.
        let ops = [
            AnchorOp::SetLine {
                line: 2,
                hash: anc(2, "start"),
                new_text: "X".into(),
            },
            AnchorOp::InsertAfter {
                line: 2,
                hash: Some(anc(2, "start")),
                new_text: "X".into(),
            },
            AnchorOp::ReplaceLines {
                start_line: 2,
                start_hash: anc(2, "start"),
                end_line: 3,
                end_hash: anc(3, "end"),
                new_text: "X".into(),
            },
            AnchorOp::Delete {
                start_line: 2,
                start_hash: anc(2, "start"),
                end_line: 3,
                end_hash: anc(3, "end"),
            },
        ];
        for op in ops {
            assert!(
                matches!(
                    resolve_ops(&lines, std::slice::from_ref(&op)),
                    Err(ResolveError::Conflict(_))
                ),
                "{op:?} must be a conflict"
            );
        }
    }

    /// J1: `insert_after` line L and an edit of line L+1 give the same file
    /// whatever their order in `ops`.
    #[test]
    fn insert_after_and_an_edit_below_commute() {
        let lines: Vec<String> = ["a", "b", "c"].iter().map(ToString::to_string).collect();
        let insert = AnchorOp::InsertAfter {
            line: 1,
            hash: Some(anc(1, "a")),
            new_text: "new".into(),
        };
        for below in [
            AnchorOp::SetLine {
                line: 2,
                hash: anc(2, "b"),
                new_text: "B".into(),
            },
            AnchorOp::Delete {
                start_line: 2,
                start_hash: anc(2, "b"),
                end_line: 2,
                end_hash: anc(2, "b"),
            },
        ] {
            let expected = if matches!(below, AnchorOp::Delete { .. }) {
                vec!["a", "new", "c"]
            } else {
                vec!["a", "new", "B", "c"]
            };
            for ops in [
                vec![insert.clone(), below.clone()],
                vec![below.clone(), insert.clone()],
            ] {
                let edits = resolve_ops(&lines, &ops).ok().unwrap();
                assert_eq!(apply_lf(lines.clone(), edits), expected, "{ops:?}");
            }
        }
    }
}
