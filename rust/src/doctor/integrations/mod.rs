//! `sobre doctor integrations` — per-editor wiring health, split by
//! concern. Submodules are re-exported flat; `run_integrations` stays the
//! only entry point.

use chrono::Utc;
use serde::Serialize;

use super::{
    BOLD, DIM, GREEN, RST, WHITE, YELLOW, claude_binary_exists, has_sobre_mcp_entry,
    resolve_sobre_binary, tildify_home,
};

#[derive(Debug, Clone, Copy)]
pub(super) struct IntegrationsOptions {
    pub json: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntegrationCheckReport {
    pub(crate) schema_version: u32,
    pub(crate) created_at: String,
    pub(crate) binary: String,
    pub(crate) integrations: Vec<IntegrationStatus>,
    pub(crate) ok: bool,
    pub(crate) repair_command: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntegrationStatus {
    pub(crate) name: String,
    pub(crate) detected: bool,
    pub(crate) checks: Vec<NamedCheck>,
    pub(crate) ok: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NamedCheck {
    pub(crate) name: String,
    pub(crate) ok: bool,
    pub(crate) detail: String,
}

pub(super) fn run_integrations(opts: &IntegrationsOptions) -> i32 {
    let Some(home) = dirs::home_dir() else {
        crate::errln!("Cannot determine home directory");
        return 2;
    };
    let binary = crate::core::portable_binary::resolve_portable_binary();

    let mut integrations = vec![
        integration_cursor(&home, &binary),
        integration_claude(&home, &binary),
    ];
    for t in crate::core::editor_registry::build_targets(&home) {
        if matches!(t.name, "Cursor" | "Claude Code") {
            continue;
        }
        integrations.push(integration_generic(&home, &binary, &t));
    }
    let ok = integrations.iter().all(|i| !i.detected || i.ok);

    let report = IntegrationCheckReport {
        schema_version: 1,
        created_at: Utc::now().to_rfc3339(),
        binary: binary.clone(),
        integrations,
        ok,
        repair_command: "sobre init --repair".to_string(),
    };

    if opts.json {
        crate::outln!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        crate::outln!();
        crate::outln!("  {BOLD}{WHITE}Integration health:{RST}");
        for i in &report.integrations {
            if !i.detected {
                continue;
            }
            let mark = if i.ok {
                format!("{GREEN}✓{RST}")
            } else {
                format!("{YELLOW}✗{RST}")
            };
            crate::outln!("  {mark}  {BOLD}{}{RST}", i.name);
            for c in &i.checks {
                let m = if c.ok {
                    format!("{GREEN}✓{RST}")
                } else {
                    format!("{YELLOW}✗{RST}")
                };
                crate::outln!(
                    "       {m}  {}  {DIM}{}{RST}",
                    c.name,
                    tildify_home(&c.detail)
                );
            }
        }
        if !report.ok {
            crate::outln!();
            crate::outln!(
                "  {YELLOW}Repair:{RST} run {BOLD}{}{RST}",
                report.repair_command
            );
        }
    }

    i32::from(!report.ok)
}

mod codex;
mod configs;
mod editors;
mod hooks;
mod wiring;

pub(crate) use codex::*;
pub(crate) use configs::*;
pub(crate) use editors::*;
pub(crate) use hooks::*;
pub(crate) use wiring::*;

#[cfg(test)]
mod tests;
