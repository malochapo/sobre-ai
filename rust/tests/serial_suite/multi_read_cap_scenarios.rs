use sobre::core::cache::SessionCache;
use sobre::tools::CrpMode;
use sobre::tools::impl_multi_read;

/// The multi-read output cap (512 KiB, a fixed constant).
const CAP_BYTES: usize = 512 * 1024;

fn setup_test_files(count: usize, size_per_file: usize) -> (tempfile::TempDir, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let mut paths = Vec::new();
    for i in 0..count {
        let path = dir.path().join(format!("file_{i}.txt"));
        let content = format!("// File {i}\n{}", "x".repeat(size_per_file));
        std::fs::write(&path, &content).unwrap();
        paths.push(path.to_string_lossy().to_string());
    }
    (dir, paths)
}

#[test]
fn multi_read_respects_output_cap() {
    // 20 files of CAP/10 bytes each: twice the cap in total.
    let (_dir, paths) = setup_test_files(20, CAP_BYTES / 10);

    let mut cache = SessionCache::new();
    let output = impl_multi_read::handle_with_task_fresh(
        &mut cache,
        &paths,
        "full",
        false,
        CrpMode::Off,
        None,
    );

    assert!(
        output.contains("Output capped"),
        "output must contain cap warning when exceeding limit:\n{}",
        &output[output.len().saturating_sub(300)..]
    );
    // The files left out are named, and none of them counts as read: a
    // later sobre_read returns its content, not an [unchanged] stub.
    assert!(output.contains("Not read:"), "must name the files left out");
    assert!(output.contains("file_19.txt"), "the last file is left out");
    let again = sobre::tools::sobre_read::handle(&mut cache, &paths[19], "full", CrpMode::Off);
    assert!(
        again.contains("// File 19"),
        "{}",
        &again[..again.len().min(200)]
    );
}

#[test]
fn multi_read_no_cap_when_under_limit() {
    let (_dir, paths) = setup_test_files(3, 100);

    let mut cache = SessionCache::new();
    let output = impl_multi_read::handle_with_task_fresh(
        &mut cache,
        &paths,
        "full",
        false,
        CrpMode::Off,
        None,
    );

    assert!(
        !output.contains("Output capped"),
        "should not cap when under limit"
    );
    assert!(output.contains("Read 3 files"), "should read all files");
}

#[test]
fn multi_read_empty_paths() {
    let mut cache = SessionCache::new();
    let output =
        impl_multi_read::handle_with_task_fresh(&mut cache, &[], "full", false, CrpMode::Off, None);
    assert!(output.contains("Read 0 files"));
}

#[test]
fn multi_read_single_large_file_passes() {
    // One file larger than the whole cap.
    let (_dir, paths) = setup_test_files(1, CAP_BYTES + 50_000);

    let mut cache = SessionCache::new();
    let output = impl_multi_read::handle_with_task_fresh(
        &mut cache,
        &paths,
        "full",
        false,
        CrpMode::Off,
        None,
    );

    assert!(
        output.contains("Read 1 files"),
        "single file should always be included even if large"
    );
    assert!(
        !output.contains("Output capped"),
        "single file should not trigger cap"
    );
}
