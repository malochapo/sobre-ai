//! Fast-initialize contract.
//!
//! VS Code's start-on-demand MCP lifecycle races the first tool call of a
//! fresh conversation against server startup (microsoft/vscode#321150). The
//! server-side mitigation: nothing that spawns processes or opens sockets may
//! run in front of the `initialize` handshake — housekeeping (orphan sweep,
//! proxy autostart, publish throttle) is deferred onto the blocking pool.
//!
//! Two guards, real binary over real stdio JSON-RPC in an isolated HOME:
//!   1. spawn → initialize-response stays under a conservative wall-clock
//!      bound (catches synchronous work in front of the handshake, such as a
//!      crash-backoff sleep or a network wait),
//!   2. a tools/call fired IMMEDIATELY after the initialized notification —
//!      the exact VS Code race pattern — succeeds on the first attempt.
//!
//! A third test pins the transport's tolerance: a line that is not a valid
//! MCP message must not close the session, or the host respawns the server
//! and the fresh process pays for its indexes again.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Generous for CI (debug binary, cold cache, shared runners) yet far below
/// the pathological regressions this guards against (30s crash-loop backoff,
/// TCP timeouts, N×ps orphan sweeps in front of the handshake).
const INITIALIZE_DEADLINE: Duration = Duration::from_secs(10);

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
    std::fs::write(project.join("hello.txt"), "hello fast init\n").unwrap();
    TestEnv {
        _tmp: tmp,
        home,
        data,
        project,
    }
}

/// A `sobre mcp` child over piped stdio, with a thread forwarding each line
/// it writes to stdout.
struct Server {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    reader: std::thread::JoinHandle<()>,
}

impl Server {
    fn spawn(env: &TestEnv) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sobre"))
            .arg("mcp")
            .current_dir(&env.project)
            .env("HOME", &env.home)
            .env("SOBRE_DATA_DIR", &env.data)
            .env("CODEX_HOME", env.home.join(".codex"))
            // Root detection must derive from the temp project's cwd. When the
            // suite itself runs inside an IDE/agent session these carry the HOST
            // workspace and would hijack the project root.
            .env_remove("CLAUDE_PROJECT_DIR")
            .env_remove("WORKSPACE_FOLDER_PATHS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("mcp server spawn");

        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let (tx, lines) = mpsc::channel::<String>();
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
        Self {
            child,
            stdin,
            lines,
            reader,
        }
    }

    fn send(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").expect("write to the server");
    }

    fn recv_response(&self, id: u64, deadline: Duration) -> serde_json::Value {
        let needle = format!("\"id\":{id}");
        let until = Instant::now() + deadline;
        loop {
            let remaining = until
                .checked_duration_since(Instant::now())
                .unwrap_or_else(|| panic!("timeout waiting for JSON-RPC response id={id}"));
            let line = self
                .lines
                .recv_timeout(remaining)
                .unwrap_or_else(|e| panic!("no response id={id} within deadline: {e}"));
            if line.contains(&needle) {
                return serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("invalid JSON-RPC line: {e}\n{line}"));
            }
        }
    }

    /// Sends `initialize` (id 1) and returns its response.
    fn initialize(&mut self, deadline: Duration) -> serde_json::Value {
        let init = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "vscode", "version": "1.109.0" }
            }
        });
        self.send(&init.to_string());
        self.recv_response(1, deadline)
    }

    /// Closes stdin (EOF → clean server shutdown) and waits for the process.
    fn finish(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
        let _ = self.reader.join();
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "HOME-override isolation is Unix-only (dirs::home_dir uses the Win32 API)"
)]
fn initialize_answers_fast_and_first_call_succeeds() {
    let env = test_env();

    let spawn_at = Instant::now();
    let mut server = Server::spawn(&env);
    let init_res = server.initialize(INITIALIZE_DEADLINE);
    let elapsed = spawn_at.elapsed();
    assert!(
        init_res["result"]["serverInfo"]["name"].is_string(),
        "initialize must return serverInfo; got: {init_res}"
    );
    assert!(
        elapsed < INITIALIZE_DEADLINE,
        "spawn → initialize-response took {elapsed:?} (bound {INITIALIZE_DEADLINE:?}) — \
         synchronous work runs in front of the handshake"
    );

    // The VS Code race pattern: initialized notification and the first tool
    // call back-to-back, no grace period. It must succeed first try.
    server.send(
        &serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
    );
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {
            "name": "sobre_read",
            "arguments": { "path": env.project.join("hello.txt").to_string_lossy() }
        }
    });
    server.send(&call.to_string());
    let call_res = server.recv_response(2, Duration::from_secs(30));
    assert!(
        call_res["error"].is_null(),
        "first tools/call immediately after initialized must succeed; got: {call_res}"
    );
    let text = call_res["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        text.contains("hello fast init"),
        "sobre_read must deliver the file on the first post-initialize call; got: {call_res}"
    );

    server.finish();
}

#[test]
#[cfg_attr(
    windows,
    ignore = "HOME-override isolation is Unix-only (dirs::home_dir uses the Win32 API)"
)]
fn lines_that_are_not_mcp_messages_leave_the_session_open() {
    let env = test_env();
    let mut server = Server::spawn(&env);
    server.initialize(INITIALIZE_DEADLINE);
    server.send(
        &serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
    );

    // Not JSON; a notification outside MCP; JSON that is not JSON-RPC.
    server.send("{ this is : not json }");
    server.send(r#"{"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":7}}"#);
    server.send(r#"{"foo":"bar"}"#);

    server.send(
        &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
    );
    let res = server.recv_response(2, Duration::from_secs(30));
    let tools = res["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list must still answer; got: {res}"));
    assert_eq!(
        tools.len(),
        sobre::server::registry::build_registry().tool_defs().len(),
        "tools/list must return the whole registry"
    );

    server.finish();
}
