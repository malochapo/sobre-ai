#[derive(Debug, Clone, Copy)]
pub struct ClientConstraints {
    pub id: &'static str,
    pub display_name: &'static str,
    /// Max chars accepted by MCP `instructions` field, if documented.
    pub mcp_instructions_max_chars: Option<usize>,
    /// Whether the client documents `autoApprove` in its MCP config schema.
    pub supports_auto_approve: bool,
}

// Keep this aligned with `docs/integrations/client-constraints-matrix-v1.md`.
pub const ALL_CLIENTS: &[ClientConstraints] = &[
    ClientConstraints {
        id: "cursor",
        display_name: "Cursor",
        mcp_instructions_max_chars: None,
        supports_auto_approve: true,
    },
    ClientConstraints {
        id: "claude-code",
        display_name: "Claude Code",
        mcp_instructions_max_chars: Some(2048),
        supports_auto_approve: false,
    },
    ClientConstraints {
        id: "vscode-copilot",
        display_name: "VS Code / GitHub Copilot",
        mcp_instructions_max_chars: None,
        supports_auto_approve: false,
    },
    ClientConstraints {
        id: "windsurf",
        display_name: "Windsurf",
        mcp_instructions_max_chars: None,
        supports_auto_approve: false,
    },
    ClientConstraints {
        id: "zed",
        display_name: "Zed",
        mcp_instructions_max_chars: None,
        supports_auto_approve: false,
    },
    ClientConstraints {
        id: "opencode",
        display_name: "OpenCode",
        mcp_instructions_max_chars: None,
        supports_auto_approve: false,
    },
    ClientConstraints {
        id: "gemini-cli",
        display_name: "Gemini CLI",
        mcp_instructions_max_chars: None,
        supports_auto_approve: false,
    },
    ClientConstraints {
        id: "codex",
        display_name: "Codex CLI",
        mcp_instructions_max_chars: None,
        supports_auto_approve: false,
    },
];

pub fn by_client_id(id: &str) -> Option<&'static ClientConstraints> {
    let id = id.trim();
    ALL_CLIENTS.iter().find(|c| c.id == id)
}

pub fn by_editor_name(name: &str) -> Option<&'static ClientConstraints> {
    match name {
        "Cursor" => by_client_id("cursor"),
        "Claude Code" => by_client_id("claude-code"),
        "VS Code" => by_client_id("vscode-copilot"),
        "Copilot CLI" => by_client_id("copilot-cli"),
        "Windsurf" => by_client_id("windsurf"),
        "Zed" => by_client_id("zed"),
        "OpenCode" => by_client_id("opencode"),
        "Gemini CLI" => by_client_id("gemini-cli"),
        "Codex CLI" => by_client_id("codex"),
        _ => None,
    }
}
