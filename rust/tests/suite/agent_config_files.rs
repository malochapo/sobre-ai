//! The agent configuration files sobre edits: where Zed's is, JSONC with
//! non-ASCII comments and values, and the name of the backup `init` keeps.

#[test]
fn zed_target_points_at_its_settings_file() {
    let home = dirs::home_dir().unwrap_or_default();
    let targets = sobre::core::editor_registry::detect::build_targets(&home);
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

#[test]
fn jsonc_cyrillic_comments_preserved() {
    let input = r#"{
  // Привет мир — настройки
  "key": "значение"
}"#;
    let v = sobre::core::jsonc::parse_jsonc(input).unwrap();
    assert_eq!(v["key"], "значение");
}

#[test]
fn jsonc_cjk_values_preserved() {
    let input = r#"{
  "name": "テストプロジェクト",
  /* 日本語のコメント */
  "path": "文档/项目"
}"#;
    let v = sobre::core::jsonc::parse_jsonc(input).unwrap();
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
    let v = sobre::core::jsonc::parse_jsonc(input).unwrap();
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
    let v = sobre::core::jsonc::parse_jsonc(input).unwrap();
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
    let v = sobre::core::jsonc::parse_jsonc(input).unwrap();
    assert_eq!(v["status"], "ок");
}

#[test]
fn backup_keeps_the_file_name_and_adds_the_sobre_suffix() {
    let p = std::path::Path::new("/home/user/.cursor/settings.json");
    let bak = sobre::config_io::backup_path_for(p);
    assert!(
        bak.to_string_lossy().ends_with(".sobre.bak"),
        "Backup must use .sobre.bak suffix"
    );
    assert!(
        bak.to_string_lossy().contains("settings.json"),
        "Backup must preserve original filename"
    );
}
