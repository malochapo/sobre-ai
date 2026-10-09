use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::core::session::{Decision, FileTouched, Finding, SessionState, SessionStats, TaskInfo};

const MAX_BUNDLE_BYTES: usize = 250_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CcpSessionBundleV1 {
    pub schema_version: u32,
    pub exported_at: DateTime<Utc>,
    pub project: ProjectIdentityV1,
    pub session: SessionExcerptV1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectIdentityV1 {
    pub project_root_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionExcerptV1 {
    pub id: String,
    pub version: u32,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub project_root: Option<String>,
    pub shell_cwd: Option<String>,
    pub task: Option<TaskInfo>,
    pub findings: Vec<Finding>,
    pub decisions: Vec<Decision>,
    pub files_touched: Vec<FileTouched>,
    pub stats: SessionStats,
    pub terse_mode: bool,
    pub compression_level: String,
}

/// The session as an exportable bundle, always redacted.
pub fn build_bundle_v1(session: &SessionState) -> CcpSessionBundleV1 {
    // The bundle records the exporting machine's compression style; importing
    // never applies it (settings come from the importer's config.toml).
    let export_level =
        crate::core::config::CompressionLevel::effective(&crate::core::config::Config::load());
    let project_root_hash = session
        .project_root
        .as_deref()
        .map(crate::core::project_hash::hash_project_root);

    let mut excerpt = SessionExcerptV1 {
        id: session.id.clone(),
        version: session.version,
        started_at: session.started_at,
        updated_at: session.updated_at,
        project_root: session.project_root.clone(),
        shell_cwd: session.shell_cwd.clone(),
        task: session.task.clone(),
        findings: session.findings.clone(),
        decisions: session.decisions.clone(),
        files_touched: session.files_touched.clone(),
        stats: session.stats.clone(),
        terse_mode: export_level.is_active(),
        compression_level: export_level.label().to_string(),
    };

    // Path minimization: prefer relative paths when project_root is known.
    let root = excerpt.project_root.clone().unwrap_or_default();
    if !root.is_empty() {
        for f in &mut excerpt.files_touched {
            if let Some(rel) = strip_root_prefix(&root, &f.path) {
                f.path = rel;
            }
        }
        for finding in &mut excerpt.findings {
            if let Some(ref file) = finding.file.clone()
                && let Some(rel) = strip_root_prefix(&root, file)
            {
                finding.file = Some(rel);
            }
        }
    }

    redact_excerpt_in_place(&mut excerpt);

    CcpSessionBundleV1 {
        schema_version: crate::core::contracts::CCP_SESSION_BUNDLE_V1_SCHEMA_VERSION,
        exported_at: Utc::now(),
        project: ProjectIdentityV1 { project_root_hash },
        session: excerpt,
    }
}

pub fn serialize_bundle_v1_pretty(bundle: &CcpSessionBundleV1) -> Result<String, String> {
    let json = serde_json::to_string_pretty(bundle).map_err(|e| e.to_string())?;
    if json.len() > MAX_BUNDLE_BYTES {
        return Err(format!(
            "ERROR: bundle too large ({} bytes > max {}). Record fewer findings and files.",
            json.len(),
            MAX_BUNDLE_BYTES
        ));
    }
    Ok(json)
}

pub fn parse_bundle_v1(json: &str) -> Result<CcpSessionBundleV1, String> {
    let b: CcpSessionBundleV1 = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if b.schema_version != crate::core::contracts::CCP_SESSION_BUNDLE_V1_SCHEMA_VERSION {
        return Err(format!(
            "ERROR: unsupported schema_version {} (expected {})",
            b.schema_version,
            crate::core::contracts::CCP_SESSION_BUNDLE_V1_SCHEMA_VERSION
        ));
    }
    Ok(b)
}

pub fn write_bundle_v1(path: &Path, json: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "ERROR: invalid path".to_string())?;
    if !parent.exists() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("bundle")
    ));
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn read_bundle_v1(path: &Path) -> Result<CcpSessionBundleV1, String> {
    let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    if json.len() > MAX_BUNDLE_BYTES {
        return Err(format!(
            "ERROR: bundle file too large ({} bytes > max {})",
            json.len(),
            MAX_BUNDLE_BYTES
        ));
    }
    parse_bundle_v1(&json)
}

pub fn import_bundle_v1_into_session(
    session: &mut SessionState,
    bundle: &CcpSessionBundleV1,
    current_project_root: Option<&str>,
) -> ImportReportV1 {
    let mut imported = bundle.session.clone();

    // Prefer current project root when provided (replay safety).
    if let Some(root) = current_project_root {
        imported.project_root = Some(root.to_string());
    }

    // Mark stale file paths if missing.
    let root = imported.project_root.clone().unwrap_or_else(|| {
        std::env::current_dir()
            .map_or_else(|_| ".".to_string(), |p| p.to_string_lossy().to_string())
    });
    let root_path = PathBuf::from(&root);

    let mut stale = 0u32;
    for f in &mut imported.files_touched {
        let candidate = candidate_path(&root_path, &f.path);
        if candidate.exists() {
            f.stale = false;
        } else {
            f.stale = true;
            stale += 1;
        }
    }

    session.replace_record(SessionState {
        id: imported.id.clone(),
        version: imported.version,
        started_at: imported.started_at,
        updated_at: imported.updated_at,
        project_root: imported.project_root.clone(),
        shell_cwd: imported.shell_cwd.clone(),
        task: imported.task.clone(),
        findings: imported.findings.clone(),
        decisions: imported.decisions.clone(),
        files_touched: imported.files_touched.clone(),
        stats: imported.stats.clone(),
        extra_roots: Vec::new(),
        last_flush: None,
    });

    ImportReportV1 {
        session_id: session.id.clone(),
        version: session.version,
        files_touched: session.files_touched.len() as u32,
        stale_files: stale,
    }
}

#[derive(Debug, Clone)]
pub struct ImportReportV1 {
    pub session_id: String,
    pub version: u32,
    pub files_touched: u32,
    pub stale_files: u32,
}

fn redact_excerpt_in_place(ex: &mut SessionExcerptV1) {
    ex.shell_cwd = None;
    // project_root is represented as hashes at bundle level; avoid exporting raw paths.
    ex.project_root = None;

    if let Some(ref mut t) = ex.task {
        t.description = crate::core::redaction::redact_text(&t.description);
        if let Some(ref mut intent) = t.intent {
            *intent = crate::core::redaction::redact_text(intent);
        }
    }
    for f in &mut ex.findings {
        f.summary = crate::core::redaction::redact_text(&f.summary);
        if let Some(ref mut file) = f.file {
            *file = crate::core::redaction::redact_text(file);
        }
    }
    for d in &mut ex.decisions {
        d.summary = crate::core::redaction::redact_text(&d.summary);
        if let Some(ref mut r) = d.rationale {
            *r = crate::core::redaction::redact_text(r);
        }
    }
}

fn strip_root_prefix(root: &str, path: &str) -> Option<String> {
    let root = root.trim_end_matches(std::path::MAIN_SEPARATOR);
    let root_prefix = format!("{root}{}", std::path::MAIN_SEPARATOR);
    if path.starts_with(&root_prefix) {
        Some(path.trim_start_matches(&root_prefix).to_string())
    } else {
        None
    }
}

fn candidate_path(root: &Path, stored_path: &str) -> PathBuf {
    let p = PathBuf::from(stored_path);
    if p.is_absolute() { p } else { root.join(p) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_respects_size_cap() {
        let s = SessionState::new();
        let b = build_bundle_v1(&s);
        let json = serialize_bundle_v1_pretty(&b).expect("json");
        assert!(json.len() < MAX_BUNDLE_BYTES);
        let parsed = parse_bundle_v1(&json).expect("parse");
        assert_eq!(parsed.schema_version, b.schema_version);
    }

    #[test]
    fn import_marks_missing_files_stale() {
        let mut s = SessionState::new();
        s.project_root = Some(
            std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        );
        s.touch_file("does-not-exist-xyz.txt", None, "full", 10);
        let b = build_bundle_v1(&s);

        let root = std::env::current_dir()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let mut target = SessionState::new();
        let report = import_bundle_v1_into_session(&mut target, &b, Some(&root));
        assert_eq!(report.files_touched, 1);
        assert_eq!(report.stale_files, 1);
        assert!(target.files_touched[0].stale);
    }
}
