//! The tool handlers in agent-like sequences: read cache and change
//! detection, error answers, `diff` without a cached version, search limits,
//! `sobre_tree` on odd inputs, and shell compression that keeps every
//! diagnostic.
#![allow(clippy::needless_raw_string_hashes)]

use sobre::core::cache::SessionCache;
use sobre::core::protocol::CrpMode;
use std::io::Write;

/// Writes `contents`, then sets the mtime at least 2 s past both the previous
/// mtime and now, so the read cache sees a modified file without the test
/// sleeping out the filesystem's timestamp granularity.
fn write_newer(path: &std::path::Path, contents: &str) {
    let before = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    std::fs::write(path, contents).unwrap();
    let now = std::time::SystemTime::now();
    let floor = before.map_or(now, |b| b.max(now));
    filetime::set_file_mtime(
        path,
        filetime::FileTime::from_system_time(floor + std::time::Duration::from_secs(2)),
    )
    .unwrap();
}

// =============================================================================
// sobre_read: a changed file is detected
// =============================================================================

mod read_cache_hits {
    use super::*;

    #[test]
    fn scenario_modified_file_detects_change() {
        let mut cache = SessionCache::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("app.rs");
        std::fs::write(&file, "fn v1() {}\n").unwrap();
        let path = file.to_str().unwrap();

        sobre::tools::sobre_read::handle_resolved(&mut cache, path, "full", CrpMode::Off);
        super::write_newer(&file, "fn v2() { updated(); }\n");

        let out = sobre::tools::sobre_read::handle_resolved(&mut cache, path, "full", CrpMode::Off);
        // Should detect the change and show new content or delta
        assert!(
            out.content.contains("v2")
                || out.content.contains("delta")
                || out.content.contains("diff"),
            "Expected change detection, got: {}",
            &out.content[..out.content.len().min(300)]
        );
    }
}

// =============================================================================
// sobre_read: a repeated compressed read gives the same output
// =============================================================================

mod compressed_read_cache {
    use super::*;

    #[test]
    fn scenario_map_mode_uses_compressed_cache() {
        let mut cache = SessionCache::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("module.rs");
        std::fs::write(
            &file,
            "use std::io;\npub fn read_all() -> io::Result<String> { Ok(String::new()) }\npub fn write_all(data: &str) -> io::Result<()> { Ok(()) }\n",
        ).unwrap();
        let path = file.to_str().unwrap();

        // First read with full mode to populate cache
        sobre::tools::sobre_read::handle_resolved(&mut cache, path, "full", CrpMode::Off);

        // First map read — processes and caches compressed output
        let out1 = sobre::tools::sobre_read::handle_resolved(&mut cache, path, "map", CrpMode::Off);
        assert!(!out1.content.is_empty());

        // Second map read of the unchanged file
        let out2 = sobre::tools::sobre_read::handle_resolved(&mut cache, path, "map", CrpMode::Off);
        assert!(!out2.content.is_empty());
        assert_eq!(out1.content, out2.content);
    }
}

// =============================================================================
// Errors: a missing file or directory is an error, not an empty success
// =============================================================================

mod error_data {
    use super::*;

    #[test]
    fn scenario_nonexistent_file_returns_error_mode() {
        let mut cache = SessionCache::new();
        let out = sobre::tools::sobre_read::handle_resolved(
            &mut cache,
            "/nonexistent/path/does_not_exist.rs",
            "full",
            CrpMode::Off,
        );
        assert_eq!(out.resolved_mode, "error");
        assert!(out.content.contains("ERROR"));
    }

    #[test]
    fn scenario_search_nonexistent_dir_returns_error() {
        let result = sobre::tools::sobre_search::handle_filtered(
            "pattern",
            "/nonexistent_dir_xyz",
            None,
            50,
            CrpMode::Off,
            true,
            false,
            None,
            None,
        )
        .text;
        assert!(
            result.starts_with("ERROR:"),
            "Expected ERROR prefix, got: {result}"
        );
    }

    #[test]
    fn scenario_tree_nonexistent_dir_returns_error() {
        let (result, _) = sobre::tools::sobre_tree::handle("/nonexistent_xyz", 3, false, true);
        assert!(
            result.starts_with("ERROR:"),
            "Expected ERROR prefix, got: {result}"
        );
    }

    #[test]
    fn scenario_tree_file_input_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("file.txt");
        std::fs::write(&file, "data").unwrap();

        let (result, _) = sobre::tools::sobre_tree::handle(file.to_str().unwrap(), 3, false, true);
        assert!(
            result.starts_with("ERROR:"),
            "Expected ERROR prefix, got: {result}"
        );
    }
}

// =============================================================================
// diff mode without a cached version answers with guidance, not the file
// =============================================================================

mod diff_guard {
    use super::*;

    #[test]
    fn scenario_diff_without_cache_returns_guidance() {
        let mut cache = SessionCache::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new_file.rs");
        std::fs::write(&file, "fn brand_new() {}\n").unwrap();
        let path = file.to_str().unwrap();

        let out = sobre::tools::sobre_read::handle_resolved(&mut cache, path, "diff", CrpMode::Off);
        assert!(
            out.content.contains("no cached version for diff")
                || out.content.contains("use mode=full first"),
            "Expected guidance message, got: {}",
            &out.content[..out.content.len().min(300)]
        );
        // Must NOT contain the file content
        assert!(
            !out.content.contains("brand_new"),
            "Full file content leaked through diff guard!"
        );
    }

    #[test]
    fn scenario_diff_after_full_read_works() {
        let mut cache = SessionCache::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tracked.rs");
        std::fs::write(&file, "fn version_one() {}\n").unwrap();
        let path = file.to_str().unwrap();

        // Read full first
        sobre::tools::sobre_read::handle_resolved(&mut cache, path, "full", CrpMode::Off);

        // Modify file
        super::write_newer(&file, "fn version_two() { changed(); }\n");

        // Now diff should work
        let out = sobre::tools::sobre_read::handle_resolved(&mut cache, path, "diff", CrpMode::Off);
        assert!(
            out.content.contains("diff") || out.content.contains("version_two"),
            "Expected diff content, got: {}",
            &out.content[..out.content.len().min(300)]
        );
    }
}

// =============================================================================
// sobre_search: max_results
// =============================================================================

mod search_max_results {
    use super::*;

    #[test]
    fn scenario_search_respects_max_results() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            let file = dir.path().join(format!("file_{i:02}.rs"));
            std::fs::write(&file, format!("fn search_target_{i}() {{}}\n")).unwrap();
        }

        let result = sobre::tools::sobre_search::handle_filtered(
            "search_target",
            dir.path().to_str().unwrap(),
            None,
            5,
            CrpMode::Off,
            false,
            false,
            None,
            None,
        )
        .text;
        let match_lines: Vec<&str> = result
            .lines()
            .filter(|l| l.contains("search_target"))
            .collect();
        assert!(
            match_lines.len() <= 5,
            "Expected at most 5 matches, got {}",
            match_lines.len()
        );
    }

    #[test]
    fn scenario_search_finds_all_when_under_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..3 {
            let file = dir.path().join(format!("src_{i}.rs"));
            std::fs::write(&file, format!("fn unique_pattern_{i}() {{}}\n")).unwrap();
        }

        let result = sobre::tools::sobre_search::handle_filtered(
            "unique_pattern",
            dir.path().to_str().unwrap(),
            None,
            50,
            CrpMode::Off,
            false,
            false,
            None,
            None,
        )
        .text;
        assert!(result.contains("3 matches"));
    }
}

// =============================================================================
// sobre_tree: a file, or an empty directory
// =============================================================================

mod tree_ux {
    #[test]
    fn scenario_file_input_suggests_parent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();

        let (result, _) = sobre::tools::sobre_tree::handle(file.to_str().unwrap(), 3, false, true);
        assert!(result.contains("is a file, not a directory"));
        assert!(result.contains("Use path="));
    }

    #[test]
    fn scenario_empty_directory_explicit_message() {
        let dir = tempfile::tempdir().unwrap();

        let (result, _) =
            sobre::tools::sobre_tree::handle(dir.path().to_str().unwrap(), 3, false, true);
        assert!(
            result.contains("empty directory"),
            "Expected 'empty directory' message, got: {result}"
        );
    }
}

// =============================================================================
// Shell compression keeps every diagnostic
// =============================================================================

mod compression_error_guard {
    use sobre::shell::compress::compress_if_beneficial_pub;

    #[test]
    fn scenario_cargo_check_error_fully_preserved() {
        let error_output = r#"   Compiling myapp v0.1.0 (/home/user/myapp)
error[E0308]: mismatched types
  --> src/main.rs:15:20
   |
15 |     let x: i32 = "hello";
   |            ---   ^^^^^^^ expected `i32`, found `&str`
   |            |
   |            expected due to this

error: aborting due to 1 previous error

For more information about this error, try `rustc --explain E0308`.
error: could not compile `myapp` (bin "myapp") due to 1 previous error
"#;
        let result = compress_if_beneficial_pub("cargo check", error_output);
        assert!(
            result.contains("src/main.rs:15:20"),
            "Error location was compressed away! Got: {result}"
        );
        assert!(result.contains("E0308"));
        assert!(result.contains("mismatched types"));
        assert!(result.contains("expected `i32`, found `&str`"));
    }

    #[test]
    fn scenario_cargo_clippy_warnings_fully_preserved() {
        let warning_output = r#"    Checking mylib v0.1.0
warning: unused variable: `x`
  --> src/lib.rs:42:9
   |
42 |     let x = compute_heavy();
   |         ^ help: if this is intentional, prefix it with an underscore: `_x`
   |
   = note: `#[warn(unused_variables)]` on by default

warning: `mylib` (lib) generated 1 warning
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.34s
"#;
        let result = compress_if_beneficial_pub("cargo clippy -- -D warnings", warning_output);
        assert!(
            result.contains("src/lib.rs:42:9"),
            "Warning location was compressed away! Got: {result}"
        );
        assert!(result.contains("unused variable: `x`"));
        assert!(result.contains("prefix it with an underscore"));
    }

    #[test]
    fn scenario_cargo_build_multiple_errors_preserved() {
        let error_output = r#"   Compiling myapp v0.1.0
error[E0502]: cannot borrow `*cache` as mutable because it is also borrowed as immutable
   --> src/tools/sobre_read.rs:320:9
    |
318 |     if let Some(existing) = cache.get(path) {
    |                             -------------- immutable borrow occurs here
320 |         cache.record_cache_hit(path);
    |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ mutable borrow occurs here

error[E0063]: missing field `last_flush` in initializer of `ContextLedger`
   --> src/core/context_ledger.rs:101:9
    |
101 |         Self {
    |         ^^^^ missing `last_flush`

error: could not compile `myapp` (lib) due to 2 previous errors
"#;
        let result = compress_if_beneficial_pub("cargo check 2>&1", error_output);
        assert!(
            result.contains("src/tools/sobre_read.rs:320:9"),
            "First error location lost! Got: {result}"
        );
        assert!(
            result.contains("src/core/context_ledger.rs:101:9"),
            "Second error location lost! Got: {result}"
        );
        assert!(result.contains("E0502"));
        assert!(result.contains("E0063"));
    }

    #[test]
    fn scenario_typescript_tsc_errors_preserved() {
        let tsc_output = r#"src/components/Button.tsx(23,5): error TS2322: Type 'string' is not assignable to type 'number'.
src/utils/api.ts(45,10): error TS2304: Cannot find name 'fetchData'.
Found 2 errors in 2 files.
"#;
        let result = compress_if_beneficial_pub("npx tsc --noEmit", tsc_output);
        assert!(
            result.contains("Button.tsx(23,5)"),
            "TSC error location lost! Got: {result}"
        );
        assert!(result.contains("api.ts(45,10)"));
        assert!(result.contains("TS2322"));
    }

    #[test]
    fn scenario_go_build_errors_preserved() {
        let go_output = r#"# myapp/internal/server
./server.go:15:2: undefined: handleRequest
./server.go:23:15: cannot use str (variable of type string) as int value in argument to process
"#;
        let result = compress_if_beneficial_pub("go build ./...", go_output);
        assert!(
            result.contains("server.go:15:2"),
            "Go error location lost! Got: {result}"
        );
        assert!(result.contains("undefined: handleRequest"));
    }

    #[test]
    fn scenario_eslint_errors_preserved() {
        let eslint_output = r#"/home/user/project/src/App.tsx
  15:7  error  'unused' is defined but never used  no-unused-vars
  23:1  error  Expected indentation of 2 spaces    indent

/home/user/project/src/utils.ts
  4:10  error  'x' is not defined  no-undef

✖ 3 problems (3 errors, 0 warnings)
"#;
        let result = compress_if_beneficial_pub("eslint src/", eslint_output);
        assert!(
            result.contains("15:7  error"),
            "ESLint error compressed away! Got: {result}"
        );
        assert!(result.contains("no-unused-vars"));
    }

    #[test]
    fn scenario_cargo_test_failures_preserved() {
        let test_output = r#"running 3 tests
test tests::test_add ... ok
test tests::test_subtract ... FAILED
test tests::test_multiply ... ok

failures:

---- tests::test_subtract stdout ----
thread 'tests::test_subtract' panicked at src/math.rs:25:5:
assertion `left == right` failed
  left: 5
 right: 3
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    tests::test_subtract

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
"#;
        let result = compress_if_beneficial_pub("cargo test", test_output);
        assert!(
            result.contains("src/math.rs:25:5"),
            "Test failure location lost! Got: {result}"
        );
        assert!(result.contains("panicked at"));
        assert!(result.contains("left: 5"));
    }

    #[test]
    fn scenario_mypy_errors_preserved() {
        let mypy_output = r#"src/main.py:10: error: Incompatible return value type (got "str", expected "int")  [return-value]
src/utils.py:25: error: Name "undefined_var" is not defined  [name-defined]
Found 2 errors in 2 files (checked 5 source files)
"#;
        let result = compress_if_beneficial_pub("mypy src/", mypy_output);
        assert!(
            result.contains("src/main.py:10"),
            "mypy error location lost! Got: {result}"
        );
        assert!(result.contains("Incompatible return value type"));
    }

    #[test]
    fn scenario_ruff_errors_preserved() {
        let ruff_output = r#"src/app.py:5:1: F401 [*] `os` imported but unused
src/utils.py:12:5: E741 Ambiguous variable name: `l`
Found 2 errors.
[*] 1 fixable with the `--fix` option.
"#;
        let result = compress_if_beneficial_pub("ruff check src/", ruff_output);
        assert!(
            result.contains("src/app.py:5:1"),
            "Ruff error location lost! Got: {result}"
        );
        assert!(result.contains("F401"));
    }

    #[test]
    fn scenario_dotnet_build_errors_preserved() {
        let dotnet_output = r#"Build started...
Build FAILED.

Program.cs(15,13): error CS1002: ; expected [/home/user/MyApp/MyApp.csproj]
Program.cs(23,5): error CS0103: The name 'undeclared' does not exist in the current context [/home/user/MyApp/MyApp.csproj]

    2 Error(s)
"#;
        let result = compress_if_beneficial_pub("dotnet build", dotnet_output);
        assert!(
            result.contains("Program.cs(15,13)"),
            "dotnet error location lost! Got: {result}"
        );
        assert!(result.contains("CS1002"));
    }

    #[test]
    fn scenario_successful_build_keeps_its_outcome() {
        let success_output = "   Compiling serde v1.0.193\n   Compiling tokio v1.35.0\n   Compiling myapp v0.1.0\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 45.23s\n";
        let result = compress_if_beneficial_pub("cargo build", success_output);
        assert!(
            result.contains("Finished `dev` profile"),
            "the build outcome must survive: {result}"
        );
    }

    #[test]
    fn scenario_make_errors_preserved() {
        let make_output = r#"gcc -Wall -o main main.c utils.c
main.c:23:5: error: implicit declaration of function 'undefined_func'
main.c:30:15: error: expected ';' before '}' token
make: *** [Makefile:12: main] Error 1
"#;
        let result = compress_if_beneficial_pub("make build", make_output);
        assert!(
            result.contains("main.c:23:5"),
            "Make/gcc error location lost! Got: {result}"
        );
        assert!(result.contains("implicit declaration"));
    }

    #[test]
    fn scenario_gradle_errors_preserved() {
        let gradle_output = r#"> Task :compileJava FAILED
/home/user/src/main/java/App.java:15: error: cannot find symbol
    UndefinedClass obj = new UndefinedClass();
    ^
  symbol:   class UndefinedClass
  location: class App

BUILD FAILED in 3s
"#;
        let result = compress_if_beneficial_pub("./gradlew build", gradle_output);
        assert!(
            result.contains("App.java:15"),
            "Gradle error location lost! Got: {result}"
        );
        assert!(result.contains("cannot find symbol"));
    }
}

// =============================================================================
// Integration: Full agent workflow
// =============================================================================

mod integration_workflow {
    use super::*;

    #[test]
    fn scenario_edit_save_reread_cycle() {
        let mut cache = SessionCache::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("workflow.rs");
        std::fs::write(&file, "fn original() {}\n").unwrap();
        let path = file.to_str().unwrap();

        // 1. Agent reads file for the first time
        let out1 =
            sobre::tools::sobre_read::handle_resolved(&mut cache, path, "full", CrpMode::Off);
        assert!(out1.content.contains("original"));
        assert_ne!(out1.resolved_mode, "error");

        // 2. Agent reads same file again (cache hit, no disk I/O)
        let out2 =
            sobre::tools::sobre_read::handle_resolved(&mut cache, path, "full", CrpMode::Off);
        assert!(out2.content.contains("unchanged") || out2.content.contains("cached"));

        // 3. File is edited externally
        super::write_newer(&file, "fn edited_version() { new_logic(); }\n");

        // 4. Agent reads again — detects change
        let out3 =
            sobre::tools::sobre_read::handle_resolved(&mut cache, path, "full", CrpMode::Off);
        assert!(
            out3.content.contains("edited_version")
                || out3.content.contains("delta")
                || out3.content.contains("diff"),
            "Should detect file change, got: {}",
            &out3.content[..out3.content.len().min(300)]
        );
    }

    #[test]
    fn scenario_search_in_project_with_many_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();

        for i in 0..10 {
            let mut f = std::fs::File::create(src.join(format!("mod_{i:02}.rs"))).unwrap();
            writeln!(f, "pub fn handler_{i}() {{}}").unwrap();
            writeln!(f, "pub fn helper_{i}() {{}}").unwrap();
        }

        let result = sobre::tools::sobre_search::handle_filtered(
            "handler_",
            dir.path().to_str().unwrap(),
            Some("*.rs"),
            50,
            CrpMode::Off,
            false,
            false,
            None,
            None,
        )
        .text;
        assert!(result.contains("handler_"));
        assert!(
            result.contains("10 matches"),
            "Expected 10 matches, got: {}",
            &result[..result.len().min(200)]
        );
    }

    #[test]
    fn scenario_tree_of_project_structure() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let tests = dir.path().join("tests");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&tests).unwrap();
        std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(src.join("lib.rs"), "pub mod utils;").unwrap();
        std::fs::write(tests.join("integration.rs"), "#[test] fn it_works() {}").unwrap();

        let (result, _) =
            sobre::tools::sobre_tree::handle(dir.path().to_str().unwrap(), 3, false, true);
        assert!(result.contains("src"));
        assert!(result.contains("tests"));
        assert!(!result.starts_with("ERROR:"));
    }

    #[test]
    fn scenario_multimode_read_workflow() {
        let mut cache = SessionCache::new();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("multi.rs");
        std::fs::write(
            &file,
            "use std::io;\n\npub struct Config {\n    pub name: String,\n    pub port: u16,\n}\n\nimpl Config {\n    pub fn new() -> Self {\n        Self { name: \"app\".into(), port: 8080 }\n    }\n}\n",
        ).unwrap();
        let path = file.to_str().unwrap();

        // 1. Read with full mode
        let full =
            sobre::tools::sobre_read::handle_resolved(&mut cache, path, "full", CrpMode::Off);
        assert!(full.content.contains("Config"));

        // 2. Read with signatures mode (should use compressed cache on 2nd call)
        let sig1 =
            sobre::tools::sobre_read::handle_resolved(&mut cache, path, "signatures", CrpMode::Off);
        assert!(!sig1.content.is_empty());

        let sig2 =
            sobre::tools::sobre_read::handle_resolved(&mut cache, path, "signatures", CrpMode::Off);
        // Should be identical (compressed cache hit)
        assert_eq!(sig1.content, sig2.content);

        // 3. Read with map mode
        let map1 = sobre::tools::sobre_read::handle_resolved(&mut cache, path, "map", CrpMode::Off);
        assert!(!map1.content.is_empty());
    }
}
