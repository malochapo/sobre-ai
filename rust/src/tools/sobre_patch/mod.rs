//! `sobre_patch` — hash-anchored editing.
//!
//! "Edit by reference, not by reproduction": the model edits lines by their
//! `(line, hash)` anchor (from `sobre_read(mode="anchored")`) instead of quoting
//! the old text byte-for-byte. Each anchor is verified against the *current*
//! file; on drift the edit is rejected with the current lines and their
//! fresh anchors, never moved to another line. The anchored ops on one file
//! are validated against the same preimage and applied all-or-nothing,
//! bottom-up.
//!
//! Shares the I/O boundary of the text replacements (`crate::tools::edit_io`):
//! TOCTOU preimage guard, permission-preserving atomic write, symlink
//! rejection. A syntax error the edit introduces is reported, never refused.

mod anchors;
mod apply;
mod output;
mod symbol;
#[cfg(test)]
mod tests;

pub use anchors::AnchorOp;
pub(crate) use symbol::{build_refactor_args, is_replace_symbol};

use std::path::Path;

#[cfg(test)]
use crate::core::cache::SessionCache;
use crate::core::tokens::count_tokens;
use crate::tools::edit_io::{
    ensure_preimage_still_matches, read_preimage, write_atomic_bytes_with_permissions,
};
#[cfg(test)]
use crate::tools::impl_edit::apply_cache_effect;
use crate::tools::impl_edit::{CacheEffect, append_evidence, append_notice, dry_run_text};

/// An anchored patch: the target file and one or more anchored edit ops.
pub struct PatchParams {
    pub path: String,
    pub ops: Vec<AnchorOp>,
    /// Run every check and show the diff without writing anything.
    pub dry_run: bool,
}

/// Parse the raw tool arguments into [`AnchorOp`]s (single op or `ops[]`).
pub fn parse_ops(
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<AnchorOp>, String> {
    anchors::parse_ops(args)
}

/// Apply an anchored patch and the resulting cache effect in one shot (tests and
/// in-process callers that hold the cache exclusively).
#[cfg(test)]
pub fn handle(cache: &mut SessionCache, params: &PatchParams) -> String {
    let last_mode = cache
        .get(&params.path)
        .map(|e| e.last_mode.clone())
        .unwrap_or_default();
    let (text, effect) = run_io(params, &last_mode);
    record_outcome(params, &last_mode, &text, &effect);
    apply_cache_effect(cache, &params.path, effect);
    text
}

/// Quality loop: a clean anchored edit is a success signal for the
/// read mode that produced the anchors; a stale-anchor `CONFLICT` is a failure
/// signal (the view the model edited against had drifted) that arms a one-shot
/// escalation of the next auto read to `anchored` — fresh line anchors to retry
/// by reference. Structural errors say nothing about the read mode and are
/// skipped.
pub fn record_outcome(params: &PatchParams, last_mode: &str, text: &str, effect: &CacheEffect) {
    let success = matches!(effect, CacheEffect::Invalidate);
    let conflict = matches!(effect, CacheEffect::None) && text.starts_with("CONFLICT:");
    if success || conflict {
        crate::core::edit_quality::record_anchored_edit_outcome(&params.path, last_mode, success);
    }
}

/// Perform the anchored patch on disk **without** touching the cache; returns
/// the [`CacheEffect`] for the caller to apply. `last_mode` is currently only
/// used by [`record_outcome`]; pass `""` when unknown.
pub fn run_io(params: &PatchParams, _last_mode: &str) -> (String, CacheEffect) {
    let file_path = &params.path;
    let path = Path::new(file_path);
    let cap = crate::core::limits::max_read_bytes();

    // `create` short-circuits the anchored pipeline: no preimage exists to
    // anchor against, so it must be the only op and the file must be new.
    if let Some(content) = single_create_op(&params.ops) {
        return match content {
            Ok(text) => handle_create(params, path, text),
            Err(e) => (e, CacheEffect::None),
        };
    }

    let pre = match read_preimage(path, cap) {
        Ok(p) => p,
        Err(e) => {
            if !path.exists() {
                let hint = crate::tools::edit_recovery::moved_or_deleted_hint(path);
                return (format!("{e}{hint}"), CacheEffect::None);
            }
            return (e, CacheEffect::None);
        }
    };

    if params.ops.is_empty() {
        return (
            "ERROR: no edits provided (pass an op or ops:[…])".to_string(),
            CacheEffect::None,
        );
    }

    // BOM parity with `sobre_read` (follow-up): the read side strips a
    // UTF-8 BOM before hashing line 1, so anchors must be validated against
    // the BOM-less body — otherwise every line-1 edit of a BOM file conflicts
    // forever. The BOM itself is preserved on write (prepended below); it is
    // an encoding artifact of the file, not of the edit.
    let (bom, body) = match pre.text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", pre.text.as_str()),
    };
    let (lines, eols) = apply::split_lines(body);

    let edits = match apply::resolve_ops(&lines, &params.ops) {
        Ok(e) => e,
        Err(apply::ResolveError::Conflict(misses)) => {
            return (
                output::render_conflict(file_path, &lines, &misses),
                CacheEffect::None,
            );
        }
        Err(apply::ResolveError::Invalid(msg)) => {
            return (format!("ERROR: {msg}"), CacheEffect::None);
        }
    };

    let n_edits = edits.len();
    let lines_before = lines.len();
    let (new_lines, new_eols) = apply::apply_edits(lines.clone(), eols, edits);
    let new_content = format!("{bom}{}", apply::join_lines(&new_lines, &new_eols));

    if new_content == pre.text {
        return (
            "ERROR: edits produced no change to the file".to_string(),
            CacheEffect::None,
        );
    }

    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    // A syntax error the edit introduces is reported with the result; the
    // edit is written either way.
    let syntax_notice = crate::core::syntax_validate::syntax_notice(ext, &pre.text, &new_content);

    if params.dry_run {
        let mut out = dry_run_text(path, &pre.text, &new_content);
        append_notice(&mut out, syntax_notice);
        return (out, CacheEffect::None);
    }

    // Code-health gate: an advisory notice on cognitive-complexity drift.
    let health_notice = crate::core::code_health::gate::evaluate(&pre.text, &new_content, ext);

    // TOCTOU guard: confirm the file did not change between read and write.
    // A point-in-time check, not a held lock — see
    // ensure_preimage_still_matches' doc for the residual window between
    // this check and the write below.
    if let Err(e) = ensure_preimage_still_matches(path, &pre.fp, cap) {
        return (e, CacheEffect::None);
    }

    if let Err(e) =
        write_atomic_bytes_with_permissions(path, new_content.as_bytes(), Some(&pre.permissions))
    {
        return (e, CacheEffect::None);
    }

    if let Ok(mut bt) = crate::core::bounce_tracker::global().lock() {
        bt.record_edit(file_path);
    }

    let mut out = render_success(
        &params.path,
        &pre.text,
        &new_content,
        lines_before,
        new_lines.len(),
        n_edits,
    );
    append_notice(&mut out, syntax_notice);
    append_notice(&mut out, health_notice);
    (out, CacheEffect::Invalidate)
}

/// If the ops contain a `create`, return its content — or an error when it is
/// mixed with anchored ops (a batch validates against one *existing* preimage,
/// which a new file by definition does not have).
fn single_create_op(ops: &[AnchorOp]) -> Option<Result<&str, String>> {
    let create = ops.iter().find_map(|op| match op {
        AnchorOp::Create { new_text } => Some(new_text.as_str()),
        _ => None,
    })?;
    if ops.len() > 1 {
        return Some(Err(
            "ERROR: create cannot be batched with anchored ops — a new file has no \
             preimage to anchor against; send create as a single op"
                .to_string(),
        ));
    }
    Some(Ok(create))
}

/// `op=create`: write a NEW file (strict — an existing file is an error, unlike
/// an overwrite). Reuses the atomic-write boundary
/// of the anchored path.
fn handle_create(params: &PatchParams, path: &Path, content: &str) -> (String, CacheEffect) {
    if path.exists() {
        return (
            format!(
                "ERROR: {} already exists — create is for new files only. \
                 Use anchored ops (sobre_read mode=\"anchored\" → set_line/replace_lines) to modify it.",
                params.path
            ),
            CacheEffect::None,
        );
    }

    if params.dry_run {
        return (dry_run_text(path, "", content), CacheEffect::None);
    }

    if let Some(parent) = path.parent()
        && !parent.exists()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        return (
            format!("ERROR: cannot create directory {}: {e}", parent.display()),
            CacheEffect::None,
        );
    }

    if let Err(e) = write_atomic_bytes_with_permissions(path, content.as_bytes(), None) {
        return (e, CacheEffect::None);
    }

    if let Ok(mut bt) = crate::core::bounce_tracker::global().lock() {
        bt.record_edit(&params.path);
    }

    let lines = content.lines().count();
    let tokens = count_tokens(content);
    let short = output::short_name(&params.path);
    let mut out = format!("✓ created {short}: {lines} lines, {tokens} tok");
    append_evidence(&mut out, "", content, &short);
    (out, CacheEffect::Invalidate)
}

fn render_success(
    path: &str,
    old_content: &str,
    new_content: &str,
    lines_before: usize,
    lines_after: usize,
    n_edits: usize,
) -> String {
    let short = output::short_name(path);
    let line_delta = lines_after as i64 - lines_before as i64;
    let delta_str = if line_delta >= 0 {
        format!("+{line_delta}")
    } else {
        format!("{line_delta}")
    };
    let old_tokens = count_tokens(old_content);
    let new_tokens = count_tokens(new_content);

    let edit_word = if n_edits == 1 { "edit" } else { "edits" };
    let mut out = format!(
        "✓ {short}: {n_edits} anchored {edit_word}, {delta_str} lines ({old_tokens}→{new_tokens} tok)"
    );
    append_evidence(&mut out, old_content, new_content, &short);
    out
}
