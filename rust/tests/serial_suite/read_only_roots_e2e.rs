//! #475 end-to-end: a configured read-only root (`read_only_roots` /
//! `LEAN_CTX_READ_ONLY_ROOTS`) is fully **readable** through the real
//! `ContextEngine` tool dispatch — the same path the MCP server drives — yet
//! **no write tool can mutate it**, while ordinary writes inside the project
//! root keep working.
//!
//! This is the integration-level counterpart to the unit guards at every write
//! choke point: it exercises `resolve_path` (jail allow-list widened by the
//! read-only roots) → tool `handle` → `pathjail::enforce_writable`, proving the
//! tier holds across the *whole* request, not just the low-level primitive.
//!
//! A prior attempt (#464) was rejected because writes could escape the
//! read-only tier; this test is the regression gate against that recurring.
#![cfg(not(feature = "no-jail"))]
// The test_env_lock guard is intentionally held across the async tool calls to
// serialize the process-global env mutation (LEAN_CTX_READ_ONLY_ROOTS) against
// other tests — the same pattern as power_user_worksession.rs.
#![allow(clippy::await_holding_lock)]

use serde_json::json;

/// Mutating the process environment is unsafe in Rust 2024 because it is not
/// thread-safe; this test serializes every environment access through
/// `test_env_lock`, so the precondition holds.
fn set_env(key: &str, value: &std::path::Path) {
    // SAFETY: the caller holds `test_env_lock` for the whole test, so no other
    // thread reads or writes the environment concurrently.
    unsafe { std::env::set_var(key, value) };
}

fn clear_env(key: &str) {
    // SAFETY: see `set_env`.
    unsafe { std::env::remove_var(key) };
}

/// Call a tool and return what came back, whichever channel it used. A write
/// gate answers either in the tool text (the atomic-write choke points return
/// `ERROR: …`) or as a JSON-RPC error (the early `ensure_writable` gates return
/// `invalid_params`). Folding both into one string keeps each assertion about
/// the refusal itself, not about the channel that carried it.
async fn call_outcome(
    engine: &lean_ctx::engine::ContextEngine,
    tool: &str,
    args: serde_json::Value,
) -> String {
    match engine.call_tool_text(tool, Some(args)).await {
        Ok(text) => text,
        Err(e) => format!("{e:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_root_is_readable_but_never_writable_via_engine() {
    let _lock = lean_ctx::core::data_dir::test_env_lock();

    let dir = tempfile::tempdir().expect("tempdir");
    let project = dir.path().join("project");
    let refrepo = dir.path().join("refrepo");
    std::fs::create_dir_all(project.join("src")).expect("project/src");
    std::fs::create_dir_all(&refrepo).expect("refrepo");
    // A project marker keeps the jail root stable (no auto-reroot heuristics).
    std::fs::write(
        project.join("Cargo.toml"),
        "[package]\nname = \"p\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
    )
    .expect("Cargo.toml");

    let proj_file = project.join("src/main.rs");
    std::fs::write(&proj_file, "fn main() { let x = 1; }\n").expect("main.rs");
    let proj_consts = project.join("src/consts.rs");
    std::fs::write(
        &proj_consts,
        "pub const A: i32 = 1;\npub const B: i32 = 1;\n",
    )
    .expect("consts.rs");

    let ref_line = "pub fn shared_secret() -> i32 { 42 }";
    let ref_file = refrepo.join("lib.rs");
    let ref_original = format!("{ref_line}\n");
    std::fs::write(&ref_file, &ref_original).expect("refrepo/lib.rs");
    let ref_path = ref_file.to_string_lossy().to_string();
    // The line's real anchor: the anchored ops pass the hash check and reach
    // the write, so what stops them below is the read-only tier, not a CONFLICT.
    let ref_hash = lean_ctx::core::anchor::line_hash(ref_line);
    let rewritten = "pub fn shared_secret() -> i32 { 999 }";
    // A file in a not-yet-existing directory, so a create that slipped through
    // would leave a directory behind as well as a file.
    let injected = refrepo.join("injected").join("injected.rs");

    let data_dir = dir.path().join("data");
    std::fs::create_dir_all(&data_dir).expect("data");
    set_env("LEAN_CTX_DATA_DIR", &data_dir);
    // The feature under test: the sibling repo is a read-only root.
    std::fs::write(
        data_dir.join("config.toml"),
        format!("read_only_roots = [{:?}]\n", refrepo.to_string_lossy()),
    )
    .expect("config.toml");

    let engine = lean_ctx::engine::ContextEngine::with_project_root(&project);

    // 1) READ of a file in the read-only root must resolve and return content
    //    (the whole point of read-only roots: read sibling repos).
    let read_out = engine
        .call_tool_text(
            "ctx_read",
            Some(json!({ "path": ref_file.to_string_lossy() })),
        )
        .await
        .expect("ctx_read call");

    // 2) Every live write route, aimed at the read-only root. Each call carries
    //    valid arguments for its route, so without the tier it would write.
    let attempts = [
        (
            "ctx_patch set_line (anchored)",
            "ctx_patch",
            json!({
                "path": ref_path,
                "op": "set_line",
                "line": 1,
                "hash": ref_hash,
                "new_text": rewritten,
            }),
        ),
        (
            "ctx_patch replace_unique",
            "ctx_patch",
            json!({
                "path": ref_path,
                "op": "replace_unique",
                "old_text": "42",
                "new_text": "999",
            }),
        ),
        (
            "ctx_patch replace_all",
            "ctx_patch",
            json!({
                "path": ref_path,
                "op": "replace_all",
                "find": "42",
                "replace": "999",
            }),
        ),
        (
            // A batch resolves each op's own `path` itself; the dispatch layer
            // only pre-resolves a top-level `path`, and there is none here.
            "ctx_patch ops[] batch",
            "ctx_patch",
            json!({
                "ops": [{
                    "op": "set_line",
                    "path": ref_path,
                    "line": 1,
                    "hash": ref_hash,
                    "new_text": rewritten,
                }],
            }),
        ),
        (
            "ctx_patch create",
            "ctx_patch",
            json!({
                "path": injected.to_string_lossy(),
                "op": "create",
                "new_text": "pub fn injected() {}\n",
            }),
        ),
        (
            "ctx_patch replace_symbol",
            "ctx_patch",
            json!({
                "path": ref_path,
                "op": "replace_symbol",
                "line": 1,
                "new_text": rewritten,
            }),
        ),
        (
            "ctx_refactor replace_symbol_body",
            "ctx_refactor",
            json!({
                "action": "replace_symbol_body",
                "path": ref_path,
                "line": 1,
                "new_text": rewritten,
            }),
        ),
    ];
    let mut refusals = Vec::with_capacity(attempts.len());
    for (label, tool, args) in attempts {
        refusals.push((label, call_outcome(&engine, tool, args).await));
    }

    // 3) CONTROL: the same routes still write inside the project root.
    let edit_ok = engine
        .call_tool_text(
            "ctx_patch",
            Some(json!({
                "path": proj_file.to_string_lossy(),
                "op": "replace_unique",
                "old_text": "let x = 1;",
                "new_text": "let x = 2;",
            })),
        )
        .await
        .expect("ctx_patch replace_unique (project) call");
    let replace_all_ok = engine
        .call_tool_text(
            "ctx_patch",
            Some(json!({
                "path": proj_consts.to_string_lossy(),
                "op": "replace_all",
                "find": "= 1;",
                "replace": "= 7;",
            })),
        )
        .await
        .expect("ctx_patch replace_all (project) call");

    // Snapshot disk state, then drop the env before asserting so a failure can
    // never leak the read-only root into a parallel test sharing the lock.
    let ref_after = std::fs::read_to_string(&ref_file).expect("read refrepo file");
    let mut ref_entries: Vec<String> = std::fs::read_dir(&refrepo)
        .expect("list refrepo")
        .map(|e| {
            e.expect("refrepo entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    ref_entries.sort();
    let proj_after = std::fs::read_to_string(&proj_file).expect("read project file");
    let consts_after = std::fs::read_to_string(&proj_consts).expect("read consts file");
    clear_env("LEAN_CTX_DATA_DIR");

    // Reads are allowed.
    assert!(
        read_out.contains("shared_secret"),
        "a read of a read-only root must return its content: {read_out}"
    );

    // Every write into the read-only root is denied and names the tier.
    for (label, out) in &refusals {
        assert!(
            out.contains("read-only root"),
            "{label} into a read-only root must be refused by the read-only tier: {out}"
        );
    }

    // The read-only tree is byte-identical — nothing leaked through any tool:
    // no rewrite, no created file or directory, no temp or backup file.
    assert_eq!(
        ref_after, ref_original,
        "the read-only file must be untouched"
    );
    assert_eq!(
        ref_entries,
        ["lib.rs"],
        "nothing may be created inside a read-only root"
    );

    // The project writes applied normally — the tier does not break writes.
    assert_eq!(
        proj_after, "fn main() { let x = 2; }\n",
        "a normal project edit must still apply: {edit_ok}"
    );
    assert_eq!(
        consts_after, "pub const A: i32 = 7;\npub const B: i32 = 7;\n",
        "a normal project replace_all must still apply: {replace_all_ok}"
    );
}
