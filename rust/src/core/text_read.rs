/// Reads a file as lossy UTF-8, rejecting binary files.
pub fn read_file_lossy(path: &str) -> Result<String, std::io::Error> {
    if crate::core::binary_detect::is_binary_file(path) {
        let msg = crate::core::binary_detect::binary_file_message(path);
        return Err(std::io::Error::other(msg));
    }
    std::fs::read(path).map(|b| strip_utf8_bom(String::from_utf8_lossy(&b).into_owned()))
}

/// A UTF-8 BOM is an encoding artifact, not content — leaking it corrupts the
/// first line of every downstream view (limitations doc #11). Shared by both
/// file readers (this module's and `tools::ctx_read::read_file_lossy`).
pub(crate) fn strip_utf8_bom(s: String) -> String {
    match s.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_owned(),
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The CLI full-read path (`cli_cache::check_and_read`) reads through THIS
    // `read_file_lossy`, not the ctx_read one — both must strip the UTF-8 BOM
    // or the CLI leaks it while the MCP path doesn't (limitations doc #11).
    #[test]
    fn read_file_lossy_strips_utf8_bom() {
        let p = std::env::temp_dir().join("lean_ctx_io_bom_test.txt");
        std::fs::write(&p, b"\xEF\xBB\xBFhello\n").unwrap();
        let s = read_file_lossy(p.to_str().unwrap()).unwrap();
        let _ = std::fs::remove_file(&p);
        assert!(!s.starts_with('\u{feff}'), "BOM must be stripped");
        assert!(s.starts_with("hello"));
    }
}
