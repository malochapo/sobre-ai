//! `ls` output reaches the agent as printed, sizes included, and grep hits
//! are grouped by file whatever the path looks like.

/// A long `ls -l` listing, human-readable sizes and byte counts alike, comes
/// back from `sobre_shell` unchanged: the output policy keeps file listings
/// verbatim.
#[test]
fn ls_output_reaches_the_agent_unchanged() {
    let sizes = ["4.0K", "1.2M", "2.5G", "3.5T", "512", "12288"];
    let mut output = String::from("total 4.1G\n");
    for (i, size) in sizes.iter().cycle().take(60).enumerate() {
        output.push_str(&format!(
            "-rw-r--r--  1 user staff {size:>6} May 20 10:00 file_{i}.dat\n"
        ));
    }
    for cmd in ["ls -lah", "ls -l"] {
        let shown = sobre::tools::sobre_shell::handle(cmd, &output, 0, sobre::tools::CrpMode::Off);
        assert_eq!(shown, output, "{cmd}");
    }
}

#[test]
fn grep_parser_handles_windows_drive_letter() {
    use sobre::core::patterns::grep::compress;
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
    use sobre::core::patterns::grep::compress;
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
    use sobre::core::patterns::grep::compress;
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
