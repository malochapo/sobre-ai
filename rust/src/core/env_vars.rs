//! Every environment variable sobre reads or sets, in one registry (decision
//! 9). `docs/reference/generated/env-vars.md` is rendered from it, and a test
//! (`every_variable_the_code_reads_is_registered`) fails when the code reads a
//! variable this list does not name.
//!
//! Invariant 2: a variable may say where things are or carry a secret; none
//! changes what sobre does. The few that look like exceptions are listed with
//! what makes them one (a re-entry marker sobre sets on its own children, a
//! signal a host sets).

/// What kind of variable it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Where sobre keeps its files.
    Location,
    /// A secret.
    Secret,
    /// Where an agent keeps its files, or what a host tells sobre.
    Host,
    /// Read from the platform: shell, locale, user, library paths.
    Platform,
    /// Set by sobre, on its children or in the files it writes.
    SetBySobre,
    /// Read by tests only.
    Test,
}

impl Kind {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Location => "Where sobre keeps its files",
            Self::Secret => "Secrets",
            Self::Host => "Agents and hosts",
            Self::Platform => "Platform",
            Self::SetBySobre => "Set by sobre",
            Self::Test => "Tests only",
        }
    }
}

pub(crate) struct EnvVar {
    /// The name, or a prefix ending in `_` for a family (`CODEX_`).
    pub name: &'static str,
    pub kind: Kind,
    pub meaning: &'static str,
}

const fn var(name: &'static str, kind: Kind, meaning: &'static str) -> EnvVar {
    EnvVar {
        name,
        kind,
        meaning,
    }
}

pub(crate) const VARS: &[EnvVar] = &[
    var(
        "SOBRE_CONFIG_DIR",
        Kind::Location,
        "Directory of config.toml (default `$XDG_CONFIG_HOME/sobre`, else `~/.config/sobre`)",
    ),
    var(
        "SOBRE_DATA_DIR",
        Kind::Location,
        "Usage ledger, sessions, knowledge, indexes, checkpoints, models (default \
         `$XDG_DATA_HOME/sobre`, else `~/.local/share/sobre`)",
    ),
    var(
        "SOBRE_STATE_DIR",
        Kind::Location,
        "Logs, ports, captured agent variables (default `$XDG_STATE_HOME/sobre`, \
         else `~/.local/state/sobre`)",
    ),
    var(
        "SOBRE_CACHE_DIR",
        Kind::Location,
        "Rebuildable caches (default `$XDG_CACHE_HOME/sobre`, else `~/.cache/sobre`)",
    ),
    var(
        "XDG_CONFIG_HOME",
        Kind::Location,
        "Base of the config directory",
    ),
    var(
        "XDG_DATA_HOME",
        Kind::Location,
        "Base of the data directory",
    ),
    var(
        "XDG_STATE_HOME",
        Kind::Location,
        "Base of the state directory",
    ),
    var(
        "XDG_CACHE_HOME",
        Kind::Location,
        "Base of the cache directory",
    ),
    var("HOME", Kind::Location, "The home directory"),
    var(
        "USERPROFILE",
        Kind::Location,
        "The home directory on Windows",
    ),
    var(
        "HOMEDRIVE",
        Kind::Location,
        "With HOMEPATH, the home directory on Windows when USERPROFILE is unset",
    ),
    var("HOMEPATH", Kind::Location, "See HOMEDRIVE"),
    var(
        "APPDATA",
        Kind::Location,
        "Windows: where Claude Desktop, VS Code, Windsurf and Zed keep their config",
    ),
    var(
        "ORT_DYLIB_PATH",
        Kind::Location,
        "Build with local embeddings: the ONNX Runtime library to load. Wins over \
         the one `sobre embeddings provision` installs; a relative path is read \
         from the current directory",
    ),
    var(
        "SOBRE_PROXY_TOKEN",
        Kind::Secret,
        "The proxy's bearer token. Unset: sobre generates one and keeps it in the \
         data directory (`sobre proxy token` prints it)",
    ),
    var(
        "CLAUDE_CONFIG_DIR",
        Kind::Host,
        "Where Claude Code keeps its config (default `~/.claude`)",
    ),
    var(
        "CODEX_HOME",
        Kind::Host,
        "Where Codex keeps its config (default `~/.codex`)",
    ),
    var(
        "CODEX_PROFILE",
        Kind::Host,
        "The Codex profile whose config `init codex` edits",
    ),
    var(
        "COPILOT_HOME",
        Kind::Host,
        "Where Copilot CLI keeps its config (default `~/.copilot`)",
    ),
    var(
        "CLAUDE_PROJECT_DIR",
        Kind::Host,
        "Set by Claude Code: the project root when the client sends no MCP roots",
    ),
    var(
        "WORKSPACE_FOLDER_PATHS",
        Kind::Host,
        "Set by Cursor: the workspace folders, used as project roots",
    ),
    var(
        "CURSOR_TASK_ID",
        Kind::Host,
        "Set by Cursor for a subagent: its reads get their own re-read stubs",
    ),
    var(
        "ANTHROPIC_API_KEY",
        Kind::Host,
        "Its presence tells `proxy enable` that Claude Code and OpenCode use an \
         API key; the value is never read or stored",
    ),
    var(
        "ANTHROPIC_AUTH_TOKEN",
        Kind::Host,
        "As ANTHROPIC_API_KEY, for Claude Code",
    ),
    var(
        "CODEX_API_KEY",
        Kind::Host,
        "Its presence tells `proxy enable` that Codex uses an API key; the value is \
         never read or stored",
    ),
    var(
        "CODEX_",
        Kind::Host,
        "Variables starting CODEX_, CLAUDE_, OPENCODE_ or GEMINI_ (credentials \
         excluded) that a hook or `sobre -c` sees are kept in \
         `state/agent_runtime_env.json` and passed to `sobre_shell` commands, which \
         the long-lived MCP server would not otherwise see (CODEX_THREAD_ID, …)",
    ),
    var("CLAUDE_", Kind::Host, "See CODEX_"),
    var("OPENCODE_", Kind::Host, "See CODEX_"),
    var("GEMINI_", Kind::Host, "See CODEX_"),
    var(
        "SHELL",
        Kind::Platform,
        "The shell `sobre_shell` runs; a non-POSIX shell (fish, nu) is replaced by \
         zsh, bash or sh",
    ),
    var(
        "COMSPEC",
        Kind::Platform,
        "Windows: the shell when no PowerShell is found",
    ),
    var(
        "MSYSTEM",
        Kind::Platform,
        "Windows: set under Git Bash/MSYS, where bash is preferred",
    ),
    var("MINGW_PREFIX", Kind::Platform, "As MSYSTEM"),
    var(
        "PATH",
        Kind::Platform,
        "Where `doctor` and `sobre_refactor` look for binaries",
    ),
    var(
        "LANG",
        Kind::Platform,
        "With LC_ALL and LC_CTYPE: when none names a UTF-8 locale, commands run \
         with one",
    ),
    var("LC_ALL", Kind::Platform, "See LANG"),
    var("LC_CTYPE", Kind::Platform, "See LANG"),
    var("NO_COLOR", Kind::Platform, "Set: no ANSI colours"),
    var(
        "RUST_LOG",
        Kind::Platform,
        "Log filter for sobre's own diagnostics (stderr, the proxy log)",
    ),
    var(
        "USER",
        Kind::Platform,
        "Linux, build with local embeddings: the per-user Nix profile searched for \
         ONNX Runtime",
    ),
    var(
        "USERNAME",
        Kind::Platform,
        "Windows: limits the stray-process cleanup to this user's processes",
    ),
    var(
        "HOMEBREW_PREFIX",
        Kind::Platform,
        "Build with local embeddings: where Homebrew installs ONNX Runtime",
    ),
    var(
        "LD_LIBRARY_PATH",
        Kind::Platform,
        "Build with local embeddings: searched for ONNX Runtime (DYLD_LIBRARY_PATH on \
         macOS)",
    ),
    var("DYLD_LIBRARY_PATH", Kind::Platform, "See LD_LIBRARY_PATH"),
    var(
        "SOBRE_WRAPPED",
        Kind::SetBySobre,
        "Set on every command sobre runs. A `sobre -c` that sees it passes the \
         command through uncompressed, so a nested call never compresses twice. \
         Any process can set it, with that effect",
    ),
    var(
        "SOBRE_SEATBELT",
        Kind::SetBySobre,
        "macOS: set on the proxy started by its LaunchAgent, which runs under \
         `sandbox-exec` with ~/Documents, ~/Desktop and ~/Downloads denied, so a \
         background process never triggers a privacy prompt",
    ),
    var(
        "BASH_ENV",
        Kind::SetBySobre,
        "Set empty, with ENV, on the shells sobre starts, so no startup file runs",
    ),
    var("ENV", Kind::SetBySobre, "See BASH_ENV"),
    var(
        "ANTHROPIC_BASE_URL",
        Kind::SetBySobre,
        "Written into Claude Code's settings by `proxy enable`, removed by `proxy \
         disable`",
    ),
    var(
        "SOBRE_TCC_STANDALONE",
        Kind::Test,
        "Unit tests: forces the macOS standalone-process check",
    ),
    var(
        "SOBRE_HANDOFF_TEST_PORT",
        Kind::Test,
        "A helper process of the proxy handoff test",
    ),
    var(
        "SOBRE_PERF_GATE",
        Kind::Test,
        "Set to 1: runs the BM25 performance gate",
    ),
    var(
        "SOBRE_UPDATE_FROZEN_HASHES",
        Kind::Test,
        "Rewrites docs/contracts/frozen-hashes.json (see CONTRIBUTING)",
    ),
];

/// `docs/reference/generated/env-vars.md`.
pub(crate) fn markdown() -> String {
    let mut out = String::from("# Appendix — Environment Variables (generated)\n\n");
    out.push_str(
        "<!-- GENERATED FILE — do not edit by hand. Run: \
         `cargo run --example gen_docs --features dev-tools` -->\n\n",
    );
    out.push_str(
        "Source of truth: `rust/src/core/env_vars.rs`; a test fails when the code \
         reads a variable it does not list. No variable changes what sobre does \
         (invariant 2): they say where files are, carry a secret, or come from \
         the host. Settings live in config.toml only.\n\n",
    );
    for kind in [
        Kind::Location,
        Kind::Secret,
        Kind::Host,
        Kind::Platform,
        Kind::SetBySobre,
        Kind::Test,
    ] {
        out.push_str(&format!(
            "## {}\n\n| Variable | Meaning |\n|---|---|\n",
            kind.title()
        ));
        for v in VARS.iter().filter(|v| v.kind == kind) {
            let name = if v.name.ends_with('_') {
                format!("`{}*`", v.name)
            } else {
                format!("`{}`", v.name)
            };
            out.push_str(&format!("| {name} | {} |\n", v.meaning));
        }
        out.push('\n');
    }
    out
}

/// Whether `name` is in the registry, by name or by family prefix.
#[cfg(test)]
fn registered(name: &str) -> bool {
    VARS.iter()
        .any(|v| v.name == name || (v.name.ends_with('_') && name.starts_with(v.name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variable the code reads (`var("X")`, `var_os("X")`, a
    /// `category_dir`/`resolve_proxy_token` name) and every `SOBRE_*` name
    /// outside the tests is in [`VARS`].
    #[test]
    fn every_variable_the_code_reads_is_registered() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let read = regex::Regex::new(
            r#"(?:var|var_os|category_dir|resolve_proxy_token)\(\s*"([A-Z][A-Z0-9_]*)""#,
        )
        .unwrap();
        let ours = regex::Regex::new(r#""(SOBRE_[A-Z0-9_]+)""#).unwrap();
        let mut missing = Vec::new();
        for entry in walkdir(&src) {
            // A Windows checkout may turn the line endings into CRLF.
            let text = std::fs::read_to_string(&entry)
                .unwrap_or_default()
                .replace("\r\n", "\n");
            // Only the code: a test module's names are fixtures.
            let code = text.split("#[cfg(test)]\nmod ").next().unwrap_or(&text);
            for cap in read.captures_iter(code).chain(ours.captures_iter(code)) {
                let name = cap[1].to_string();
                if !registered(&name) {
                    missing.push(format!("{name} ({})", entry.display()));
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(
            missing.is_empty(),
            "not in core::env_vars::VARS: {missing:?}"
        );
    }

    fn walkdir(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        out
    }

    #[test]
    fn names_are_unique() {
        let mut names: Vec<&str> = VARS.iter().map(|v| v.name).collect();
        names.sort_unstable();
        let n = names.len();
        names.dedup();
        assert_eq!(names.len(), n);
    }
}
