//! Live client-wiring smoke.
//!
//! Boots the REAL binary end-to-end, in an isolated HOME:
//!   1. `sobre init --agent <client>` installs the MCP registration and the
//!      skill, and writes no rules into the agent's own files,
//!   2. `sobre mcp` is spoken to over actual stdio JSON-RPC,
//!   3. every client gets the same `initialize` instructions — the one rules
//!      channel — and the same `tools/list` surface.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

struct TestEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
    data: std::path::PathBuf,
    project: std::path::PathBuf,
}

fn test_env() -> TestEnv {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let data = tmp.path().join("data");
    let project = tmp.path().join("project");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(project.join(".git")).unwrap();
    TestEnv {
        _tmp: tmp,
        home,
        data,
        project,
    }
}

fn base_cmd(bin: &str, env: &TestEnv) -> Command {
    let mut cmd = Command::new(bin);
    cmd.current_dir(&env.project)
        .env("HOME", &env.home)
        .env("SOBRE_DATA_DIR", &env.data)
        .env("CODEX_HOME", env.home.join(".codex"));
    cmd
}

fn run_init(bin: &str, env: &TestEnv, agent: &str) {
    let out = base_cmd(bin, env)
        .args(["init", "--agent", agent])
        .output()
        .expect("init spawn");
    assert!(
        out.status.success(),
        "init --agent {agent} failed:\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Speak minimal MCP over the child's stdio: initialize (with `client_name`),
/// then tools/list. Returns (instructions, advertised tool names).
fn mcp_handshake(bin: &str, env: &TestEnv, client_name: &str) -> (String, Vec<String>) {
    let mut child: Child = base_cmd(bin, env)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("mcp server spawn");

    let mut stdin = child.stdin.take().expect("child stdin");
    let stdout = child.stdout.take().expect("child stdout");

    // Reader thread: forward every stdout line; a deadline on the receiving
    // side keeps a wedged server from hanging the suite.
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(l) => {
                    if tx.send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let recv_response = |id: u64| -> serde_json::Value {
        let needle = format!("\"id\":{id}");
        let deadline = std::time::Instant::now() + Duration::from_mins(1);
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .unwrap_or_else(|| {
                    panic!("timeout waiting for JSON-RPC response id={id}");
                });
            let line = rx
                .recv_timeout(remaining)
                .unwrap_or_else(|e| panic!("no response id={id} within deadline: {e}"));
            if line.contains(&needle) {
                return serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("invalid JSON-RPC line: {e}\n{line}"));
            }
        }
    };

    let init = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": client_name, "version": "1.0.0" }
        }
    });
    writeln!(stdin, "{init}").expect("write initialize");
    let init_res = recv_response(1);
    let instructions = init_res["result"]["instructions"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !instructions.is_empty(),
        "initialize must return instructions; got: {init_res}"
    );

    writeln!(
        stdin,
        "{}",
        serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
    )
    .expect("write initialized");

    writeln!(
        stdin,
        "{}",
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })
    )
    .expect("write tools/list");
    let tools_res = recv_response(2);
    let tools: Vec<String> = tools_res["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list must return tools; got: {tools_res}"))
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_string))
        .collect();

    drop(stdin); // EOF → clean server shutdown
    let _ = child.wait();
    let _ = reader.join();

    (instructions, tools)
}

fn assert_full_surface(tools: &[String], client: &str) {
    // Invariant 3: every client sees the same surface, and it is the whole
    // registry. This used to mirror a per-client quirk that hid `ctx_patch`
    // from clients with a native editor.
    let registered: Vec<String> = sobre::server::registry::build_registry()
        .tool_defs()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    for expected in &registered {
        assert!(
            tools.iter().any(|t| t == expected),
            "{client}: {expected} must be advertised; got {tools:?}"
        );
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "HOME-override isolation is Unix-only (dirs::home_dir uses the Win32 API)"
)]
fn every_client_gets_the_same_instructions_and_no_rules_file() {
    let bin = env!("CARGO_BIN_EXE_sobre");
    let env = test_env();

    for agent in ["cursor", "claude", "codex"] {
        run_init(bin, &env, agent);
    }

    // Wiring on disk: MCP registrations, and no rules written anywhere.
    let mcp_json = std::fs::read_to_string(env.home.join(".cursor/mcp.json")).expect("mcp.json");
    assert!(mcp_json.contains("sobre"), "mcp.json must register us");
    assert!(
        !mcp_json.contains("instructions"),
        "the MCP entry carries no copy of the instructions"
    );
    let config = std::fs::read_to_string(env.home.join(".codex/config.toml")).expect("config");
    assert!(config.contains("sobre"), "config.toml must register us");
    for rules in [
        ".cursor/rules/sobre.mdc",
        ".claude/CLAUDE.md",
        ".codex/instructions.md",
        ".codex/AGENTS.md",
        ".codex/SOBRE.md",
    ] {
        assert!(
            !env.home.join(rules).exists(),
            "init must not write {rules}"
        );
    }
    for name in ["AGENTS.md", "SOBRE.md", "CLAUDE.md"] {
        assert!(
            !env.project.join(name).exists(),
            "init must not write {name}"
        );
    }

    let mut seen = Vec::new();
    for client in ["cursor", "claude-code", "codex"] {
        let (instructions, tools) = mcp_handshake(bin, &env, client);
        assert!(
            instructions.contains("Prefer them when they fit; native tools keep working"),
            "{client} must get the guide.\n{instructions}"
        );
        assert_full_surface(&tools, client);
        seen.push(instructions);
    }
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "every client gets byte-identical instructions"
    );
}
