//! Entrypoint smoke gate: every advertised entrypoint must be reachable.
//!
//! Background: the v3.4.7 release shipped tools/commands whose *implementation*
//! existed but whose *entrypoint* was not wired — `sobre pack` / `index` fell
//! through to the global help, and some tools were missing from the manifest.
//! "Implemented" must imply "invokable". Two independent surfaces are guarded:
//!
//! 1. MCP — key entrypoint tools are present in the manifest SSOT.
//! 2. CLI — top-level subcommands route to their handler instead of falling
//!    through to the global help (the exact `pack`/`index` regression).
//!
//! Dispatch on the MCP side is registry-driven (`registry.get_arc(name)`), and
//! the generated tool reference is checked against the registry by
//! `reference_docs_drift`. This gate adds the *advertised entrypoint* and *CLI
//! wiring* dimensions.

use std::process::Command;

use serde_json::Value;

/// Collect every `"name"` string field anywhere in the manifest JSON. Tool names
/// (`sobre_*`) only ever appear as tool entry names, so membership is unambiguous.
fn collect_names(value: &Value, out: &mut std::collections::BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(n)) = map.get("name") {
                out.insert(n.clone());
            }
            for v in map.values() {
                collect_names(v, out);
            }
        }
        Value::Array(items) => {
            for v in items {
                collect_names(v, out);
            }
        }
        _ => {}
    }
}

#[test]
fn mcp_entrypoint_tools_are_advertised() {
    let manifest = sobre::core::mcp_manifest::manifest_value();
    let mut names = std::collections::BTreeSet::new();
    collect_names(&manifest, &mut names);

    assert!(
        !names.is_empty(),
        "manifest advertises no tools — manifest_value() produced an empty surface"
    );

    // The entrypoint tools that back the regressed CLI commands plus the core
    // read/search surface. If a handler exists but is not advertised here, the
    // tool is unreachable for MCP clients (the v3.4.7 "missing manifest entry").
    let required = [
        "sobre_read",
        "sobre_shell",
        "sobre_search",
        "sobre_compose",
        "sobre_graph",
        "sobre_knowledge",
        "sobre_session",
        "sobre_patch",
    ];
    for tool in required {
        assert!(
            names.contains(tool),
            "entrypoint tool '{tool}' is not advertised in the manifest: \
             register it in the tool registry"
        );
    }
}

fn sobre() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sobre"));
    // Mirror cli_characterization.rs: sandbox the binary so subcommands are
    // side-effect-free (no background process, no real HOME writes).
    cmd.env("HOME", "/tmp/sobre-entrypoint-test");
    cmd
}

/// Run the sandboxed binary; return trimmed stdout, trimmed stderr and the exit
/// code. Stderr matters because an unknown command is reported there, so the
/// wiring detector keys off it.
fn run(args: &[&str]) -> (String, String, i32) {
    let out = sobre()
        .args(args)
        .output()
        .expect("failed to spawn sobre binary");
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    (stdout, stderr, out.status.code().unwrap_or(-1))
}

#[test]
fn cli_subcommands_are_wired() {
    // An unknown command is reported on stderr — clap's `unrecognized
    // subcommand '<cmd>'` — with empty stdout and exit 2. That stderr marker is
    // the *signature* of "not wired" — the exact v3.4.7 `pack`/`index`
    // regression. Derive it from a token that can never be a real command, so
    // the detector validates itself.
    let (stdout, stderr, code) = run(&["__sobre_not_a_command__"]);
    assert!(
        stdout.is_empty()
            && stderr.contains("unrecognized subcommand '__sobre_not_a_command__'")
            && code == 2,
        "unknown-command report changed (stdout={stdout:?}, stderr={stderr:?}, \
         exit={code}); the wiring detector below keys off the stderr marker"
    );

    // Probe each command with a *garbage subcommand* — never `--help`. A
    // declared command parses its own arguments and never emits the top-level
    // `unrecognized subcommand '<cmd>'` marker; an undeclared one does. Set =
    // every command with subcommands, each side-effect-free on an unknown
    // subcommand. That every declared command dispatches is checked by the
    // compiler (the dispatch `match` on the clap enum is exhaustive); the
    // *full* tool surface is guarded MCP-side by
    // `mcp_entrypoint_tools_are_advertised`.
    let probe = "__sobre_wiring_probe__";
    for cmd in ["index", "config", "embeddings", "proxy", "hook"] {
        let (_stdout, stderr, _code) = run(&[cmd, probe]);
        assert!(
            !stderr.contains(&format!("unrecognized subcommand '{cmd}'")),
            "`sobre {cmd}` is not a declared command — it is not wired in \
             cli/args.rs (the v3.4.7 entrypoint regression)."
        );
    }
}
