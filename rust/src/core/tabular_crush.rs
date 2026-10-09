//! Deterministic tabular (CSV/TSV) crusher — columnar redundancy factoring for
//! delimited data (a port of Headroom's tabular compressor).
//!
//! Real-world CSV/TSV dumps (DB exports, `psql -A -F,`, analytics extracts) are
//! dominated by columns that repeat one value on every row (a `status`, `region`,
//! `tenant` column). This module factors that out, mirroring
//! [`crate::core::json_crush`] but for tables: every *constant* column (exactly
//! one distinct value across all data rows) is hoisted once into `_const`; the
//! remaining columns are kept positionally in `_rows`, so a value never repeats
//! more than it must. The transform is exactly reversible via `reconstruct`
//! (test-only).
//!
//! The compact form is serialized with `serde_json` (robust quoting, escapes and
//! Unicode — no hand-rolled format that could desync the reader), and the output
//! is a pure function of the input (columns walked in header order, `_const`
//! keyed deterministically), so identical input yields byte-identical output.
//! The crusher never inflates: the result is kept only when it clears the
//! `beneficial` gate, and a no-op input returns [`None`].

use std::collections::BTreeSet;

use serde_json::{Map, Value};

/// Parse delimited text into rows of fields (RFC-4180 quoting aware).
#[must_use]
fn parse(input: &str, delimiter: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut field = String::new();
    let mut record: Vec<String> = Vec::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
        } else if ch == '"' && field.is_empty() {
            // Only a quote that opens a field quotes it; elsewhere it is text.
            in_quotes = true;
        } else if ch == delimiter {
            record.push(std::mem::take(&mut field));
        } else if ch == '\n' || ch == '\r' {
            // Swallow a CRLF pair as one terminator.
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            record.push(std::mem::take(&mut field));
            push_record(&mut rows, std::mem::take(&mut record));
        } else {
            field.push(ch);
        }
    }
    // Trailing field/record (no final newline).
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        push_record(&mut rows, record);
    }
    rows
}

/// Append a record unless it is a blank line (one empty field). A record of
/// several empty fields (`,,`) is data and is kept.
fn push_record(rows: &mut Vec<Vec<String>>, record: Vec<String>) {
    if record.len() > 1 || record.first().is_some_and(|f| !f.trim().is_empty()) {
        rows.push(record);
    }
}

/// Marks a crushed-table document. Vanishingly unlikely as a real top-level CSV
/// shape (the input is delimited text, never a JSON object), so `reconstruct` (test-only)
/// can identify our own output unambiguously.
const MARKER: &str = "_lc_tbl";
/// Full header, in original column order — the source of truth for reconstructing
/// each row's column sequence.
const ORDER_KEY: &str = "_order";
/// Columns hoisted to their single repeated value.
const CONST_KEY: &str = "_const";
/// Per-row values for the *varying* columns, positional in header order.
const ROWS_KEY: &str = "_rows";

/// Below this data-row count, columnar factoring rarely beats its own overhead.
const MIN_ROWS: usize = 3;

/// Lossless columnar crush of delimited `text`, returning the compact JSON form
/// only when it clears the `beneficial` reduction gate. `None` for non-tabular,
/// ragged, or low-redundancy input — the caller keeps its own path.
pub fn crush_text_if_beneficial(text: &str, delimiter: char) -> Option<String> {
    let crushed = crush(text, delimiter)?;
    beneficial(&crushed, text).then_some(crushed)
}

/// Tabular reshaping into the columnar JSON form carries per-cell quoting
/// overhead the JSON crusher's array-of-objects form does not, so the columnar
/// win (eliminating constant columns) is gated on a 1/4 byte reduction rather
/// than the JSON crusher's stricter halving — still a clear, never-inflating win
/// whose exact data stays reconstructible via `reconstruct` (test-only).
const MIN_SAVE_RATIO_NUM: usize = 3;
const MIN_SAVE_RATIO_DEN: usize = 4;

fn beneficial(compact: &str, raw: &str) -> bool {
    compact.len().saturating_mul(MIN_SAVE_RATIO_DEN) <= raw.len().saturating_mul(MIN_SAVE_RATIO_NUM)
}

/// Core crush. Returns `None` unless the input is a well-formed table with at
/// least one constant column.
fn crush(text: &str, delimiter: char) -> Option<String> {
    // JSON is the JSON crusher's job; never treat it as a degenerate one-column
    // table (and our own output starts with `{`, so this is also re-entry-safe).
    let head = text.trim_start();
    if head.starts_with('{') || head.starts_with('[') {
        return None;
    }

    let rows = parse(text, delimiter);
    if rows.len() < MIN_ROWS + 1 {
        return None; // need a header + >= MIN_ROWS data rows
    }
    let header = &rows[0];
    let ncols = header.len();
    if ncols < 2 {
        return None; // a single column has nothing to factor across
    }
    let data = &rows[1..];

    // Only rectangular tables: a ragged row makes positional reconstruction
    // ambiguous, so fall through to the generic path.
    if data.iter().any(|r| r.len() != ncols) {
        return None;
    }
    // Distinct headers only: a duplicate name would alias in keyed `_const`.
    let mut header_seen = BTreeSet::new();
    if !header.iter().all(|h| header_seen.insert(h.as_str())) {
        return None;
    }

    let mut const_map = Map::new();
    let mut kept: Vec<usize> = Vec::new();
    for (c, name) in header.iter().enumerate() {
        let constant = data.iter().all(|row| row[c] == data[0][c]);
        if constant {
            const_map.insert(name.clone(), Value::String(data[0][c].clone()));
        } else {
            kept.push(c);
        }
    }
    if const_map.is_empty() {
        return None; // nothing factored — leave the table to the generic path
    }

    let rows_json: Vec<Value> = data
        .iter()
        .map(|row| {
            Value::Array(
                kept.iter()
                    .map(|&c| Value::String(row[c].clone()))
                    .collect(),
            )
        })
        .collect();

    let mut out = Map::new();
    out.insert(MARKER.to_string(), Value::from(1u8));
    out.insert(
        ORDER_KEY.to_string(),
        Value::Array(header.iter().cloned().map(Value::String).collect()),
    );
    out.insert(CONST_KEY.to_string(), Value::Object(const_map));
    out.insert(ROWS_KEY.to_string(), Value::Array(rows_json));
    serde_json::to_string(&Value::Object(out)).ok()
}

/// Rebuild the parsed rows (`[header, ..data]`) from crushed `text`. `None` if
/// `text` is not a tabular-crush document or is internally inconsistent.
#[cfg(test)]
pub fn reconstruct(text: &str) -> Option<Vec<Vec<String>>> {
    let v: Value = serde_json::from_str(text).ok()?;
    let obj = v.as_object()?;
    obj.get(MARKER)?;

    let order: Vec<String> = obj
        .get(ORDER_KEY)?
        .as_array()?
        .iter()
        .map(|x| x.as_str().unwrap_or_default().to_string())
        .collect();
    let empty = Map::new();
    let const_map = obj
        .get(CONST_KEY)
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let rows = obj.get(ROWS_KEY)?.as_array()?;

    // The positional `_rows` columns are the header minus the hoisted
    // constants — exactly the `kept` order the crush emitted.
    let out_cols: Vec<&str> = order.iter().map(String::as_str).collect();
    let varying: Vec<&str> = out_cols
        .iter()
        .copied()
        .filter(|c| !const_map.contains_key(*c))
        .collect();

    let mut result: Vec<Vec<String>> = Vec::with_capacity(rows.len() + 1);
    result.push(out_cols.iter().map(|c| (*c).to_string()).collect());

    for row in rows {
        let arr = row.as_array()?;
        if arr.len() != varying.len() {
            return None;
        }
        let vary_vals: std::collections::BTreeMap<&str, &str> = varying
            .iter()
            .copied()
            .zip(arr.iter().map(|x| x.as_str().unwrap_or_default()))
            .collect();
        let mut full = Vec::with_capacity(out_cols.len());
        for col in &out_cols {
            let cell = const_map
                .get(*col)
                .and_then(Value::as_str)
                .or_else(|| vary_vals.get(col).copied())
                .unwrap_or_default();
            full.push(cell.to_string());
        }
        result.push(full);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_fields_with_embedded_delimiter() {
        let rows = parse("a,b\n\"x,y\",\"he said \"\"hi\"\"\"\n", ',');
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][0], "x,y");
        assert_eq!(rows[1][1], "he said \"hi\"");
    }

    #[test]
    fn parse_skips_blank_lines() {
        assert_eq!(parse("a\n\n\nb", ',').len(), 2);
    }

    /// A record of empty fields is data, not a blank line.
    #[test]
    fn parse_keeps_records_of_empty_fields() {
        let rows = parse("a,b\n1,2\n,\n3,4\n", ',');
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert_eq!(rows[2], vec![String::new(), String::new()]);
    }

    /// A quote inside an unquoted field is a literal character (RFC 4180
    /// quotes a whole field or nothing), so it never swallows the next row.
    #[test]
    fn parse_keeps_mid_field_quotes_literal() {
        let rows = parse("name,height\nbob,5'10\"\nal,6'1\"\n", ',');
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[1], vec!["bob".to_string(), "5'10\"".to_string()]);
        assert_eq!(rows[2], vec!["al".to_string(), "6'1\"".to_string()]);
    }

    /// A redundant roster: two constant columns (`status`, `region`) and two
    /// varying ones (`id`, `name`) over enough rows to clearly pay.
    fn roster_csv() -> String {
        let mut s = String::from("id,name,status,region\n");
        for i in 0..16 {
            s.push_str(&format!("{i},user{i},active,eu-central-1\n"));
        }
        s
    }

    #[test]
    fn lossless_factors_constant_columns() {
        let csv = roster_csv();
        let crushed = crush_text_if_beneficial(&csv, ',').expect("should crush");
        // Each constant value appears once, not on every row.
        assert_eq!(crushed.matches("eu-central-1").count(), 1);
        assert_eq!(crushed.matches("active").count(), 1);
        assert!(crushed.contains("_const"));
    }

    #[test]
    fn lossless_roundtrips_exactly() {
        let csv = roster_csv();
        let crushed = crush_text_if_beneficial(&csv, ',').unwrap();
        let restored = reconstruct(&crushed).unwrap();
        let original = parse(&csv, ',');
        assert_eq!(restored, original);
    }

    #[test]
    fn output_is_byte_stable_across_calls() {
        let csv = roster_csv();
        let run = || crush_text_if_beneficial(&csv, ',').unwrap();
        assert_eq!(run(), run(), "crush output must be deterministic");
    }

    #[test]
    fn never_inflates_and_clears_the_gate() {
        let csv = roster_csv();
        let crushed = crush_text_if_beneficial(&csv, ',').unwrap();
        assert!(crushed.len() < csv.len(), "must never inflate");
        assert!(beneficial(&crushed, &csv), "must clear the reduction gate");
    }

    #[test]
    fn tsv_delimiter_is_honoured() {
        let mut s = String::from("id\tname\tstatus\tregion\n");
        for i in 0..16 {
            s.push_str(&format!("{i}\tuser{i}\tactive\teu-central-1\n"));
        }
        let crushed = crush_text_if_beneficial(&s, '\t').expect("tsv crushes");
        assert_eq!(reconstruct(&crushed).unwrap(), parse(&s, '\t'));
    }

    #[test]
    fn quoted_fields_with_embedded_delimiters_roundtrip() {
        // `note` varies and carries embedded commas + escaped quotes, so it stays
        // a varying cell in `_rows`; the constant columns make the crush pay.
        let mut s = String::from("id,note,status,region\n");
        for i in 0..16 {
            s.push_str(&format!("{i},\"a, {i} \"\"q\"\"\",active,eu-central-1\n"));
        }
        let crushed = crush_text_if_beneficial(&s, ',').unwrap();
        let restored = reconstruct(&crushed).unwrap();
        assert_eq!(restored, parse(&s, ','));
        // The embedded-delimiter, escaped-quote value survived verbatim.
        assert_eq!(restored[1][1], "a, 0 \"q\"");
    }

    #[test]
    fn skips_tables_with_nothing_to_factor() {
        // Every column varies on every row -> no constant to hoist.
        let mut s = String::from("a,b,c\n");
        for i in 0..10 {
            s.push_str(&format!("{i},{},{}\n", i + 100, i + 200));
        }
        assert!(crush_text_if_beneficial(&s, ',').is_none());
    }

    #[test]
    fn skips_ragged_too_small_and_json_input() {
        assert!(crush_text_if_beneficial("a,b\n1,ok\n2,ok", ',').is_none()); // < MIN_ROWS
        assert!(crush_text_if_beneficial("a,b\n1,ok\n2\n3,ok\n4,ok", ',').is_none()); // ragged
        assert!(crush_text_if_beneficial("col\n1\n2\n3\n4", ',').is_none()); // single column
        assert!(crush_text_if_beneficial("[{\"a\":1}]", ',').is_none()); // JSON
        assert!(crush_text_if_beneficial("", ',').is_none());
    }
}
