use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    FsRead,
    FsWrite,
    FsDelete,
    NetOutbound,
    ExecSandbox,
    ExecUnrestricted,
    KnowledgeRead,
    KnowledgeWrite,
    CrossProject,
    ConfigWrite,
}

impl Capability {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::FsRead => "fs:read",
            Self::FsWrite => "fs:write",
            Self::FsDelete => "fs:delete",
            Self::NetOutbound => "net:outbound",
            Self::ExecSandbox => "exec:sandbox",
            Self::ExecUnrestricted => "exec:unrestricted",
            Self::KnowledgeRead => "knowledge:read",
            Self::KnowledgeWrite => "knowledge:write",
            Self::CrossProject => "cross_project",
            Self::ConfigWrite => "config:write",
        }
    }
}

pub struct CapabilityCheckResult {
    pub allowed: bool,
    pub missing: Vec<Capability>,
}

pub fn required_capabilities(tool_name: &str) -> &'static [Capability] {
    match tool_name {
        // The live write path. `ctx_edit` is not registered: keying FsWrite on it
        // meant no shipped tool required FsWrite at all, so a role without it
        // could still edit files through ctx_patch.
        // ctx_checkpoint restore rewrites files from a snapshot.
        "ctx_patch" | "ctx_refactor" | "ctx_checkpoint" => {
            &[Capability::FsRead, Capability::FsWrite]
        }
        // ctx_execute's own description says it "shares ctx_shell's security
        // policy"; requiring the same capability is what makes that true.
        "ctx_shell" | "ctx_execute" => &[Capability::ExecUnrestricted],
        "ctx_knowledge" => &[Capability::KnowledgeRead, Capability::KnowledgeWrite],
        "ctx_session" => &[],
        _ => &[Capability::FsRead],
    }
}

pub fn role_capabilities(role_name: &str) -> HashSet<Capability> {
    match role_name {
        "admin" => HashSet::from([
            Capability::FsRead,
            Capability::FsWrite,
            Capability::FsDelete,
            Capability::NetOutbound,
            Capability::ExecSandbox,
            Capability::ExecUnrestricted,
            Capability::KnowledgeRead,
            Capability::KnowledgeWrite,
            Capability::CrossProject,
            Capability::ConfigWrite,
        ]),
        "reviewer" | "ci" => HashSet::from([
            Capability::FsRead,
            Capability::ExecSandbox,
            Capability::KnowledgeRead,
        ]),
        "minimal" => HashSet::from([Capability::FsRead, Capability::KnowledgeRead]),
        _ => capabilities_from_role(role_name),
    }
}

fn capabilities_from_role(role_name: &str) -> HashSet<Capability> {
    let Some(role) = crate::core::roles::load_role(role_name) else {
        return HashSet::from([
            Capability::FsRead,
            Capability::FsWrite,
            Capability::ExecSandbox,
            Capability::ExecUnrestricted,
            Capability::KnowledgeRead,
            Capability::KnowledgeWrite,
        ]);
    };

    let mut caps = HashSet::new();
    caps.insert(Capability::FsRead);

    let has_tool = |name: &str| {
        role.tools.allowed.iter().any(|a| a == "*" || a == name)
            && !role.tools.denied.iter().any(|d| d == name || d == "*")
    };

    if has_tool("ctx_patch") || has_tool("ctx_refactor") || has_tool("ctx_checkpoint") {
        caps.insert(Capability::FsWrite);
    }
    if has_tool("ctx_shell") || has_tool("ctx_execute") {
        caps.insert(Capability::ExecSandbox);
        caps.insert(Capability::ExecUnrestricted);
    }
    if has_tool("ctx_knowledge") {
        caps.insert(Capability::KnowledgeRead);
        caps.insert(Capability::KnowledgeWrite);
    }
    if role.io.allow_cross_project_search {
        caps.insert(Capability::CrossProject);
    }
    if role.io.allow_secret_paths {
        caps.insert(Capability::ConfigWrite);
    }

    caps
}

pub fn check_capabilities(role_name: &str, tool_name: &str) -> CapabilityCheckResult {
    let required = required_capabilities(tool_name);
    if required.is_empty() {
        return CapabilityCheckResult {
            allowed: true,
            missing: Vec::new(),
        };
    }
    let granted = role_capabilities(role_name);
    let missing: Vec<Capability> = required
        .iter()
        .filter(|c| !granted.contains(c))
        .copied()
        .collect();
    CapabilityCheckResult {
        allowed: missing.is_empty(),
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_has_all_capabilities() {
        let caps = role_capabilities("admin");
        assert!(caps.contains(&Capability::FsRead));
        assert!(caps.contains(&Capability::FsWrite));
        assert!(caps.contains(&Capability::FsDelete));
        assert!(caps.contains(&Capability::NetOutbound));
        assert!(caps.contains(&Capability::ExecUnrestricted));
        assert!(caps.contains(&Capability::ConfigWrite));
    }

    #[test]
    fn checkpoint_restore_requires_write() {
        assert!(required_capabilities("ctx_checkpoint").contains(&Capability::FsWrite));
    }

    #[test]
    fn reviewer_cannot_write() {
        // ctx_patch and ctx_refactor are the registered write paths. ctx_edit is
        // not registered and falls through to the fs:read default, so asserting
        // on it proved nothing.
        for tool in ["ctx_patch", "ctx_refactor"] {
            assert!(
                crate::server::registry::is_registered(tool),
                "{tool} is not registered: this test no longer proves anything"
            );
            let result = check_capabilities("reviewer", tool);
            assert!(!result.allowed, "reviewer must not be granted {tool}");
            assert!(
                result.missing.contains(&Capability::FsWrite),
                "{tool}: reviewer must be missing fs:write, got {:?}",
                result.missing
            );
        }
    }

    #[test]
    fn ops_role_gets_exec_but_not_write() {
        // ops is resolved through roles.rs (capabilities_from_role), not a
        // hard-coded grant. It does not grant ctx_patch/ctx_refactor, so it must
        // not get fs:write; it grants ctx_shell/ctx_execute, so it gets exec.
        for tool in ["ctx_patch", "ctx_refactor"] {
            let result = check_capabilities("ops", tool);
            assert!(!result.allowed, "ops must not be granted {tool}");
            assert!(result.missing.contains(&Capability::FsWrite));
        }
        for tool in ["ctx_shell", "ctx_execute"] {
            assert!(
                check_capabilities("ops", tool).allowed,
                "ops must be granted {tool}"
            );
        }
    }

    /// `capabilities_from_role` derives capabilities from tool names. Built-in
    /// roles cannot pin that derivation (coder and admin use "*", reviewer is a
    /// hard-coded arm), so custom roles that name one tool each do: keying the
    /// derivation back on an unregistered name fails here.
    #[test]
    fn custom_role_capabilities_follow_the_live_tools() {
        let iso = crate::core::data_dir::isolated_data_dir();
        let dir = iso.path().join("roles");
        std::fs::create_dir_all(&dir).unwrap();
        let write_role = |name: &str, allowed: &[&str]| {
            let list = allowed
                .iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(", ");
            std::fs::write(
                dir.join(format!("{name}.toml")),
                format!("[tools]\nallowed = [{list}]\n"),
            )
            .unwrap();
        };

        for tool in ["ctx_patch", "ctx_refactor", "ctx_shell", "ctx_execute"] {
            let name = format!("probe_{}", tool.trim_start_matches("ctx_"));
            write_role(&name, &["ctx_read", tool]);
            let result = check_capabilities(&name, tool);
            assert!(
                result.allowed,
                "a role allowing only ctx_read and {tool} must be granted {tool}, missing {:?}",
                result.missing
            );
        }

        write_role("probe_readonly", &["ctx_read"]);
        for tool in ["ctx_patch", "ctx_refactor", "ctx_shell", "ctx_execute"] {
            assert!(
                !check_capabilities("probe_readonly", tool).allowed,
                "a role allowing only ctx_read must not be granted {tool}"
            );
        }
    }

    #[test]
    fn minimal_cannot_shell() {
        let result = check_capabilities("minimal", "ctx_shell");
        assert!(!result.allowed);
        assert!(result.missing.contains(&Capability::ExecUnrestricted));
    }

    #[test]
    fn session_always_allowed() {
        let result = check_capabilities("minimal", "ctx_session");
        assert!(result.allowed);
        assert!(result.missing.is_empty());
    }

    #[test]
    fn developer_can_edit() {
        // "developer" is not a built-in role, so it exercises the not-found
        // fallback grant; "coder" exercises capabilities_from_role with "*".
        // Both must reach the live write tools, which require fs:write.
        for role in ["developer", "coder"] {
            for tool in ["ctx_patch", "ctx_refactor"] {
                let result = check_capabilities(role, tool);
                assert!(
                    result.allowed,
                    "{role}/{tool}: missing {:?}",
                    result.missing
                );
            }
        }
    }

    #[test]
    fn unknown_role_gets_defaults() {
        let result = check_capabilities("unknown_role", "ctx_read");
        assert!(result.allowed);
    }

    #[test]
    fn unknown_tool_requires_fs_read() {
        let required = required_capabilities("some_unknown_tool");
        assert_eq!(required, &[Capability::FsRead]);
    }

    #[test]
    fn display_names_are_colon_separated() {
        assert_eq!(Capability::FsRead.display_name(), "fs:read");
        assert_eq!(
            Capability::ExecUnrestricted.display_name(),
            "exec:unrestricted"
        );
    }
}
