use std::path::Path;

use rmcp::ErrorData;
use rmcp::model::Tool;
use serde_json::{Map, Value, json};

use crate::core::git::shadow::{self, Checkpoint};
use crate::core::tokens::count_tokens;
use crate::server::tool_trait::{McpTool, ToolContext, ToolOutput, get_int, get_str};
use crate::tool_defs::tool_def;

const DEFAULT_LOG_LIMIT: usize = 20;
const DIFF_MAX_TOKENS: usize = 8000;

/// `sobre_checkpoint` — snapshot, review, diff, and revert the agent's code
/// changes in a shadow git history kept outside the user's own `.git`.
pub struct SobreCheckpointTool;

impl McpTool for SobreCheckpointTool {
    fn name(&self) -> &'static str {
        "sobre_checkpoint"
    }

    /// Errors, conflicts and unknown actions start with `ERROR`, `CONFLICT`
    /// or `Unknown action` (B29).
    fn text_reports_failures(&self) -> bool {
        true
    }

    fn tool_def(&self) -> Tool {
        tool_def(
            "sobre_checkpoint",
            "Local shadow git history of the agent's changes — separate from the user's .git.\n\
WORKFLOW: snapshot before+after changes to capture exactly what was modified.\n\
Actions: snapshot (record current state), log (list checkpoints with SHAs),\n\
diff from=... to=... (compare checkpoints), restore ref=... (revert files).\n\
ANTIPATTERN: Never touches the user's repository — completely isolated shadow history.",
            json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["snapshot", "log", "diff", "restore"],
                        "description": "snapshot|log|diff|restore (default: log)"
                    },
                    "message": { "type": "string", "description": "Label for the snapshot (action=snapshot only)" },
                    "from": { "type": "string", "description": "Base checkpoint SHA for diff (default: HEAD~1, action=diff only)" },
                    "to": { "type": "string", "description": "Target checkpoint SHA for diff (default: HEAD, action=diff only)" },
                    "ref": { "type": "string", "description": "Checkpoint SHA to restore (required for action=restore)" },
                    "path": { "type": "string", "description": "File/dir to restore (omit for full restore, action=restore only)" },
                    "limit": { "type": "integer", "description": "Max checkpoints in log (default: 20, max: 200)" }
                },
                "allOf": [
                    {
                        "if": { "properties": { "action": { "const": "restore" } }, "required": ["action"] },
                        "then": { "required": ["action", "ref"] }
                    }
                ]
            }),
        )
    }

    fn handle(
        &self,
        args: &Map<String, Value>,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ErrorData> {
        let action = get_str(args, "action").unwrap_or_else(|| "log".to_string());
        let project = Path::new(&ctx.project_root).to_path_buf();
        if ctx.project_root.is_empty() {
            return Err(ErrorData::invalid_params(
                "no project root resolved for sobre_checkpoint",
                None,
            ));
        }

        let result = tokio::task::block_in_place(|| match action.as_str() {
            "snapshot" => {
                let msg = get_str(args, "message").unwrap_or_default();
                shadow::snapshot(&project, &msg).map(|c| render_checkpoint_line("Checkpoint", &c))
            }
            "log" => {
                let limit =
                    get_int(args, "limit").map_or(DEFAULT_LOG_LIMIT, |n| n.clamp(1, 200) as usize);
                shadow::log(&project, limit).map(|cs| render_log(&cs))
            }
            "diff" => {
                let from = get_str(args, "from");
                let to = get_str(args, "to");
                shadow::diff(&project, from.as_deref(), to.as_deref())
                    .map(|d| budget(&d, DIFF_MAX_TOKENS))
            }
            "restore" => {
                let git_ref = get_str(args, "ref").ok_or_else(|| {
                    "restore requires 'ref' (a checkpoint sha from log)".to_string()
                })?;
                let path = get_str(args, "path");
                shadow::restore(&project, &git_ref, path.as_deref())
            }
            other => Err(format!(
                "invalid action '{other}' (use: snapshot, log, diff, restore)"
            )),
        });

        match result {
            Ok(text) => Ok(ToolOutput {
                text,
                original_tokens: 0,
                saved_tokens: 0,
                shell_outcome: None,
                content_blocks: None,
            }),
            Err(e) => Err(ErrorData::invalid_params(
                format!("sobre_checkpoint failed: {e}"),
                None,
            )),
        }
    }
}

fn render_checkpoint_line(prefix: &str, c: &Checkpoint) -> String {
    let files = c
        .files_changed
        .map(|n| format!(" · {n} file(s)"))
        .unwrap_or_default();
    format!("{prefix} {}{files} — {}", c.sha, c.message)
}

fn render_log(checkpoints: &[Checkpoint]) -> String {
    if checkpoints.is_empty() {
        return "No checkpoints yet. Run `sobre_checkpoint` with action=snapshot to record one."
            .to_string();
    }
    let mut out = format!("{} checkpoint(s):\n", checkpoints.len());
    for c in checkpoints {
        out.push_str(&format!("- {} · {} — {}\n", c.sha, c.time, c.message));
    }
    out.trim_end().to_string()
}

fn budget(content: &str, max_tokens: usize) -> String {
    if content.trim().is_empty() {
        return "No differences.".to_string();
    }
    let tokens = count_tokens(content);
    if tokens <= max_tokens {
        return content.to_string();
    }
    let ratio = max_tokens as f64 / tokens as f64;
    let keep = ((content.chars().count() as f64 * ratio) as usize).max(1);
    let truncated: String = content.chars().take(keep).collect();
    // The whole diff is saved like a compressed shell output, so the cut
    // part stays reachable (B26).
    let rest = crate::shell::save_tee("sobre_checkpoint diff", content)
        .and_then(|p| {
            let id = std::path::Path::new(&p).file_name()?.to_str()?.to_string();
            Some(format!(
                "[sobre: diff cut at ~{max_tokens} tokens; full diff at {p}, or sobre_expand(id=\"{id}\")]"
            ))
        })
        .unwrap_or_else(|| format!("[sobre: diff cut at ~{max_tokens} tokens]"));
    format!("{truncated}\n\n{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(sha: &str, msg: &str, files: Option<usize>) -> Checkpoint {
        Checkpoint {
            sha: sha.to_string(),
            time: "2026-06-07T10:00:00Z".to_string(),
            message: msg.to_string(),
            files_changed: files,
        }
    }

    #[test]
    fn renders_empty_log_with_hint() {
        assert!(render_log(&[]).contains("snapshot"));
    }

    #[test]
    fn renders_checkpoint_line_with_file_count() {
        let line = render_checkpoint_line("Checkpoint", &cp("abc1234", "fix bug", Some(3)));
        assert_eq!(line, "Checkpoint abc1234 · 3 file(s) — fix bug");
    }

    #[test]
    fn renders_log_entries() {
        let out = render_log(&[cp("a1", "one", None), cp("b2", "two", None)]);
        assert!(out.contains("2 checkpoint(s)"));
        assert!(out.contains("- a1 ·"));
        assert!(out.contains("- b2 ·"));
    }

    #[test]
    fn a_cut_diff_points_to_the_whole_diff() {
        let _dir = crate::core::data_dir::isolated_data_dir();
        let diff = (0..400).fold(String::new(), |mut acc, i| {
            use std::fmt::Write as _;
            let _ = writeln!(acc, "+line {i} of the change");
            acc
        });
        let out = budget(&diff, 50);
        assert!(out.contains("sobre_expand(id="), "{out}");
        let id = out.rsplit("id=\"").next().unwrap().trim_end_matches("\")]");
        let full = crate::tools::sobre_expand::handle(&serde_json::json!({ "id": id }));
        assert!(full.contains("+line 399 of the change"), "{full}");
    }

    #[test]
    fn budget_handles_empty_diff() {
        assert_eq!(budget("", 100), "No differences.");
    }
}
