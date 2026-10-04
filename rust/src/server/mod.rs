pub mod background_shell;
pub mod bounded_lock;
pub mod bypass_hint;
pub mod compaction_sync;
pub mod context_gate;
mod dispatch;
pub mod elicitation;
pub(crate) mod execute;
pub mod helpers;
pub mod multi_path;
pub mod notifications;
pub mod permission_inheritance;
pub mod progress;
pub mod prompts;
pub mod reference_store;
pub mod registry;
pub mod resources;
pub mod roots;
use roots::has_project_marker;
pub mod tool_trait;
pub mod tool_visibility;

use futures::FutureExt;
use rmcp::ErrorData;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, InitializeRequestParams,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use std::path::PathBuf;

use crate::tools::{CrpMode, LeanCtxServer};
mod call_tool;
mod post_dispatch;
mod post_process;
mod server_handler;

pub fn build_instructions_for_test(crp_mode: CrpMode) -> String {
    crate::instructions::build_instructions_for_test(crp_mode)
}

/// Deterministic STATIC Claude Code instructions (cold first-contact, no dynamic
/// session/knowledge/gotcha payload) for the char-budget benchmark.
pub fn build_claude_code_static_instructions_for_test() -> String {
    crate::instructions::claude_code_static_instructions_for_test()
}

fn is_home_or_agent_dir(dir: &std::path::Path) -> bool {
    if let Some(home) = dirs::home_dir()
        && dir == home
    {
        return true;
    }
    crate::core::pathutil::is_agent_config_dir(dir)
}

fn git_toplevel_from(dir: &std::path::Path) -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
}

pub fn derive_project_root_from_cwd() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let canonical = crate::core::pathutil::safe_canonicalize_or_self(&cwd);

    if is_home_or_agent_dir(&canonical) {
        return git_toplevel_from(&canonical);
    }

    if has_project_marker(&canonical) {
        return Some(canonical.to_string_lossy().to_string());
    }

    if let Some(git_root) = git_toplevel_from(&canonical) {
        return Some(git_root);
    }

    if let Some(root) = detect_multi_root_workspace(&canonical) {
        return Some(root);
    }

    // Fallback: use CWD as project root if it's a specific, safe directory.
    // This ensures bare directories (no .git, no markers) still work.
    // Guard: reject home dir, filesystem root, and agent sandbox dirs.
    if !crate::core::pathutil::is_broad_or_unsafe_root(&canonical) {
        tracing::info!(
            "No project markers found — using CWD as project root: {}",
            canonical.display()
        );
        return Some(canonical.to_string_lossy().to_string());
    }

    None
}

// Delegated to crate::core::pathutil::is_broad_or_unsafe_root
#[cfg(test)]
use crate::core::pathutil::is_broad_or_unsafe_root;

/// Detect a multi-root workspace: a directory that has no project markers
/// itself, but contains child directories that do. In this case, use the
/// parent as jail root and auto-allow all child projects via LEAN_CTX_ALLOW_PATH.
fn detect_multi_root_workspace(dir: &std::path::Path) -> Option<String> {
    // Never enumerate the home dir or macOS TCC-protected dirs (Documents/Desktop/
    // Downloads): read_dir there triggers a macOS privacy prompt (#356), and a real
    // project under them is already handled upstream via has_project_marker.
    if crate::core::pathutil::is_tcc_sensitive_home_dir(dir) {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut child_projects: Vec<String> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && has_project_marker(&path) {
            let canonical = crate::core::pathutil::safe_canonicalize_or_self(&path);
            child_projects.push(canonical.to_string_lossy().to_string());
        }
    }

    if child_projects.len() >= 2 {
        crate::core::runtime_flags::add_allow_paths(
            child_projects.iter().map(PathBuf::from).collect(),
        );
        tracing::info!(
            "Multi-root workspace detected at {}: auto-allowing {} child projects",
            dir.display(),
            child_projects.len()
        );
        return Some(dir.to_string_lossy().to_string());
    }

    None
}

pub fn tool_descriptions_for_test() -> Vec<(String, String)> {
    crate::server::registry::build_registry()
        .tool_defs()
        .into_iter()
        .map(|t| {
            (
                t.name.to_string(),
                t.description.as_deref().unwrap_or("").to_string(),
            )
        })
        .collect()
}

pub fn tool_schemas_json_for_test() -> String {
    crate::server::registry::build_registry()
        .tool_defs()
        .iter()
        .map(|t| {
            format!(
                "{}: {}",
                t.name,
                serde_json::to_string(&t.input_schema).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_shell_tool_name(name: &str) -> bool {
    matches!(name, "ctx_shell" | "ctx_execute")
}

fn extract_file_read_from_shell(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let bin = parts[0].rsplit('/').next().unwrap_or(parts[0]);
    match bin {
        "cat" | "head" | "tail" | "less" | "more" | "bat" | "batcat" => {
            let file_arg = parts.iter().skip(1).find(|a| !a.starts_with('-'))?;
            Some(file_arg.to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_dir_detected_as_agent_dir() {
        if let Some(home) = dirs::home_dir() {
            assert!(is_home_or_agent_dir(&home));
        }
    }

    #[test]
    fn agent_dirs_detected() {
        let claude = std::path::PathBuf::from("/home/user/.claude");
        assert!(is_home_or_agent_dir(&claude));
        let codex = std::path::PathBuf::from("/home/user/.codex");
        assert!(is_home_or_agent_dir(&codex));
        let project = std::path::PathBuf::from("/home/user/projects/myapp");
        assert!(!is_home_or_agent_dir(&project));
    }

    #[test]
    fn test_registry_tool_count_ssot() {
        let registry = crate::server::registry::build_registry();
        assert_eq!(
            registry.len(),
            19,
            "registry tool count drift — the surface is 19 and capped at 20              (invariant 1). Update this test AND the docs when it moves."
        );
    }

    #[test]
    fn production_server_always_has_registry() {
        // The list_tools fallback that serves static defs when `registry` is None
        // must stay unreachable in production: every public constructor funnels
        // through new_with_startup, which sets registry = Some. Lock that invariant
        // so the advertised tool set can never silently drift from what dispatch
        // (which requires the registry) can actually execute.
        let server = crate::tools::create_server();
        assert!(
            server.registry.is_some(),
            "production server must carry a tool registry"
        );
    }

    #[test]
    fn detect_multi_root_workspace_with_child_projects() {
        // detect_multi_root_workspace SETS `LEAN_CTX_ALLOW_PATH` as a side
        // effect (auto-allowing the child projects) — hold the global env
        // lock so a parallel jail test never observes the transient value
        // (the pre-existing full-suite flake reported in #695).
        let _guard = crate::core::data_dir::test_env_lock();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let proj_a = workspace.join("project-a");
        let proj_b = workspace.join("project-b");
        std::fs::create_dir_all(proj_a.join(".git")).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();
        std::fs::write(proj_b.join("package.json"), "{}").unwrap();

        let result = detect_multi_root_workspace(&workspace);
        assert!(
            result.is_some(),
            "should detect workspace with 2 child projects"
        );

        crate::test_env::remove_var("LEAN_CTX_ALLOW_PATH");
    }

    #[test]
    fn detect_multi_root_workspace_returns_none_for_single_project() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let proj_a = workspace.join("project-a");
        std::fs::create_dir_all(proj_a.join(".git")).unwrap();

        let result = detect_multi_root_workspace(&workspace);
        assert!(
            result.is_none(),
            "should not detect workspace with only 1 child project"
        );
    }

    #[test]
    fn is_broad_or_unsafe_root_allows_tmp_subdirs() {
        assert!(!is_broad_or_unsafe_root(std::path::Path::new(
            "/tmp/leanctx-test"
        )));
        assert!(!is_broad_or_unsafe_root(std::path::Path::new(
            "/tmp/my-project"
        )));
    }

    #[test]
    fn derive_project_root_falls_back_to_bare_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare-dir");
        std::fs::create_dir_all(&bare).unwrap();

        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&bare).unwrap();
        let result = derive_project_root_from_cwd();
        std::env::set_current_dir(original).unwrap();

        assert!(result.is_some(), "bare dir should produce a project root");
        let root = result.unwrap();
        assert!(
            root.contains("bare-dir"),
            "fallback should use the bare dir path"
        );
    }
}
