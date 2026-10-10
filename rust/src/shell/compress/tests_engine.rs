#[allow(unused_imports)]
use super::*;
/// Already-compact TOON output must be preserved verbatim (not recompressed)
/// regardless of the command, because re-compressing it destroys the exact
/// line/field shape agents use to validate CLI output contracts.
#[cfg(test)]
mod toon_passthrough_tests {
    use super::super::engine::compress_if_beneficial;

    /// Builds a TOON tabular block large enough to clear the 30-token floor that
    /// otherwise returns small outputs unchanged anyway.
    fn toon_task_list(rows: usize) -> String {
        let mut out = String::from("task_count: ");
        out.push_str(&rows.to_string());
        out.push_str("\ntasks[");
        out.push_str(&rows.to_string());
        out.push_str("]{id,status,priority,title}:\n");
        for i in 0..rows {
            out.push_str(&format!(
                "  task-{i},open,p{},Implement feature number {i} end to end\n",
                i % 3
            ));
        }
        out
    }

    #[test]
    fn toon_output_is_preserved_verbatim() {
        let toon = toon_task_list(12);
        let result = compress_if_beneficial("vida task list", &toon);
        assert_eq!(
            result, toon,
            "TOON output must pass through unchanged, got: {result}"
        );
    }

    #[test]
    fn toon_passthrough_is_command_agnostic() {
        // A command sobre would normally compress still passes TOON through,
        // because the decision is output-shape based, not command based.
        let toon = toon_task_list(20);
        let result = compress_if_beneficial("my-tool report --format toon", &toon);
        assert_eq!(result, toon, "TOON must pass through for any command");
    }

    #[test]
    fn non_toon_output_is_still_compressible() {
        // A noisy, repetitive log that is NOT TOON should not be forced verbatim
        // by the passthrough — the normal pipeline still applies.
        let mut log = String::new();
        for i in 0..200 {
            log.push_str(&format!(
                "2026-06-04T10:00:{:02}Z INFO worker processed item {i} successfully in 12ms\n",
                i % 60
            ));
        }
        let result = compress_if_beneficial("./run-batch.sh", &log);
        assert_ne!(
            result, log,
            "non-TOON noisy log should not be forced verbatim by the TOON passthrough"
        );
    }
}

/// Exit-code-aware compression: a command that actually FAILED must
/// never be lossily compressed, so the agent always sees the real error and never
/// re-runs the command without sobre.
#[cfg(test)]
mod outcome_aware_tests {
    use super::super::engine::{compress_and_measure, compress_for_outcome};

    /// A noisy, repetitive log a successful run WOULD compress.
    fn noisy_log() -> String {
        let mut log = String::new();
        for i in 0..200 {
            log.push_str(&format!(
                "2026-06-04T10:00:{:02}Z INFO worker processed item {i} successfully in 12ms\n",
                i % 60
            ));
        }
        log
    }

    #[test]
    fn failed_generic_command_is_verbatim() {
        let log = noisy_log();
        let failed = compress_for_outcome("./run-batch.sh", &log, 1);
        assert_eq!(
            failed, log,
            "a failed command's output must be preserved verbatim, not digested"
        );
    }

    #[test]
    fn succeeding_command_still_compresses() {
        let log = noisy_log();
        let ok = compress_for_outcome("./run-batch.sh", &log, 0);
        assert_ne!(
            ok, log,
            "a succeeding command's compressible output should still compress"
        );
    }

    #[test]
    fn empty_failed_output_stays_empty() {
        assert_eq!(compress_for_outcome("flaky-check", "   \n  ", 1), "");
    }

    #[test]
    fn failed_command_keeps_buried_error_line() {
        let mut out = String::from("starting deploy\n");
        out.push_str("uploading artifact 1 of 3\n");
        out.push_str("ERR_PERMISSION: cannot write to /opt/app (needle-9b1e4d77)\n");
        out.push_str("rolling back\n");
        let failed = compress_for_outcome("./deploy.sh prod", &out, 13);
        assert!(
            failed.contains("needle-9b1e4d77"),
            "the actual error line must survive on a failed command: {failed}"
        );
    }

    #[test]
    fn measure_labels_stderr_only_on_failure() {
        let (failed, _) = compress_and_measure("./build.sh", "compiling main", "linker error", 1);
        assert!(
            failed.contains(crate::shell::STDERR_LABEL),
            "failure with both streams must label stderr: {failed}"
        );
        assert!(failed.contains("compiling main") && failed.contains("linker error"));

        let (ok, _) = compress_and_measure("./build.sh", "compiling main", "note: ok", 0);
        assert!(
            !ok.contains(crate::shell::STDERR_LABEL),
            "success output must not inject the stderr label: {ok}"
        );
    }
}
/// MSBuild parallel diagnostics dedup
#[test]
fn msbuild_parallel_dedup_removes_duplicate_diagnostics() {
    let output = "Microsoft (R) Build Engine\n\
Build started\n\
src/Foo.cs(10,5): warning CS1591: Missing XML comment [node1]\n\
src/Bar.cs(20,3): warning CS0168: Variable is declared but never used [node1]\n\
src/Foo.cs(10,5): warning CS1591: Missing XML comment [node2]\n\
src/Bar.cs(20,3): warning CS0168: Variable is declared but never used [node2]\n\
src/Baz.cs(30,1): error CS1002: ; expected [node1]\n\
Build succeeded with warnings.\n\
    2 Warning(s)\n\
    1 Error(s)\n";

    let deduped = super::engine::dedup_build_diagnostics(output);
    // Each diagnostic should appear only once
    assert_eq!(
        deduped.matches("CS1591").count(),
        1,
        "CS1591 should appear once after dedup: {deduped}"
    );
    assert_eq!(
        deduped.matches("CS0168").count(),
        1,
        "CS0168 should appear once after dedup: {deduped}"
    );
    // Error line should still be there
    assert!(deduped.contains("CS1002"), "error line must be preserved");
    // The dedup note says so, as sobre
    assert!(
        deduped.contains("[sobre: ") && deduped.contains("duplicate diagnostic(s) removed]"),
        "dedup note missing: {deduped}"
    );
}

// Chained commands must not be pattern-compressed
#[test]
fn chained_command_detection() {
    use super::engine::is_chained_command;
    assert!(is_chained_command("git fetch && git worktree add /tmp/wt"));
    assert!(is_chained_command("git status; git log --oneline -3"));
    assert!(!is_chained_command("git status"));
    assert!(!is_chained_command("echo 'a && b'"));
    assert!(!is_chained_command("echo \"a; b\""));
}

#[test]
fn chained_git_fetch_preserves_worktree_output() {
    let cmd = "git fetch origin feat && git worktree add /tmp/wt -b feat origin/feat";
    let output = "From github.com:org/repo\n * branch feat -> FETCH_HEAD\nPreparing worktree (new branch 'feat')\nHEAD is now at abc1234 initial commit\n";
    let compressed = super::engine::compress_if_beneficial_pub(cmd, output);
    assert!(
        compressed.contains("worktree") || compressed.contains("Preparing"),
        "worktree output must survive chain compression, got: {compressed}"
    );
}

// Small output should NOT be compressed — verbatim is cheaper
#[test]
fn small_output_stays_verbatim() {
    let cmd = "ls -la /tmp/templates";
    let output: String = (0..59)
        .map(|i| format!("template_{i:02}.yaml"))
        .collect::<Vec<_>>()
        .join("\n");
    let compressed = super::engine::compress_if_beneficial_pub(cmd, &output);
    assert_eq!(
        compressed, output,
        "output under 200 tokens must stay verbatim, got: {compressed}"
    );
}

/// The guard the pattern snapshot tests use follows the gates in front of
/// the pattern table: test runners, findings-only linters, the output
/// policy and chains never reach it.
#[test]
fn reaches_pattern_table_follows_the_gates() {
    use super::engine::reaches_pattern_table;
    for cmd in [
        "cargo build",
        "git status",
        "grep -rn TODO src/",
        "ruff check .",
    ] {
        assert!(reaches_pattern_table(cmd), "{cmd}");
    }
    for cmd in [
        "cargo test",
        "pytest -q",
        "eslint src/",
        "curl https://api.example.com",
        "npx prisma migrate dev",
        "git commit -m x",
        "git status && git log",
    ] {
        assert!(!reaches_pattern_table(cmd), "{cmd}");
    }
}

/// A successful build's warnings reach the agent: every distinct one, with
/// its location.
#[cfg(test)]
mod build_warning_tests {
    use super::super::engine::compress_for_outcome;

    /// `npm run` keeps every warning and error line, not only its last
    /// lines; npm's own `npm warn` lines included.
    #[test]
    fn npm_run_keeps_its_warnings() {
        let mut out = String::from("\n> app@1.0.0 build\n> webpack --mode production\n\n");
        for i in 0..40 {
            out.push_str(&format!(
                "asset chunk-{i}.js {}.{} KiB [emitted] [minimized] (name: chunk-{i})\n",
                10 + i,
                i % 10
            ));
        }
        out.push_str("WARNING in ./src/legacy.js 12:4-20\n");
        out.push_str("Module not found: Error: Can't resolve 'fs' in '/app/src'\n");
        out.push_str("npm warn config production Use `--omit=dev` instead.\n");
        for i in 0..40 {
            out.push_str(&format!(
                "orphan modules {i}.{i} KiB [orphan] {i} modules\n"
            ));
        }
        out.push_str("webpack 5.89.0 compiled with 1 warning in 4321 ms\n");

        let result = compress_for_outcome("npm run build", &out, 0);
        for line in [
            "WARNING in ./src/legacy.js 12:4-20",
            "Module not found: Error: Can't resolve 'fs' in '/app/src'",
            "npm warn config production Use `--omit=dev` instead.",
        ] {
            assert!(result.contains(line), "{line:?} lost:\n{result}");
        }
    }

    #[test]
    fn dotnet_build_keeps_every_warning() {
        let warning = |i: usize| {
            format!(
                "/home/u/app/Services/Service{i}.cs({i},7): warning CS0168: The variable 'unused{i}' is declared but never used [/home/u/app/app.csproj]"
            )
        };
        let mut out = String::from(
            "  Determining projects to restore...\n  All projects are up-to-date for restore.\n",
        );
        for i in 0..40 {
            out.push_str(&warning(i));
            out.push('\n');
        }
        out.push_str("  app -> /home/u/app/bin/Debug/net8.0/app.dll\n\nBuild succeeded.\n\n");
        for i in 0..40 {
            out.push_str(&warning(i));
            out.push('\n');
        }
        out.push_str("    40 Warning(s)\n    0 Error(s)\n\nTime Elapsed 00:00:04.12\n");

        let result = compress_for_outcome("dotnet build", &out, 0);
        for i in 0..40 {
            assert!(
                result.contains(&format!("Service{i}.cs({i},7): warning CS0168")),
                "warning {i} lost:\n{result}"
            );
        }
    }

    #[test]
    fn cargo_doc_keeps_its_warnings() {
        let mut out = String::new();
        for i in 0..150 {
            out.push_str(&format!(" Documenting dep{i} v0.1.{i}\n"));
        }
        for i in 0..30 {
            out.push_str(&format!(
                "warning: unresolved link to `Missing{i}`\n --> src/lib.rs:{i}:5\n  |\n{i} | /// See [`Missing{i}`].\n  |            ^^^^^^^^ no item named `Missing{i}` in scope\n\n"
            ));
        }
        out.push_str("warning: `app` (lib doc) generated 30 warnings\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.87s\n   Generated /home/u/app/target/doc/app/index.html\n");

        let result = compress_for_outcome("cargo doc --no-deps", &out, 0);
        for i in 0..30 {
            assert!(
                result.contains(&format!("unresolved link to `Missing{i}`"))
                    && result.contains(&format!("src/lib.rs:{i}:5")),
                "warning {i} lost:\n{result}"
            );
        }
    }

    #[test]
    fn eslint_warnings_keep_their_locations() {
        let rules = ["no-unused-vars", "eqeqeq", "no-console", "prefer-const"];
        let mut out = String::new();
        for f in 0..6 {
            out.push_str(&format!("\n/home/u/app/src/module{f}.js\n"));
            for l in 0..5 {
                let rule = rules[(f + l) % rules.len()];
                out.push_str(&format!(
                    "  {}:{}  warning  Problem number {l} in module {f}  {rule}\n",
                    l * 10 + 1,
                    l + 3
                ));
            }
        }
        out.push_str("\n✖ 30 problems (0 errors, 30 warnings)\n");

        let result = compress_for_outcome("eslint src/", &out, 0);
        for f in 0..6 {
            for l in 0..5 {
                assert!(
                    result.contains(&format!("module{f}.js"))
                        && result.contains(&format!("{}:{}", l * 10 + 1, l + 3))
                        && result.contains(&format!("Problem number {l} in module {f}")),
                    "warning {f}/{l} lost:\n{result}"
                );
            }
        }
    }

    #[test]
    fn swiftlint_warnings_keep_their_locations() {
        let mut out = String::from("Linting Swift files in current working directory\n");
        for f in 0..60 {
            out.push_str(&format!("Linting 'File{f}.swift' ({}/60)\n", f + 1));
        }
        for f in 0..12 {
            out.push_str(&format!(
                "/repo/Sources/File{f}.swift:{}:5: warning: Line Length Violation: Line should be 120 characters or less; currently it has {} characters (rule_{f})\n",
                f + 10,
                f + 121
            ));
        }
        out.push_str("Done linting! Found 12 violations, 0 serious in 60 files.\n");

        let result = compress_for_outcome("swiftlint", &out, 0);
        for f in 0..12 {
            assert!(
                result.contains(&format!(
                    "/repo/Sources/File{f}.swift:{}:5: warning",
                    f + 10
                )),
                "violation {f} lost:\n{result}"
            );
        }
    }
}
