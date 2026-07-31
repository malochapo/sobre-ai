//! Setup helper routines (skill install, TOML key upserts, profile + premium
//! feature configuration). Split out of `setup/mod.rs` for focus.

#[allow(clippy::wildcard_imports)]

pub fn install_skill_files(home: &std::path::Path) -> Vec<(String, bool)> {
    crate::rules_inject::install_all_skills(home)
}

pub(crate) fn install_kiro_steering(home: &std::path::Path) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| home.to_path_buf());
    let steering_dir = cwd.join(".kiro").join("steering");
    let steering_file = steering_dir.join("lean-ctx.md");

    if steering_file.exists()
        && std::fs::read_to_string(&steering_file)
            .unwrap_or_default()
            .contains("lean-ctx")
    {
        println!("  Kiro steering file already exists at .kiro/steering/lean-ctx.md");
        return;
    }

    let _ = std::fs::create_dir_all(&steering_dir);
    let _ = std::fs::write(&steering_file, crate::hooks::kiro_steering_content());
    println!(
        "  \x1b[32m✓\x1b[0m Created .kiro/steering/lean-ctx.md (Kiro will now prefer lean-ctx tools)"
    );
}

pub(crate) fn shorten_path(path: &str, home: &str) -> String {
    if let Some(stripped) = path.strip_prefix(home) {
        format!("~{stripped}")
    } else {
        path.to_string()
    }
}
