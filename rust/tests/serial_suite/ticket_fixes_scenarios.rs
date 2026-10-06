// Integration tests for all ticket fixes:
// - #271: Crash hardening (catch_unwind, unwrap elimination)
// - #288: Crash loop backoff resilience
// - #289: Daemon autostart lifecycle
// - Proxy opt-in (bootstrap/repair respect config)
// - Loop detector error-awareness
// - JSONC UTF-8 safety
// - ls -lah human-readable size passthrough
// - PowerShell -NoProfile
// - Uninstall: .bak cleanup, XDG dirs, project-local files
// - Windows drive-colon grep parsing
// - Shell-tokenizer for hook rewrites

// ──────────────────────────────────────────────────────────
// Scenario Group 2: Zed auto-detect (#247)
// ──────────────────────────────────────────────────────────

#[test]
fn zed_target_exists_with_correct_paths() {
    let home = dirs::home_dir().unwrap_or_default();
    let targets = lean_ctx::core::editor_registry::detect::build_targets(&home);
    let zed = targets
        .iter()
        .find(|t| t.name == "Zed")
        .expect("Zed target must exist");

    assert_eq!(zed.agent_key, "zed");
    let config_str = zed.config_path.to_string_lossy();
    assert!(
        config_str.contains("settings.json"),
        "Zed config should reference settings.json: {config_str}"
    );
}

// ──────────────────────────────────────────────────────────
// Scenario Group 3: Crash loop backoff resilience (#288)
// User: IDE restarts several times quickly on slow machine,
// lean-ctx should not block for too long.
// ──────────────────────────────────────────────────────────

#[test]
fn crash_loop_constants_are_resilient_for_slow_ides() {
    let threshold = lean_ctx::core::startup_guard::CRASH_LOOP_THRESHOLD;
    let window = lean_ctx::core::startup_guard::CRASH_LOOP_WINDOW_SECS;
    let backoff = lean_ctx::core::startup_guard::CRASH_LOOP_MAX_BACKOFF_SECS;
    assert!(
        threshold >= 8,
        "Threshold too low: slow IDEs may restart MCP 5-7 times normally"
    );
    assert!(
        window >= 60,
        "Window too short: Zed/Windsurf startup can take 30-60s"
    );
    assert!(
        backoff <= 30,
        "Max backoff too aggressive: users will think lean-ctx is broken"
    );
}

#[test]
fn crash_loop_scenario_normal_ide_startup() {
    let _env = lean_ctx::core::data_dir::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("LEAN_CTX_DATA_DIR", dir.path()) };

    let start = std::time::Instant::now();
    for _ in 0..7 {
        lean_ctx::core::startup_guard::crash_loop_backoff("scenario-normal");
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "7 starts within window should NOT trigger backoff, took {elapsed:?}"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::remove_var("LEAN_CTX_DATA_DIR") };
}

#[test]
fn crash_loop_reset_allows_fresh_start() {
    let _env = lean_ctx::core::data_dir::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("LEAN_CTX_DATA_DIR", dir.path()) };

    for _ in 0..7 {
        lean_ctx::core::startup_guard::crash_loop_backoff("scenario-reset");
    }

    lean_ctx::core::startup_guard::reset_crash_loop("scenario-reset");

    let log_path = dir.path().join(".scenario-reset-starts.log");
    assert!(!log_path.exists(), "reset should remove the crash log file");

    let start = std::time::Instant::now();
    lean_ctx::core::startup_guard::crash_loop_backoff("scenario-reset");
    assert!(
        start.elapsed() < std::time::Duration::from_millis(100),
        "after reset, first call should be instant"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::remove_var("LEAN_CTX_DATA_DIR") };
}

// ──────────────────────────────────────────────────────────
// Scenario Group 5: Loop detector error-awareness
// User: agent calls ctx_read on a file that fails.
// The failed call should NOT count toward the loop limit.
// ──────────────────────────────────────────────────────────

#[test]
fn loop_detector_scenario_file_not_found_then_retry() {
    let mut detector = lean_ctx::core::loop_detection::LoopDetector::new();

    let r1 = detector.record_call("ctx_read", "nonexistent_file");
    assert_eq!(r1.call_count, 1);

    detector.record_error_outcome("ctx_read", "nonexistent_file");

    let r2 = detector.record_call("ctx_read", "nonexistent_file");
    assert_eq!(
        r2.call_count, 1,
        "after error undo, effective count should be 1, not 2"
    );
    assert_eq!(
        r2.level,
        lean_ctx::core::loop_detection::ThrottleLevel::Normal,
        "should not be throttled after error"
    );
}

#[test]
fn loop_detector_scenario_repeated_permission_errors() {
    let mut detector = lean_ctx::core::loop_detection::LoopDetector::new();

    for _ in 0..10 {
        detector.record_call("ctx_read", "restricted_file");
        detector.record_error_outcome("ctx_read", "restricted_file");
    }

    let r = detector.record_call("ctx_read", "restricted_file");
    assert_eq!(
        r.call_count, 1,
        "all failed attempts undone, effective count = 1"
    );
}

#[test]
fn loop_detector_scenario_mixed_success_and_failure() {
    let mut detector = lean_ctx::core::loop_detection::LoopDetector::new();

    detector.record_call("ctx_read", "file_a");
    detector.record_call("ctx_read", "file_a");
    detector.record_error_outcome("ctx_read", "file_a");

    let r = detector.record_call("ctx_read", "file_a");
    assert_eq!(
        r.call_count, 2,
        "2 calls, 1 error undo, next call should see count=2"
    );
}

#[test]
fn loop_detector_scenario_error_on_different_tool_no_crosstalk() {
    let mut detector = lean_ctx::core::loop_detection::LoopDetector::new();

    detector.record_call("ctx_shell", "cmd1");
    detector.record_error_outcome("ctx_shell", "cmd1");

    let r = detector.record_call("ctx_read", "file1");
    assert_eq!(
        r.call_count, 1,
        "error on ctx_shell should not affect ctx_read count"
    );
}

// ──────────────────────────────────────────────────────────
// Scenario Group 6: JSONC UTF-8 safety
// User: has JSONC config with Cyrillic comments, CJK text,
// or emoji in file paths.
// ──────────────────────────────────────────────────────────

#[test]
fn jsonc_cyrillic_comments_preserved() {
    let input = r#"{
  // Привет мир — настройки
  "key": "значение"
}"#;
    let v = lean_ctx::core::jsonc::parse_jsonc(input).unwrap();
    assert_eq!(v["key"], "значение");
}

#[test]
fn jsonc_cjk_values_preserved() {
    let input = r#"{
  "name": "テストプロジェクト",
  /* 日本語のコメント */
  "path": "文档/项目"
}"#;
    let v = lean_ctx::core::jsonc::parse_jsonc(input).unwrap();
    assert_eq!(v["name"], "テストプロジェクト");
    assert_eq!(v["path"], "文档/项目");
}

#[test]
fn jsonc_emoji_in_values_and_comments() {
    let input = r#"{
  // 🚀 config
  "emoji_key": "value 🎉",
  "path": "/home/user/📦project"
}"#;
    let v = lean_ctx::core::jsonc::parse_jsonc(input).unwrap();
    assert_eq!(v["emoji_key"], "value 🎉");
    assert_eq!(v["path"], "/home/user/📦project");
}

#[test]
fn jsonc_mixed_multibyte_stress() {
    let input = r#"{
  // Cyrillic: план → реализация
  // CJK: 設定 → テスト
  // Emoji: 🔧→⚡
  "ru": "ошибка",
  "ja": "設定ファイル",
  "emoji": "🔥🚀✨",
  "url": "https://example.com/документация"
}"#;
    let v = lean_ctx::core::jsonc::parse_jsonc(input).unwrap();
    assert_eq!(v["ru"], "ошибка");
    assert_eq!(v["ja"], "設定ファイル");
    assert_eq!(v["emoji"], "🔥🚀✨");
    assert_eq!(v["url"], "https://example.com/документация");
}

#[test]
fn jsonc_block_comment_with_multibyte() {
    let input = r#"{
  /* Многострочный
     комментарий с
     кириллицей */
  "status": "ок"
}"#;
    let v = lean_ctx::core::jsonc::parse_jsonc(input).unwrap();
    assert_eq!(v["status"], "ок");
}

// ──────────────────────────────────────────────────────────
// Scenario Group 7: ls -lah human-readable size passthrough
// User: runs `ls -lah` and lean-ctx compresses the output.
// Human-readable sizes (4.0K, 1.2M) should pass through,
// not be re-formatted as "0B".
// ──────────────────────────────────────────────────────────

#[test]
fn ls_lah_human_readable_sizes_passthrough() {
    let output = "total 32K\n\
        drwxr-xr-x  5 user staff  160 May 20 10:00 src\n\
        -rw-r--r--  1 user staff 4.0K May 20 10:00 Cargo.toml\n\
        -rw-r--r--  1 user staff 1.2M May 20 10:00 binary.dat\n\
        -rw-r--r--  1 user staff 2.5G May 20 10:00 huge.bin\n";

    let result =
        lean_ctx::core::patterns::ls::compress(output).expect("should compress ls -lah output");
    assert!(
        result.contains("4.0K"),
        "4.0K should pass through: {result}"
    );
    assert!(
        result.contains("1.2M"),
        "1.2M should pass through: {result}"
    );
    assert!(
        result.contains("2.5G"),
        "2.5G should pass through: {result}"
    );
    assert!(
        !result.contains("  0B"),
        "human-readable sizes must not become 0B: {result}"
    );
}

#[test]
fn ls_l_raw_sizes_converted() {
    let output = "total 32\n\
        -rw-r--r--  1 user staff  4096 May 20 10:00 Cargo.toml\n\
        -rw-r--r--  1 user staff 12288 May 20 10:00 Cargo.lock\n\
        -rw-r--r--  1 user staff   512 May 20 10:00 README.md\n\
        -rw-r--r--  1 user staff  1024 May 20 10:00 build.rs\n\
        drwxr-xr-x  5 user staff   160 May 20 10:00 src\n\
        drwxr-xr-x  3 user staff    96 May 20 10:00 tests\n";

    let result =
        lean_ctx::core::patterns::ls::compress(output).expect("should compress ls -l output");
    assert!(result.contains("4.0K"), "4096 should become 4.0K: {result}");
    assert!(
        result.contains("12.0K"),
        "12288 should become 12.0K: {result}"
    );
    assert!(result.contains("512B"), "512 should become 512B: {result}");
}

#[test]
fn ls_mixed_suffix_variants() {
    let output = "total 100K\n\
        -rw-r--r--  1 user staff  15K May 20 10:00 small.txt\n\
        -rw-r--r--  1 user staff 3.5T May 20 10:00 enormous.dat\n\
        -rw-r--r--  1 user staff 4.0K May 20 10:00 config.toml\n\
        -rw-r--r--  1 user staff 1.2M May 20 10:00 data.bin\n\
        drwxr-xr-x  5 user staff  160 May 20 10:00 src\n";

    let result =
        lean_ctx::core::patterns::ls::compress(output).expect("should compress mixed sizes");
    assert!(result.contains("15K"), "15K should pass through: {result}");
    assert!(
        result.contains("3.5T"),
        "3.5T should pass through: {result}"
    );
}

// ──────────────────────────────────────────────────────────
// Scenario Group 8: UTF-8 truncation safety
// User: has files with non-ASCII content that gets truncated
// at compression boundaries.
// ──────────────────────────────────────────────────────────

// ──────────────────────────────────────────────────────────
// Scenario Group 10: Crash hardening — ctx fields
// Verify that ToolContext optional fields return errors
// instead of panicking.
// ──────────────────────────────────────────────────────────

#[test]
fn crash_hardening_no_remaining_unwrap_on_ctx_fields() {
    const FIELDS: &[&str] = &[
        "cache",
        "session",
        "workflow",
        "agent_id",
        "client_name",
        "tool_calls",
        "ledger",
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tools/registered");
    let mut scanned = 0;
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("src/tools/registered exists") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        scanned += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        for field in FIELDS {
            if text.contains(&format!("ctx.{field}.as_ref().unwrap()")) {
                hits.push(format!("{}: ctx.{field}", path.display()));
            }
        }
    }
    assert!(scanned > 0, "no tool sources found under {}", dir.display());
    assert!(
        hits.is_empty(),
        "Found remaining .as_ref().unwrap() on ctx fields:\n{}",
        hits.join("\n")
    );
}

// ──────────────────────────────────────────────────────────
// Scenario Group 11: hash_fast UTF-8 safety
// ──────────────────────────────────────────────────────────

#[test]
fn hash_fast_with_real_world_cyrillic_plan_file() {
    let mut content = String::from("# EYE-343 §8.8 — план реализации\n\n");
    content.push_str("> Спецификация: [`docs/korobka-arch.md` §8.8]\n\n");
    content.push_str("## Шаг 1: Подготовка инфраструктуры\n\n");
    while content.len() < 20_000 {
        content.push_str("Дополнительная строка с кириллицей для тестирования. ");
    }

    let h1 = lean_ctx::server::helpers::hash_fast(&content);
    let h2 = lean_ctx::server::helpers::hash_fast(&content);
    assert_eq!(h1, h2, "hash must be deterministic");
    assert!(!h1.is_empty());
}

// ──────────────────────────────────────────────────────────
// Scenario Group 12: doctor --fix crash loop reset
// Verifies that the process name constant is shared between
// crash_loop_backoff and reset_crash_loop.
// ──────────────────────────────────────────────────────────

#[test]
fn crash_loop_reset_uses_same_name_as_backoff() {
    let _env = lean_ctx::core::data_dir::test_env_lock();
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::set_var("LEAN_CTX_DATA_DIR", dir.path()) };

    let name = lean_ctx::core::startup_guard::MCP_PROCESS_NAME;

    for _ in 0..5 {
        lean_ctx::core::startup_guard::crash_loop_backoff(name);
    }

    let log_path = dir.path().join(format!(".{name}-starts.log"));
    assert!(
        log_path.exists(),
        "crash log should exist after backoff calls"
    );

    lean_ctx::core::startup_guard::reset_crash_loop(name);
    assert!(
        !log_path.exists(),
        "reset_crash_loop with same name must delete the log"
    );

    // SAFETY: serialized by `test_env_lock()`.
    unsafe { std::env::remove_var("LEAN_CTX_DATA_DIR") };
}

#[test]
fn hash_fast_with_mixed_scripts_at_4096_boundary() {
    let mut s = String::new();
    while s.len() < 4090 {
        s.push('a');
    }
    s.push_str("日本語");
    while s.len() < 20_000 {
        s.push('x');
    }

    let hash = lean_ctx::server::helpers::hash_fast(&s);
    assert!(!hash.is_empty(), "must handle CJK at 4096 boundary");
}

// ──────────────────────────────────────────────────────────
// Scenario Group: Uninstall .bak cleanup
// ──────────────────────────────────────────────────────────

#[test]
fn uninstall_bak_path_generation() {
    use std::path::Path;
    let p = Path::new("/home/user/.cursor/settings.json");
    let bak = lean_ctx::uninstall::bak_path_for(p);
    assert!(
        bak.to_string_lossy().ends_with(".lean-ctx.bak"),
        "Backup must use .lean-ctx.bak suffix"
    );
    assert!(
        bak.to_string_lossy().contains("settings.json"),
        "Backup must preserve original filename"
    );
}

// ──────────────────────────────────────────────────────────
// Scenario Group: Windows drive-colon grep parsing
// ──────────────────────────────────────────────────────────

#[test]
fn grep_parser_handles_windows_drive_letter() {
    use lean_ctx::core::patterns::grep::compress;
    let mut input = String::new();
    for i in 1..=10 {
        input.push_str(&format!(
            "C:\\Users\\dev\\src\\main.rs:{i}:fn handler_{i}() {{}}\n"
        ));
    }
    let result = compress(&input);
    assert!(result.is_some(), "Must parse Windows drive paths");
    let r = result.unwrap();
    assert!(
        r.contains("main.rs"),
        "Must extract filename from Windows path: {r}"
    );
}

#[test]
fn grep_parser_handles_unix_paths() {
    use lean_ctx::core::patterns::grep::compress;
    let mut input = String::new();
    for i in 1..=10 {
        input.push_str(&format!("src/main.rs:{i}:fn handler_{i}() {{}}\n"));
    }
    let result = compress(&input);
    assert!(result.is_some(), "Must parse Unix paths");
    assert!(
        result.unwrap().contains("main.rs"),
        "Must extract filename from Unix path"
    );
}

#[test]
fn grep_parser_handles_relative_dotslash() {
    use lean_ctx::core::patterns::grep::compress;
    let mut input = String::new();
    for i in 1..=10 {
        input.push_str(&format!("./src/app.ts:{i}:export const val_{i} = {i};\n"));
    }
    let result = compress(&input);
    assert!(result.is_some(), "Must parse ./ relative paths");
    assert!(
        result.unwrap().contains("app.ts"),
        "Must extract filename from ./ path"
    );
}

// ──────────────────────────────────────────────────────────
// Scenario Group: Shell tokenizer (paths with spaces)
// ──────────────────────────────────────────────────────────

#[test]
fn hook_shell_tokenize_respects_quotes() {
    use lean_ctx::hook_handlers::shell_tokenize;
    let tokens = shell_tokenize(r#"cat "My Documents/file.txt""#);
    assert_eq!(tokens, vec!["cat", "My Documents/file.txt"]);
}

#[test]
fn hook_shell_tokenize_handles_single_quotes() {
    use lean_ctx::hook_handlers::shell_tokenize;
    let tokens = shell_tokenize("rg 'hello world' src/");
    assert_eq!(tokens, vec!["rg", "hello world", "src/"]);
}

#[test]
fn hook_shell_tokenize_simple_no_quotes() {
    use lean_ctx::hook_handlers::shell_tokenize;
    let tokens = shell_tokenize("ls src/components");
    assert_eq!(tokens, vec!["ls", "src/components"]);
}

#[test]
fn hook_shell_quote_adds_quotes_for_spaces() {
    use lean_ctx::hook_handlers::shell_quote;
    let q = shell_quote("My Documents/file.txt");
    assert!(q.starts_with('"') && q.ends_with('"'));
    assert!(q.contains("My Documents"));
}

#[test]
fn hook_shell_quote_noop_for_simple_paths() {
    use lean_ctx::hook_handlers::shell_quote;
    let q = shell_quote("src/main.rs");
    assert_eq!(q, "src/main.rs");
}
