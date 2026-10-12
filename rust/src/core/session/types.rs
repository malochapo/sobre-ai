/// Where this server process works: the project root, the shell's current
/// directory and the extra roots of a multi-root workspace. Lives in memory
/// only; every process starts empty.
#[derive(Clone, Debug, Default)]
pub struct SessionState {
    pub project_root: Option<String>,
    pub shell_cwd: Option<String>,
    /// Extra project roots for multi-root workspaces: config `search_roots`,
    /// the host's workspace folders and MCP `roots/list`.
    pub extra_roots: Vec<String>,
}
