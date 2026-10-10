//! `init` and `doctor` on an isolated HOME: the JSON reports, where the
//! Claude Code entry lands, the skill, and the agent files left alone.

use std::process::Command;

use sobre::core::setup_report::SetupReport;

fn run_json(bin: &str, args: &[&str], envs: &[(&str, &str)]) -> (i32, String) {
    let mut cmd = Command::new(bin);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("process start");
    let code = out.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    let json_str = extract_json(&stdout).unwrap_or_else(|| {
        eprintln!(
            "--- run_json debug ({} {}) ---\nexit={code}\nstdout[{}]={stdout}\nstderr[{}]={stderr}\n---",
            bin,
            args.join(" "),
            out.stdout.len(),
            out.stderr.len(),
        );
        stdout.clone()
    });
    (code, json_str)
}

fn extract_json(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end >= start {
        Some(s[start..=end].to_string())
    } else {
        None
    }
}

fn write_exe(path: &std::path::Path, content: &str) {
    std::fs::write(path, content).expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "Windows handle inheritance causes subprocess hangs in CI"
)]
fn init_and_doctor_json_reports_and_wiring() {
    let bin = env!("CARGO_BIN_EXE_sobre");

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let home_str = home.to_string_lossy().to_string();

    // A `claude` CLI on PATH that leaves a trace if it is ever run: sobre
    // writes its entry into `.claude.json` itself and never calls the CLI.
    let claude_path = bin_dir.join(if cfg!(windows) {
        "claude.cmd"
    } else {
        "claude"
    });
    if cfg!(windows) {
        write_exe(
            &claude_path,
            "@echo off\r\nsetlocal\r\nset \"OUT=%USERPROFILE%\\claude-mcp.json\"\r\npowershell -NoProfile -Command \"[Console]::In.ReadToEnd() | Set-Content -Path $env:OUT -NoNewline\"\r\nexit /b 0\r\n",
        );
    } else {
        write_exe(
            &claude_path,
            "#!/bin/sh\nset -eu\nOUT=\"$HOME/claude-mcp.json\"\ncat > \"$OUT\"\nexit 0\n",
        );
    }

    let dirs = SeparateDirs::new(tmp.path());

    let mut envs = vec![("HOME", home_str.as_str())];
    envs.extend(dirs.to_env_pairs());

    #[cfg(not(windows))]
    {
        envs.push(("SHELL", "/bin/bash"));
    }
    #[cfg(windows)]
    {
        envs.push(("USERPROFILE", home_str.as_str()));
    }

    // Prefer our fake claude first in PATH.
    let old_path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let new_path = format!("{}{sep}{}", bin_dir.to_string_lossy(), old_path);
    envs.push(("PATH", new_path.as_str()));

    // init --check --json says it is a preview, and writes nothing.
    let (code, out) = run_json(
        bin,
        &["init", "--check", "--json", "--agent", "claude"],
        &envs,
    );
    assert_eq!(code, 0, "init --check exit code");
    let preview: SetupReport = serde_json::from_str(&out).expect("init --check JSON parse");
    assert!(preview.dry_run, "a preview is marked as one: {out}");
    let claude = &preview.steps[0].items[0];
    assert_eq!(claude.status, "would_connect", "{out}");
    assert!(
        claude
            .note
            .as_deref()
            .is_some_and(|n| n.contains("would create ~/.claude.json")),
        "{out}"
    );
    assert!(
        !home.join(".claude.json").exists(),
        "init --check must write nothing"
    );

    // init --repair --json returns clean JSON (SetupReport)
    let (code, out) = run_json(bin, &["init", "--repair", "--json"], &envs);
    assert_eq!(code, 0, "init --repair exit code");
    let setup: SetupReport = serde_json::from_str(&out).unwrap_or_else(|e| {
        panic!(
            "init --repair JSON parse: {e}\nstdout[{}]=<<<{out}>>>",
            out.len()
        )
    });
    assert_eq!(setup.schema_version, 1);

    // init never writes shell files: no env.sh, no shell-hook scripts.
    for f in ["env.sh", "shell-hook.zsh", "shell-hook.bash"] {
        assert!(
            !std::path::Path::new(&dirs.config).join(f).exists(),
            "init must not write {f}"
        );
    }

    // init --agent claude writes the entry into `.claude.json` itself, with
    // the Claude CLI on PATH too: one writer.
    let _ = std::fs::remove_file(home.join(".claude.json"));
    let out = Command::new(bin)
        .args(["init", "--agent", "claude"])
        .envs(envs.iter().copied())
        .output()
        .expect("init --agent claude");
    assert!(out.status.success(), "init --agent claude exit");
    let saved = std::fs::read_to_string(home.join(".claude.json")).expect(".claude.json");
    let v: serde_json::Value = serde_json::from_str(&saved).expect("claude json parse");
    assert!(
        v["mcpServers"]["sobre"].get("command").is_some(),
        "the entry is in .claude.json: {saved}"
    );
    assert!(
        !home.join("claude-mcp.json").exists(),
        "the claude CLI must not be run"
    );

    // doctor --fix --json returns clean JSON (SetupReport shape)
    // Exit code may be 1 if doctor finds unfixable issues (e.g. no real shell profile in CI)
    let (_code, out) = run_json(bin, &["doctor", "--fix", "--json"], &envs);
    let doctor_report: SetupReport = serde_json::from_str(&out).expect("doctor JSON parse");
    assert_eq!(doctor_report.schema_version, 1);

    // doctor opens with the install summary: version and connected agents.
    let out = Command::new(bin)
        .arg("doctor")
        .envs(envs.iter().copied())
        .output()
        .expect("doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(&format!("sobre {}", env!("CARGO_PKG_VERSION"))),
        "doctor must show the version: {stdout}"
    );
    assert!(
        stdout.contains("agents connected:"),
        "doctor must show the connected agents: {stdout}"
    );
}

#[test]
fn claude_config_dir_fallback_writes_dot_claude_json() {
    let bin = env!("CARGO_BIN_EXE_sobre");

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();

    let claude_cfg = tmp.path().join("claude-cfg");
    std::fs::create_dir_all(&claude_cfg).unwrap();

    let home_str = home.to_string_lossy().to_string();
    let claude_cfg_str = claude_cfg.to_string_lossy().to_string();

    // A `claude` CLI on PATH that fails if run.
    let claude_path = bin_dir.join(if cfg!(windows) {
        "claude.cmd"
    } else {
        "claude"
    });
    if cfg!(windows) {
        write_exe(&claude_path, "@echo off\r\nexit /b 1\r\n");
    } else {
        write_exe(&claude_path, "#!/bin/sh\nexit 1\n");
    }

    let dirs = SeparateDirs::new(tmp.path());

    let mut envs = vec![
        ("HOME", home_str.as_str()),
        ("CLAUDE_CONFIG_DIR", claude_cfg_str.as_str()),
    ];
    envs.extend(dirs.to_env_pairs());

    #[cfg(not(windows))]
    {
        envs.push(("SHELL", "/bin/bash"));
    }
    #[cfg(windows)]
    {
        envs.push(("USERPROFILE", home_str.as_str()));
    }

    let old_path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let new_path = format!("{}{sep}{}", bin_dir.to_string_lossy(), old_path);
    envs.push(("PATH", new_path.as_str()));

    let out = Command::new(bin)
        .args(["init", "--agent", "claude"])
        .envs(envs.iter().copied())
        .output()
        .expect("init --agent claude");
    assert!(out.status.success(), "init --agent claude exit");

    let cfg_path = claude_cfg.join(".claude.json");
    let content = std::fs::read_to_string(&cfg_path).expect(".claude.json exists");
    assert!(
        content.contains("\"mcpServers\""),
        "must contain mcpServers"
    );
    assert!(content.contains("sobre"), "must contain sobre entry");

    // `doctor` is a health gate: it exits non-zero whenever *any* check
    // needs attention, and this sandbox legitimately fails several unrelated
    // ones (no sobre on PATH, no shell profile, a fake `claude` that exits 1)
    // — the same reason the JSON smoke test above tolerates a non-zero code. The
    // contract under test is the `.claude.json` fallback *detection*, which is
    // printed to stdout regardless of the gate, so assert on the report and only
    // guard the exit code against a hard error (panic/crash → not 0/1).
    let out = Command::new(bin)
        .args(["doctor"])
        .envs(envs.iter().copied())
        .output()
        .expect("doctor");
    let code = out.status.code().unwrap_or(-1);
    assert!(
        code == 0 || code == 1,
        "doctor must run as a health gate (exit 0 or 1), got {code}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("MCP config") && stdout.contains("sobre found"),
        "doctor should report sobre found in MCP config; got:\n{stdout}"
    );
}

#[test]
fn init_agent_leaves_agent_instruction_files_untouched() {
    let bin = env!("CARGO_BIN_EXE_sobre");

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    // Create a git repo so project files are generated.
    std::fs::create_dir_all(project.join(".git")).unwrap();

    // Existing user AGENTS.md should be preserved.
    let agents_path = project.join("AGENTS.md");
    std::fs::write(&agents_path, "# My Agents\n\nDo not overwrite.\n").unwrap();

    let home_str = home.to_string_lossy().to_string();

    // A `claude` CLI on PATH.
    let claude_path = bin_dir.join(if cfg!(windows) {
        "claude.cmd"
    } else {
        "claude"
    });
    if cfg!(windows) {
        write_exe(&claude_path, "@echo off\r\nrem succeed\r\nexit /b 0\r\n");
    } else {
        write_exe(&claude_path, "#!/bin/sh\nexit 0\n");
    }

    let dirs = SeparateDirs::new(tmp.path());

    let mut envs = vec![("HOME", home_str.as_str())];
    envs.extend(dirs.to_env_pairs());
    #[cfg(not(windows))]
    {
        envs.push(("SHELL", "/bin/bash"));
    }
    #[cfg(windows)]
    {
        envs.push(("USERPROFILE", home_str.as_str()));
    }

    let old_path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ";" } else { ":" };
    let new_path = format!("{}{sep}{}", bin_dir.to_string_lossy(), old_path);
    envs.push(("PATH", new_path.as_str()));

    for _ in 0..2 {
        let out = Command::new(bin)
            .args(["init", "--agent", "claude"])
            .current_dir(&project)
            .envs(envs.iter().copied())
            .output()
            .expect("init --agent claude");
        assert!(out.status.success(), "init --agent claude exit");
    }

    // sobre's only rules channel is the MCP instructions: init writes nothing
    // into the project's own agent files, and creates none.
    assert_eq!(
        std::fs::read_to_string(&agents_path).unwrap(),
        "# My Agents\n\nDo not overwrite.\n",
        "AGENTS.md must be byte-identical"
    );
    for name in ["SOBRE.md", "CLAUDE.md", ".cursorrules", ".windsurfrules"] {
        assert!(!project.join(name).exists(), "init must not create {name}");
    }
}

/// On Windows, `dirs::home_dir()` uses the Win32 API (`SHGetKnownFolderPath`)
/// rather than `USERPROFILE`, so env-var overrides in the subprocess don't
/// control where files land. We can only verify file creation on Unix.
#[test]
#[cfg_attr(windows, ignore = "dirs::home_dir ignores a HOME override on Windows")]
fn init_claude_installs_the_skill_and_no_rules_file() {
    let bin = env!("CARGO_BIN_EXE_sobre");

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();

    let home_str = home.to_string_lossy().to_string();

    let dirs = SeparateDirs::new(tmp.path());

    let mut envs = vec![("HOME", home_str.as_str())];
    envs.extend(dirs.to_env_pairs());
    #[cfg(not(windows))]
    {
        envs.push(("SHELL", "/bin/bash"));
    }
    #[cfg(windows)]
    {
        envs.push(("USERPROFILE", home_str.as_str()));
    }

    let out = Command::new(bin)
        .args(["init", "--agent", "claude"])
        .current_dir(&project)
        .envs(envs.iter().copied())
        .output()
        .expect("init --agent claude");
    assert!(out.status.success(), "init --agent claude exit");

    let skill = std::fs::read_to_string(home.join(".claude/skills/sobre/SKILL.md"))
        .expect("init installs the sobre skill");
    assert!(skill.starts_with("---\nname: sobre\n"), "skill frontmatter");

    assert!(
        !home.join(".claude/CLAUDE.md").exists(),
        "sobre writes no rules into CLAUDE.md"
    );
    assert!(
        !project.join("CLAUDE.md").exists(),
        "must not create project CLAUDE.md"
    );
    // Native tools are never denied.
    let settings = std::fs::read_to_string(home.join(".claude/settings.json")).unwrap_or_default();
    assert!(
        !settings.contains("\"deny\""),
        "settings.json denies nothing: {settings}"
    );
}

/// One directory per SOBRE_<CATEGORY>_DIR, so a file written to the wrong
/// category is caught.
struct SeparateDirs {
    config: String,
    data: String,
    state: String,
    cache: String,
}

impl SeparateDirs {
    fn new(tmp_root: &std::path::Path) -> Self {
        let config = tmp_root.join("config");
        let data = tmp_root.join("data");
        let state = tmp_root.join("state");
        let cache = tmp_root.join("cache");
        for d in [&config, &data, &state, &cache] {
            std::fs::create_dir_all(d).unwrap();
        }
        SeparateDirs {
            config: config.to_string_lossy().to_string(),
            data: data.to_string_lossy().to_string(),
            state: state.to_string_lossy().to_string(),
            cache: cache.to_string_lossy().to_string(),
        }
    }

    fn to_env_pairs(&self) -> Vec<(&str, &str)> {
        vec![
            ("SOBRE_CONFIG_DIR", self.config.as_str()),
            ("SOBRE_DATA_DIR", self.data.as_str()),
            ("SOBRE_STATE_DIR", self.state.as_str()),
            ("SOBRE_CACHE_DIR", self.cache.as_str()),
        ]
    }
}
