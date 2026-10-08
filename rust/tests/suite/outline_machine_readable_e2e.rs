//! #990 e2e — machine-readable purity, and no unrequested text.
//!
//! `sobre_outline format=json` must return byte-exact, parseable JSON with no
//! prose decoration, and no call — first or later — carries text the agent
//! did not ask for.
//!
//! The project root is pinned via `CLAUDE_PROJECT_DIR`, so there is no async
//! client-roots race.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

const SAMPLE_RS: &str = r"//! Sample module for sobre_outline e2e.

pub struct Engine {
    pub name: String,
    pub power: u32,
}

impl Engine {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), power: 0 }
    }

    pub fn ignite(&mut self) -> bool {
        self.power = 100;
        true
    }
}

pub trait Drivable {
    fn drive(&self) -> String;
}

pub enum Gear {
    Park,
    Drive,
    Reverse,
}

pub fn accelerate(engine: &mut Engine, delta: u32) -> u32 {
    engine.power = engine.power.saturating_add(delta);
    engine.power
}
";

/// Fully isolated sobre environment with a one-file project tree.
struct McpSandbox {
    _root: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
    cache: PathBuf,
    project: PathBuf,
}

impl McpSandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        let config = root.path().join("config");
        let data = root.path().join("data");
        let state = root.path().join("state");
        let cache = root.path().join("cache");
        let project = root.path().join("project");
        for d in [&home, &config, &data, &state, &cache, &project] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(project.join("sample.rs"), SAMPLE_RS).unwrap();
        // Canonicalize so `index build --root`, `CLAUDE_PROJECT_DIR` and the
        // server's cwd resolve to the exact same path (macOS /tmp -> /private/tmp).
        let project = project.canonicalize().unwrap();
        Self {
            _root: root,
            home,
            config,
            data,
            state,
            cache,
            project,
        }
    }

    fn base_command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sobre"));
        cmd.env("HOME", &self.home)
            .env("SOBRE_CONFIG_DIR", &self.config)
            .env("SOBRE_DATA_DIR", &self.data)
            .env("SOBRE_STATE_DIR", &self.state)
            .env("SOBRE_CACHE_DIR", &self.cache)
            // Resolve the project root synchronously from the host env, as
            // Claude Code sets it (no async
            // client-roots / cwd race) so the auto-context overview is live on
            // the very FIRST tool call. This makes the briefing-survival check
            // discriminating: the root is ready when the json call runs, so the
            // unfixed dispatch would burn the once-per-session flag right there.
            .env("CLAUDE_PROJECT_DIR", &self.project);
        cmd
    }

    /// Synchronously build the project graph + BM25 so the overview warms fast.
    fn build_index(&self) {
        let out = self
            .base_command()
            .args(["index", "build", "--root"])
            .arg(&self.project)
            .output()
            .expect("spawn sobre index build");
        assert!(
            out.status.success(),
            "index build failed: {}\nstdout:\n{}\nstderr:\n{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }
}

/// A live `mcp` stdio session driven request/response so we can poll for
/// readiness before asserting on order-sensitive, once-per-session behaviour.
struct McpSession {
    child: Child,
    stdin: Option<ChildStdin>,
    reader: BufReader<ChildStdout>,
}

impl McpSession {
    fn start(sandbox: &McpSandbox) -> Self {
        let mut child = sandbox
            .base_command()
            .arg("mcp")
            .current_dir(&sandbox.project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sobre mcp");
        let stdin = child.stdin.take().expect("mcp stdin");
        let stdout = child.stdout.take().expect("mcp stdout");
        let mut session = Self {
            child,
            stdin: Some(stdin),
            reader: BufReader::new(stdout),
        };
        session.handshake();
        session
    }

    fn send(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    /// Read stdout until the JSON-RPC response with `id` arrives, skipping any
    /// server-initiated notifications (which carry no `id`).
    fn read_for(&mut self, id: i64) -> Value {
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).expect("read mcp stdout");
            assert!(n != 0, "mcp closed stdout before responding to id {id}");
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(line)
                && v.get("id").and_then(Value::as_i64) == Some(id)
            {
                return v;
            }
        }
    }

    fn handshake(&mut self) {
        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "e2e", "version": "1.0" }
            }
        })
        .to_string();
        self.send(&init);
        self.read_for(1);
        self.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string());
    }

    fn call(&mut self, id: i64, name: &str, args: &Value) -> String {
        self.send(
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": name, "arguments": args }
            })
            .to_string(),
        );
        let resp = self.read_for(id);
        text_content(&resp).unwrap_or_default()
    }

    /// Poll `sobre_compose` without a task (the session-start overview) until the
    /// project graph is ready. That call counts as the session briefing, so a
    /// session warmed this way gets no further auto-context.
    fn shutdown(mut self) {
        self.stdin.take();
        let _ = self.child.wait();
    }
}

fn text_content(resp: &Value) -> Option<String> {
    resp.get("result")?
        .get("content")?
        .as_array()?
        .first()?
        .get("text")?
        .as_str()
        .map(str::to_string)
}

#[test]
fn json_call_is_pure_and_no_call_carries_a_briefing() {
    let sandbox = McpSandbox::new();
    sandbox.build_index();

    let mut session = McpSession::start(&sandbox);

    // Call A is json (machine-readable) and the session's first tool call; B
    // and C are the same plain-text outline.
    let call_a = session.call(
        2,
        "sobre_outline",
        &json!({ "path": "sample.rs", "format": "json" }),
    );
    let call_b = session.call(3, "sobre_outline", &json!({ "path": "sample.rs" }));
    let call_c = session.call(4, "sobre_outline", &json!({ "path": "sample.rs" }));
    session.shutdown();

    // (1) Machine-readable purity: A is byte-exact JSON, no prose decoration.
    assert!(
        call_a.trim_start().starts_with('{'),
        "json call must start with '{{', got:\n{call_a}"
    );
    let parsed: Value = serde_json::from_str(&call_a)
        .unwrap_or_else(|e| panic!("json call must parse ({e}):\n{call_a}"));
    assert_eq!(
        parsed["backend"], "tree-sitter",
        "outline must be AST-backed, not regex"
    );
    assert!(
        parsed["symbols"].as_array().is_some_and(|s| !s.is_empty()),
        "outline must list symbols:\n{call_a}"
    );

    // (2) No call carries text the agent did not ask for: the first text call
    // is the tool's output alone, the same as the next identical call.
    assert_eq!(
        call_b, call_c,
        "the first text call must not carry a once-per-session prefix"
    );
}
