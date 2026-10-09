//! `ls` sizes and `grep` paths survive their compression patterns: sizes
//! already human-readable pass through, byte sizes are converted, and grep
//! hits are grouped by file whatever the path looks like.

#[test]
fn ls_lah_human_readable_sizes_passthrough() {
    let output = "total 32K\n\
        drwxr-xr-x  5 user staff  160 May 20 10:00 src\n\
        -rw-r--r--  1 user staff 4.0K May 20 10:00 Cargo.toml\n\
        -rw-r--r--  1 user staff 1.2M May 20 10:00 binary.dat\n\
        -rw-r--r--  1 user staff 2.5G May 20 10:00 huge.bin\n";

    let result =
        sobre::core::patterns::ls::compress(output).expect("should compress ls -lah output");
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

    let result = sobre::core::patterns::ls::compress(output).expect("should compress ls -l output");
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

    let result = sobre::core::patterns::ls::compress(output).expect("should compress mixed sizes");
    assert!(result.contains("15K"), "15K should pass through: {result}");
    assert!(
        result.contains("3.5T"),
        "3.5T should pass through: {result}"
    );
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
