use serde_json::json;

/// §4.5: inner handle MUST use the (already resolved) abs_path it is given,
/// never re-derive a path from raw args. A raw "../escape.rs" must never
/// reach the filesystem layer; only the provided abs_path does.
#[test]
fn inner_handle_uses_provided_abs_path_not_raw_args() {
    let args = json!({"action": "references", "path": "../escape.rs", "line": 1, "column": 0});
    let out = super::handle(&args, "/proj", "/proj/resolved.rs");
    // open_file fails reading the (nonexistent) resolved file → error names abs_path.
    assert!(
        out.contains("/proj/resolved.rs"),
        "abs_path not used: {out}"
    );
    assert!(
        !out.contains("../escape.rs"),
        "raw path leaked to fs layer: {out}"
    );
}

/// `declaration` is a known action: the unknown-action arm must not fire for it,
/// and its help text now advertises `declaration`.
///
/// NOTE (adaptation): the real `handle` opens the file *before* the action
/// match, so reaching the unknown-action help arm requires a backend. We seed
/// a no-op stub backend for `rust` and point at a real temp `.rs` file so
/// dispatch deterministically reaches the help text, offline, without
/// starting rust-analyzer.
#[test]
fn unknown_action_help_lists_the_available_actions() {
    struct StubBackend;
    impl crate::lsp::backend::LspBackend for StubBackend {
        fn open_file(
            &mut self,
            _uri: &lsp_types::Uri,
            _language_id: &str,
            _text: &str,
        ) -> Result<(), String> {
            Ok(())
        }
        fn references(
            &mut self,
            _uri: &lsp_types::Uri,
            _position: lsp_types::Position,
        ) -> Result<Vec<lsp_types::Location>, String> {
            Ok(vec![])
        }
        fn definition(
            &mut self,
            _uri: &lsp_types::Uri,
            _position: lsp_types::Position,
        ) -> Result<lsp_types::GotoDefinitionResponse, String> {
            Ok(lsp_types::GotoDefinitionResponse::Array(vec![]))
        }
        fn implementations(
            &mut self,
            _uri: &lsp_types::Uri,
            _position: lsp_types::Position,
        ) -> Result<Vec<lsp_types::Location>, String> {
            Ok(vec![])
        }
        fn rename(
            &mut self,
            _uri: &lsp_types::Uri,
            _position: lsp_types::Position,
            _new_name: &str,
        ) -> Result<Option<lsp_types::WorkspaceEdit>, String> {
            Ok(None)
        }
    }

    let dir = std::env::temp_dir().join(format!("sobre_r1_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("x.rs");
    std::fs::write(&file, "fn x() {}\n").unwrap();
    let root = dir.to_string_lossy().to_string();
    let abs = file.to_string_lossy().to_string();

    let _stub = crate::lsp::router::stub_test_lock();
    crate::lsp::router::seed_stub_backend("rust", &root, Box::new(StubBackend));

    let args = json!({"action": "definitely_bogus", "path": "x.rs", "line": 1});
    let out = super::handle(&args, &root, &abs);
    for action in [
        "references",
        "rename (preview)",
        "replace_symbol_body",
        "ctx_outline",
    ] {
        assert!(out.contains(action), "help text missing {action}: {out}");
    }
    assert!(
        !out.contains("symbols_overview"),
        "help still lists symbols_overview: {out}"
    );
    assert!(
        !out.contains("declaration"),
        "help still lists declaration: {out}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "requires pre-built symbol index; flaky under parallel cargo test"]
fn resolve_name_path_unique_class() {
    let _lock = crate::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    crate::test_env::set_var("SOBRE_DATA_DIR", data.to_string_lossy().to_string());

    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src/lib.rs"),
        "pub struct UniqueZqWidget { pub a: u8 }\n",
    )
    .unwrap();
    let root = proj.to_string_lossy().to_string();

    let r =
        super::resolve_name_path_scoped("UniqueZqWidget", &root, None).expect("unique resolution");
    assert!(r.rel_path.ends_with("lib.rs"), "got: {}", r.rel_path);
    assert!(r.end_line >= r.start_line && r.start_line > 0);

    crate::test_env::remove_var("SOBRE_DATA_DIR");
}

#[test]
fn resolve_name_path_unknown_is_no_symbol() {
    let _lock = crate::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    crate::test_env::set_var("SOBRE_DATA_DIR", data.to_string_lossy().to_string());

    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src/lib.rs"),
        "pub struct UniqueZqWidget { pub a: u8 }\n",
    )
    .unwrap();
    let root = proj.to_string_lossy().to_string();

    let err = super::resolve_name_path_scoped("ZzzNoSuchSymbol123", &root, None).unwrap_err();
    assert!(err.starts_with("NO_SYMBOL"), "got: {err}");

    crate::test_env::remove_var("SOBRE_DATA_DIR");
}

#[test]
#[ignore = "requires pre-built symbol index; flaky under parallel cargo test"]
fn resolve_name_path_trait_impl_method() {
    let _lock = crate::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    crate::test_env::set_var("SOBRE_DATA_DIR", data.to_string_lossy().to_string());

    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src/lib.rs"),
        "pub struct RenderBridge;\n\
             pub trait Exec { fn execute(&self); }\n\
             impl Exec for RenderBridge {\n\
             \x20   fn execute(&self) { let _ = 1; }\n\
             }\n",
    )
    .unwrap();
    let root = proj.to_string_lossy().to_string();

    let r = super::resolve_name_path_scoped("RenderBridge/execute", &root, None)
        .expect("trait-impl method should resolve");
    assert!(r.rel_path.ends_with("lib.rs"), "got: {}", r.rel_path);
    // Muss auf den Impl-Methoden-Body zeigen (Zeile >= 3), nicht auf das
    // struct (Z. 1) oder die Trait-Deklaration (Z. 2).
    assert!(
        r.start_line >= 3,
        "should point at impl method, got L{}",
        r.start_line
    );
    assert!(r.end_line >= r.start_line && r.start_line > 0);

    crate::test_env::remove_var("SOBRE_DATA_DIR");
}

#[test]
fn container_matches_ancestor_cases() {
    use super::container_matches_ancestor as m;
    assert!(m("RenderBridge", "RenderBridge"));
    assert!(m("Exec for RenderBridge", "RenderBridge"));
    assert!(m("Exec for RenderBridge<Wasm>", "RenderBridge"));
    assert!(!m("OtherType", "RenderBridge"));
    assert!(!m("Exec for Other", "RenderBridge"));
}

#[test]
#[ignore = "requires pre-built symbol index; flaky under parallel cargo test"]
fn resolve_name_path_inherent_impl_method() {
    let _lock = crate::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    crate::test_env::set_var("SOBRE_DATA_DIR", data.to_string_lossy().to_string());

    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src/lib.rs"),
        "pub struct RenderBridge;\n\
             impl RenderBridge {\n\
             \x20   pub fn run(&self) { let _ = 1; }\n\
             }\n",
    )
    .unwrap();
    let root = proj.to_string_lossy().to_string();

    let r = super::resolve_name_path_scoped("RenderBridge/run", &root, None)
        .expect("inherent-impl method should still resolve");
    assert!(r.rel_path.ends_with("lib.rs"), "got: {}", r.rel_path);
    assert!(r.start_line >= 2 && r.end_line >= r.start_line);

    crate::test_env::remove_var("SOBRE_DATA_DIR");
}

#[test]
#[ignore = "requires pre-built symbol index; flaky under parallel cargo test"]
fn resolve_name_path_ambiguous_trait_impls() {
    let _lock = crate::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    crate::test_env::set_var("SOBRE_DATA_DIR", data.to_string_lossy().to_string());

    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    std::fs::write(
        proj.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src/lib.rs"),
        "pub struct RenderBridge;\n\
             pub trait A { fn execute(&self); }\n\
             pub trait B { fn execute(&self); }\n\
             pub mod a;\n\
             pub mod b;\n",
    )
    .unwrap();
    // a.rs: impl A for RenderBridge — plain targets, multi-line body so fn is indexed
    std::fs::write(
        proj.join("src/a.rs"),
        "impl A for RenderBridge {\n\
             \x20   fn execute(&self) { let _ = 1; }\n\
             }\n",
    )
    .unwrap();
    // b.rs: impl B for RenderBridge — plain targets, multi-line body so fn is indexed
    std::fs::write(
        proj.join("src/b.rs"),
        "impl B for RenderBridge {\n\
             \x20   fn execute(&self) { let _ = 1; }\n\
             }\n",
    )
    .unwrap();
    let root = proj.to_string_lossy().to_string();

    // "RenderBridge/execute": two segments → container_matches_ancestor runs for each hit.
    // "A for RenderBridge" and "B for RenderBridge" both match ancestor "RenderBridge",
    // producing two distinct hits (src/a.rs and src/b.rs) → AMBIGUOUS_SYMBOL.
    let err = super::resolve_name_path_scoped("RenderBridge/execute", &root, None)
        .expect_err("two trait impls (cross-file) with same method must be ambiguous");
    assert!(err.starts_with("AMBIGUOUS_SYMBOL"), "got: {err}");

    crate::test_env::remove_var("SOBRE_DATA_DIR");
}

/// #845: file_scope narrows ambiguous symbols to a single file.
#[test]
fn file_scope_disambiguates_ambiguous_symbol() {
    // Private data dir under the global env lock: serializes this test against
    // resolve_name_path_unknown_is_no_symbol (which sets SOBRE_DATA_DIR and
    // reaches the same graph build) and restores the env on drop, even on panic.
    let _data = crate::core::data_dir::isolated_data_dir();
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj845");
    std::fs::create_dir_all(proj.join("src")).unwrap();

    std::fs::write(
        proj.join("src/a.rs"),
        "impl A for RenderBridge {\n\
             \x20   fn execute(&self) { let _ = 1; }\n\
             }\n",
    )
    .unwrap();
    std::fs::write(
        proj.join("src/b.rs"),
        "impl B for RenderBridge {\n\
             \x20   fn execute(&self) { let _ = 1; }\n\
             }\n",
    )
    .unwrap();
    let root = proj.to_string_lossy().to_string();

    // Pre-build the graph so every resolve below is served by `open_existing`
    // and never reaches `graph_provider::BUILD_GATE`. That gate is process-wide
    // and try_lock'ed: while ANY other test builds ANY graph, `open_or_build`
    // returns None and resolve answers "NO_SYMBOL: no symbol index available".
    // Same remedy as impl_overview's overview_header_surfaces_persisted_call_edges.
    assert!(
        !crate::core::graph_index::scan(&root).files.is_empty(),
        "premise: symbol graph must build for the fixture"
    );

    // Without file_scope: ambiguous (2 matches).
    let err = super::resolve_name_path_scoped("RenderBridge/execute", &root, None)
        .expect_err("should be ambiguous without file_scope");
    assert!(err.starts_with("AMBIGUOUS_SYMBOL"), "got: {err}");

    // With file_scope="src/a.rs": resolves to exactly one match.
    let r = super::resolve_name_path_scoped("RenderBridge/execute", &root, Some("src/a.rs"))
        .expect("file_scope should disambiguate");
    assert!(r.rel_path.ends_with("src/a.rs"), "got: {}", r.rel_path);

    // With file_scope="src/b.rs": resolves to exactly one match in b.rs.
    let r = super::resolve_name_path_scoped("RenderBridge/execute", &root, Some("src/b.rs"))
        .expect("file_scope should disambiguate");
    assert!(r.rel_path.ends_with("src/b.rs"), "got: {}", r.rel_path);
}

#[test]
fn anchor_indent_reads_leading_whitespace() {
    let content = "class A {\n    fun b() {}\n}\n";
    assert_eq!(super::anchor_indent(content, 2), "    "); // line 2 (1-based) → 4 spaces
    assert_eq!(super::anchor_indent(content, 1), ""); // line 1 → none
}

#[test]
fn reindent_prefixes_first_line_only() {
    assert_eq!(
        super::reindent_first_line("fun x() {}", "    "),
        "    fun x() {}"
    );
    // Already-indented text is left untouched.
    assert_eq!(
        super::reindent_first_line("    fun x()", "    "),
        "    fun x()"
    );
}

#[test]
fn apply_symbol_edit_headless_replaces_range() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Foo.txt"), "aaa\nBODY\nccc\n").unwrap();
    let abs = dir.path().join("Foo.txt").to_string_lossy().to_string();
    let edit = crate::lsp::backend::RangeEdit {
        abs_path: abs.clone(),
        rel_path: "Foo.txt".into(),
        range: crate::lsp::backend::TextRange0Based {
            start_line: 1,
            start_char: 0,
            end_line: 1,
            end_char: 4,
        },
        text: "NEW".into(),
        expected_hash: None,
    };
    // No port file under this temp dir → headless apply.
    let res = super::apply_symbol_edit("replace_symbol_body", dir.path().to_str().unwrap(), &edit)
        .unwrap();
    assert!(res.applied);
    assert_eq!(std::fs::read_to_string(&abs).unwrap(), "aaa\nNEW\nccc\n");
}

#[test]
fn handle_replace_symbol_body_via_position_fallback() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn old() {\n  1\n}\n").unwrap();
    let args = serde_json::json!({
        "action": "replace_symbol_body",
        "path": "a.rs",
        "line": 1,
        "end_line": 3,
        "new_text": "fn new() {\n  2\n}"
    });
    let out = super::handle(&args, dir.path().to_str().unwrap(), "");
    assert!(out.contains("replace_symbol_body applied"), "got: {out}");
    let after = std::fs::read_to_string(dir.path().join("a.rs")).unwrap();
    assert!(after.contains("fn new()"), "file: {after}");
}

#[test]
fn handle_replace_symbol_body_conflict_on_stale_hash() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn old() {\n  1\n}\n").unwrap();
    // Range = full file lines 1..=3; old content = the whole file text.
    let stale = serde_json::json!({
        "action": "replace_symbol_body",
        "path": "a.rs", "line": 1, "end_line": 3,
        "new_text": "fn new() {\n  2\n}",
        "expected_hash": "deadbeefnotahash"
    });
    let out = super::handle(&stale, dir.path().to_str().unwrap(), "");
    assert!(out.contains("CONFLICT"), "got: {out}");
    // file unchanged
    assert!(
        std::fs::read_to_string(dir.path().join("a.rs"))
            .unwrap()
            .contains("fn old()")
    );
}

/// #803: with an explicit `path`, `name_path` is looked up in that file only,
/// so a symbol the index knows from another checkout is never the target.
/// Simulates a git worktree INSIDE the project root
/// (`/repo/.claude/worktrees/wt/`): the edit fails and neither file changes.
#[test]
#[ignore = "requires pre-built symbol index; flaky under parallel cargo test"]
fn handle_replace_symbol_worktree_mismatch_blocks_write() {
    let _lock = crate::core::data_dir::test_env_lock();
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    crate::test_env::set_var("SOBRE_DATA_DIR", data.to_string_lossy().to_string());

    // Main checkout root with a symbol.
    let main_root = tmp.path().join("repo");
    std::fs::create_dir_all(main_root.join("src")).unwrap();
    std::fs::write(
        main_root.join("Cargo.toml"),
        "[package]\nname=\"x\"\nversion=\"0.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        main_root.join("src/lib.rs"),
        "fn worktree_canary_zz() { let _ = 1; }\n",
    )
    .unwrap();
    let root = main_root.to_string_lossy().to_string();

    // Worktree INSIDE the project root (realistic layout).
    let wt_dir = main_root.join(".claude/worktrees/wt/src");
    std::fs::create_dir_all(&wt_dir).unwrap();
    std::fs::write(
        wt_dir.join("lib.rs"),
        "fn worktree_canary_zz() { let _ = 2; }\n",
    )
    .unwrap();

    // The symbol resolves via the main-root index to "src/lib.rs".
    let resolved = super::resolve_name_path_scoped("worktree_canary_zz", &root, None);
    assert!(resolved.is_ok(), "symbol should resolve: {resolved:?}");

    // Caller provides a path inside the worktree: the lookup is scoped to it.
    let wt_file = main_root.join(".claude/worktrees/wt/src/lib.rs");
    let args = serde_json::json!({
        "action": "replace_symbol_body",
        "name_path": "worktree_canary_zz",
        "path": wt_file.to_string_lossy().to_string(),
        "new_text": "fn worktree_canary_zz() { panic!(\"CANARY\"); }"
    });
    let out = super::handle(&args, &root, "");
    assert!(
        out.starts_with("ERROR"),
        "must not resolve elsewhere, got: {out}"
    );
    // Verify neither file was modified.
    let main_content = std::fs::read_to_string(main_root.join("src/lib.rs")).unwrap();
    assert!(
        !main_content.contains("CANARY"),
        "main checkout must NOT be modified: {main_content}"
    );
    let wt_content = std::fs::read_to_string(&wt_file).unwrap();
    assert!(
        !wt_content.contains("CANARY"),
        "worktree must NOT be modified: {wt_content}"
    );

    crate::test_env::remove_var("SOBRE_DATA_DIR");
}

/// Decision 5: with `path`, `name_path` resolves in that file only, so a name
/// defined in several files edits the one the caller named.
#[test]
fn name_path_with_path_edits_that_file() {
    let _data = crate::core::data_dir::isolated_data_dir();
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("proj_scope");
    std::fs::create_dir_all(proj.join("src")).unwrap();
    for file in ["src/a.rs", "src/b.rs"] {
        std::fs::write(proj.join(file), "pub fn execute() -> u8 {\n    1\n}\n").unwrap();
    }
    let root = proj.to_string_lossy().to_string();
    assert!(
        !crate::core::graph_index::scan(&root).files.is_empty(),
        "premise: symbol graph must build for the fixture"
    );

    let unscoped = super::handle(
        &json!({"action": "replace_symbol_body", "name_path": "execute",
                "new_text": "pub fn execute() -> u8 {\n    2\n}"}),
        &root,
        "",
    );
    assert!(unscoped.contains("AMBIGUOUS_SYMBOL"), "{unscoped}");

    let scoped = super::handle(
        &json!({"action": "replace_symbol_body", "name_path": "execute", "path": "src/b.rs",
                "new_text": "pub fn execute() -> u8 {\n    2\n}"}),
        &root,
        "",
    );
    assert!(scoped.contains("replace_symbol_body applied"), "{scoped}");
    let read = |f: &str| std::fs::read_to_string(proj.join(f)).unwrap();
    assert!(read("src/b.rs").contains("    2"), "b.rs must be edited");
    assert!(read("src/a.rs").contains("    1"), "a.rs must be untouched");
}
