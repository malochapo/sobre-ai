//! `sobre init --repair` on a fresh machine with detected but unconfigured
//! agents leaves `doctor` fully green: no check sends the user back to
//! `sobre init` right after it ran.

use std::process::Command;

#[test]
#[cfg_attr(
    windows,
    ignore = "HOME-override isolation is Unix-only (dirs::home_dir uses the Win32 API)"
)]
fn init_repair_leaves_doctor_fully_green() {
    let bin = env!("CARGO_BIN_EXE_sobre");

    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(proj.join(".git")).unwrap();

    // Agents that exist but were never wired to sobre. A bare state dir is
    // exactly what doctor treats as "installed" — init must agree.
    std::fs::create_dir_all(home.join(".cursor")).unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();

    let bin_dir = std::path::Path::new(bin).parent().unwrap();
    let envs: Vec<(String, String)> = vec![
        ("HOME".into(), home.to_string_lossy().into_owned()),
        (
            "PATH".into(),
            format!("{}:/usr/bin:/bin", bin_dir.to_string_lossy()),
        ),
        (
            "SOBRE_CONFIG_DIR".into(),
            home.join("cfg").to_string_lossy().into_owned(),
        ),
        (
            "SOBRE_DATA_DIR".into(),
            home.join("data").to_string_lossy().into_owned(),
        ),
        (
            "CODEX_HOME".into(),
            home.join(".codex").to_string_lossy().into_owned(),
        ),
        ("SHELL".into(), "/bin/bash".into()),
        ("NO_COLOR".into(), "1".into()),
        ("TERM".into(), "dumb".into()),
    ];
    std::fs::create_dir_all(home.join("cfg")).unwrap();
    let init = Command::new(bin)
        .args(["init", "--repair"])
        .current_dir(&proj)
        .env_clear()
        .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .output()
        .expect("init spawn");
    assert!(
        init.status.success(),
        "init --repair must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let doctor = Command::new(bin)
        .arg("doctor")
        .current_dir(&proj)
        .env_clear()
        .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .output()
        .expect("doctor spawn");
    let stdout = String::from_utf8_lossy(&doctor.stdout);

    // No check may send the user back to init right after it ran.
    assert!(
        !stdout.contains("run: sobre init"),
        "doctor must not point back to init right after it ran; doctor said:\n{stdout}"
    );

    // doctor is a health gate: exit 0 == every counted check green.
    assert!(
        doctor.status.success(),
        "doctor must be fully green right after init --repair; output:\n{stdout}"
    );
}
