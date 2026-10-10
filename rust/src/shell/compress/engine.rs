use crate::core::patterns;
use crate::core::tokens::count_tokens;

use super::classification::{has_structural_output, is_search_output, looks_like_toon};

pub(in crate::shell) fn compress_and_measure(
    command: &str,
    stdout: &str,
    stderr: &str,
    exit_code: i32,
) -> (String, usize) {
    let compressed_stdout = compress_for_outcome(command, stdout, exit_code);
    let compressed_stderr = compress_for_outcome(command, stderr, exit_code);

    let mut result = String::new();
    if !compressed_stdout.is_empty() {
        result.push_str(&compressed_stdout);
    }
    if !compressed_stderr.is_empty() {
        if !result.is_empty() {
            // On failure, label the stderr block so the agent can attribute the
            // error (mirrors `shell::combine_streams`); success keeps the plain
            // join for byte-stable output.
            if exit_code != 0 {
                result.push('\n');
                result.push_str(crate::shell::STDERR_LABEL);
            }
            result.push('\n');
        }
        result.push_str(&compressed_stderr);
    }

    let output_tokens = count_tokens(&result);
    (result, output_tokens)
}

/// Compress one stream, but never lossily for a command that actually FAILED.
///
/// A non-zero exit keeps the output verbatim (size-capped via [`truncate_verbatim`]
/// with safety-needle-preserving head/tail truncation) so the real error always
/// reaches the model and the agent never has to re-run the command without
/// sobre. This generalizes the build-tool-error guard inside
/// [`compress_if_beneficial`] to ANY non-zero exit. Empty output and explicit
/// `<lc_safe>` spans keep the normal pipeline (the latter so its markers are
/// stripped correctly); a succeeding command still compresses as before.
pub(crate) fn compress_for_outcome(command: &str, output: &str, exit_code: i32) -> String {
    if exit_code != 0 && !output.trim().is_empty() && !crate::core::protect::has_markers(output) {
        return truncate_verbatim(output, count_tokens(output));
    }
    compress_if_beneficial_with_exit(command, output, exit_code)
}

/// Distinct-value ratio at/above which the lossy stage drops an all-present
/// column. Conservative: only near-unique noise (timestamps, UUIDs) is dropped,
/// so genuinely varying-but-meaningful columns are kept.
/// Minimum token savings to justify compression overhead (tee-log pointer
/// + potential read-back round-trip). Below this floor, verbatim is cheaper.
const MIN_USEFUL_SAVINGS: usize = 100;

pub(crate) fn compress_if_beneficial(command: &str, output: &str) -> String {
    compress_if_beneficial_with_exit(command, output, -1)
}

fn compress_if_beneficial_with_exit(command: &str, output: &str, exit_code: i32) -> String {
    if output.trim().is_empty() {
        return String::new();
    }

    // Honour explicit <lc_safe>…</lc_safe> spans. Secret redaction has
    // already run upstream (sobre_shell::handle → redact_shell_output_secrets), so
    // the pipeline order is redact → protect → compress and a marker can never
    // smuggle a secret past redaction. Protected spans pass through verbatim;
    // each unprotected segment flows through the normal pipeline.
    if crate::core::protect::has_markers(output) {
        return crate::core::protect::compress_preserving(output, |seg| {
            compress_if_beneficial(command, seg)
        });
    }

    // CRITICAL: Never compress error output from build/check/lint tools.
    // Compiler errors, type errors, lint findings etc. must be preserved verbatim
    // so the agent can see file paths, line numbers, and full diagnostics.
    // Folding compiler/test-runner *progress* noise composes with — never
    // replaces — the verbatim token cap: diagnostics stay verbatim, and a log
    // whose diagnostics alone exceed the budget is still head/tail-truncated
    // with safety-needle preservation.
    if is_error_output_from_build_tool(command, output) {
        let base =
            maybe_fold_progress(output, count_tokens(output)).unwrap_or_else(|| output.to_string());
        let base = dedup_build_diagnostics(&base);
        return truncate_verbatim(&base, count_tokens(&base));
    }

    // Test-runner output: structurally compress successful runs through the
    // dedicated test-pattern compressors (cargo test, pytest, jest, etc.).
    // Failed runs (exit_code != 0) stay verbatim to preserve failure diagnostics.
    if is_test_runner_command(command) {
        let base =
            maybe_fold_progress(output, count_tokens(output)).unwrap_or_else(|| output.to_string());
        if exit_code == 0
            && let Some(compressed) = patterns::test::compress(&base)
        {
            let original_tokens = count_tokens(output);
            let compressed_tokens = count_tokens(&compressed);
            if compressed_tokens < original_tokens {
                return compressed;
            }
        }
        return truncate_verbatim(&base, count_tokens(&base));
    }

    if !is_search_output(command) && crate::tools::sobre_shell::contains_auth_flow(output) {
        return output.to_string();
    }

    let original_tokens = count_tokens(output);

    // Small outputs are cheaper verbatim than compressed + tee-log round-trip.
    // The tee-log pointer alone costs ~50 tokens; a second read-back call doubles
    // the token cost. Outputs below this floor are never worth compressing.
    if original_tokens < 200 {
        return output.to_string();
    }

    let min_output_tokens = 20;

    let cfg = crate::core::config::Config::load();
    let policy = crate::shell::output_policy::classify(command, &cfg.passthrough_commands);
    if policy == crate::shell::output_policy::OutputPolicy::Verbatim
        || policy == crate::shell::output_policy::OutputPolicy::Passthrough
    {
        return truncate_verbatim(output, original_tokens);
    }

    // Format-aware passthrough: output already in a compact, token-oriented
    // format (TOON) is kept verbatim.
    // Recompressing it saves little and rewrites the exact line/field shape an
    // agent relies on to validate a CLI output contract. This is output-shape
    // based, so any tool emitting the format is covered without listing commands.
    if looks_like_toon(output) {
        return truncate_verbatim(output, original_tokens);
    }

    // Structural output AND version-control history are owned by their
    // dedicated compressor: apply it if it yields a gain, otherwise return the
    // output verbatim. Never let the generic terse/dedup/truncate fallbacks
    // below reshape it — they would corrupt commit subjects/hashes or drop
    // commits the caller explicitly requested (`git log --oneline -40`).
    if !is_chained_command(command)
        && (has_structural_output(command) || patterns::has_vcs_owner(command))
    {
        let cl = command.to_ascii_lowercase();
        if let Some(compressed) = patterns::try_specific_pattern(&cl, output)
            && !compressed.trim().is_empty()
        {
            let compressed_tokens = count_tokens(&compressed);
            let savings = original_tokens.saturating_sub(compressed_tokens);
            if compressed_tokens >= min_output_tokens
                && compressed_tokens < original_tokens
                && savings >= MIN_USEFUL_SAVINGS
            {
                return compressed;
            }
        }
        return output.to_string();
    }

    if !is_chained_command(command)
        && let Some(mut compressed) = patterns::compress_output(command, output)
        && !compressed.trim().is_empty()
    {
        let level = crate::core::config::CompressionLevel::effective(&cfg);
        if level.is_active() {
            let terse_result =
                crate::core::terse::pipeline::compress(output, &level, Some(&compressed));
            if terse_result.quality_passed {
                compressed = terse_result.output;
            }
        }

        let compressed_tokens = count_tokens(&compressed);
        let savings = original_tokens.saturating_sub(compressed_tokens);
        if compressed_tokens >= min_output_tokens
            && compressed_tokens < original_tokens
            && savings >= MIN_USEFUL_SAVINGS
        {
            let ratio = compressed_tokens as f64 / original_tokens as f64;
            if ratio < 0.05 && original_tokens > 100 && original_tokens < 2000 {
                tracing::warn!("compression removed >95% of small output, returning original");
                return output.to_string();
            }
            return compressed;
        }
        if compressed_tokens < min_output_tokens {
            return output.to_string();
        }
    }

    {
        let level = crate::core::config::CompressionLevel::effective(&cfg);
        if level.is_active() {
            let terse_result = crate::core::terse::pipeline::compress(output, &level, None);
            if terse_result.quality_passed && terse_result.savings_pct >= 3.0 {
                return terse_result.output;
            }
        }
    }

    let cleaned = crate::core::compressor::lightweight_cleanup(output);
    let cleaned_tokens = count_tokens(&cleaned);
    if cleaned_tokens < original_tokens {
        let lines: Vec<&str> = cleaned.lines().collect();
        if lines.len() > 30 {
            let compressed = truncate_with_safety_scan(&lines, original_tokens);
            if let Some(c) = compressed {
                return c;
            }
        }
        if cleaned_tokens < original_tokens {
            return cleaned;
        }
    }

    let lines: Vec<&str> = output.lines().collect();
    if lines.len() > 30
        && let Some(c) = truncate_with_safety_scan(&lines, original_tokens)
    {
        return c;
    }

    output.to_string()
}

/// Detects whether the output contains error diagnostics from a build/check/lint tool.
/// When true, compression is bypassed to preserve file paths, line numbers, and messages.
/// Deduplicate identical diagnostic lines in build output.
/// MSBuild parallel builds print each warning/error twice: once per build-node
/// and once in the summary section. This removes exact duplicates while
/// preserving order (first occurrence wins).
pub(crate) fn dedup_build_diagnostics(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() < 10 {
        return output.to_string();
    }
    let mut seen = std::collections::HashSet::new();
    let mut deduped_count = 0usize;
    let mut result = Vec::with_capacity(lines.len());
    for line in &lines {
        let trimmed = line.trim();
        if is_diagnostic_line(trimmed) {
            let key = normalize_diagnostic_key(trimmed);
            if !seen.insert(key) {
                deduped_count += 1;
                continue;
            }
        }
        result.push(*line);
    }
    if deduped_count > 0 {
        result.push("");
        let note = format!("[sobre: {deduped_count} duplicate diagnostic(s) removed]");
        // We can't push a &str that references a local — collect into owned
        let mut out: String = result.join("\n");
        out.push('\n');
        out.push_str(&note);
        return out;
    }
    output.to_string()
}

/// Recognises MSBuild / compiler diagnostic lines (file(line): warning CS1234: ...)
/// and generic compiler diagnostics (file:line: warning/error: ...).
/// Normalize a diagnostic line for dedup: strip trailing `[project]` suffixes
/// that MSBuild appends per build node, so identical diagnostics from parallel
/// nodes collapse into one.
fn normalize_diagnostic_key(line: &str) -> String {
    let trimmed = line.trim();
    // MSBuild appends ` [/path/to/project.csproj]` — strip it.
    if let Some(bracket) = trimmed.rfind(" [")
        && trimmed.ends_with(']')
    {
        return trimmed[..bracket].to_string();
    }
    trimmed.to_string()
}

fn is_diagnostic_line(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    // MSBuild: path(line,col): warning CS1234: message
    // MSBuild: path(line,col): error CS1234: message
    if (line.contains("): warning ") || line.contains("): error ")) && line.contains('(') {
        return true;
    }
    // GCC/Clang/Rust: path:line:col: warning: / error: / note:
    if (line.contains(": warning:") || line.contains(": error:") || line.contains(": warning["))
        && line.contains(':')
    {
        return true;
    }
    false
}

/// Linters that print nothing but findings and can exit 0 with some: any
/// output they give is diagnostics, whatever words it uses (ESLint's stylish
/// format has no `warning:`, stylelint and biome mark severity with symbols,
/// a ruff finding is a rule code such as `F401`, under `--exit-zero`).
const FINDINGS_ONLY_LINTERS: &[&str] = &[
    "eslint",
    "npx eslint",
    "biome ",
    "stylelint",
    "yamllint",
    "oxlint",
    "ruff check",
];

fn is_findings_only_linter(command: &str) -> bool {
    let cmd = command.trim().to_ascii_lowercase();
    FINDINGS_ONLY_LINTERS.iter().any(|l| cmd.starts_with(l))
}

fn is_error_output_from_build_tool(command: &str, output: &str) -> bool {
    if is_findings_only_linter(command) {
        return true;
    }

    let cmd = command.trim().to_ascii_lowercase();
    let is_build_tool = cmd.starts_with("cargo check")
        || cmd.starts_with("cargo build")
        || cmd.starts_with("cargo clippy")
        || cmd.starts_with("cargo test")
        || cmd.starts_with("cargo fmt")
        || cmd.starts_with("cargo run")
        || cmd.starts_with("rustc ")
        || cmd.starts_with("gcc ")
        || cmd.starts_with("g++ ")
        || cmd.starts_with("clang ")
        || cmd.starts_with("clang++ ")
        || cmd.starts_with("make ")
        || cmd.starts_with("cmake ")
        || cmd.starts_with("go build")
        || cmd.starts_with("go vet")
        || cmd.starts_with("go test")
        || cmd.starts_with("golangci-lint")
        || cmd.starts_with("tsc ")
        || cmd.starts_with("tsc\t")
        || cmd == "tsc"
        || cmd.starts_with("npx tsc")
        || cmd.starts_with("prettier ")
        || cmd.starts_with("mypy ")
        || cmd.starts_with("pyright ")
        || cmd.starts_with("pylint ")
        || cmd.starts_with("flake8")
        || cmd.starts_with("black --check")
        || cmd.starts_with("swift build")
        || cmd.starts_with("swiftc ")
        || cmd.starts_with("xcodebuild ")
        || cmd.starts_with("javac ")
        || cmd.starts_with("gradle ")
        || cmd.starts_with("./gradlew ")
        || cmd.starts_with("mvn ")
        || cmd.starts_with("./mvnw ")
        || cmd.starts_with("dotnet build")
        || cmd.starts_with("dotnet test")
        || cmd.starts_with("msbuild")
        || cmd.starts_with("zig build")
        || cmd.starts_with("nim c ")
        || cmd.starts_with("ghc ")
        || cmd.starts_with("stack build")
        || cmd.starts_with("cabal build")
        || cmd.starts_with("mix compile")
        || cmd.starts_with("mix test")
        || cmd.starts_with("mix credo")
        || cmd.starts_with("shellcheck ")
        || cmd.starts_with("hadolint ")
        || cmd.starts_with("terraform validate")
        || cmd.starts_with("terraform plan")
        || cmd.starts_with("ansible-lint")
        || cmd.starts_with("rubocop ")
        || cmd.starts_with("solhint ")
        || cmd.starts_with("slither ");

    if !is_build_tool {
        return false;
    }

    // Check if the output actually contains error indicators
    output.contains("error[")
        || output.contains("error:")
        || output.contains("Error:")
        || output.contains("ERROR:")
        || output.contains(" error ")
        || output.contains("warning[")
        || output.contains("warning:")
        || output.contains("failed")
        || output.contains("FAILED")
        || output.contains("panicked at")
        || output.contains("cannot find")
        || output.contains("not found")
        || output.contains("undefined")
        || output.contains("unresolved")
        || output.contains("expected ")
        || output.contains("mismatched types")
        || output.contains("aborting due to")
        || output.contains("could not compile")
}

/// Strips leading `VAR=value` environment assignments from a command segment so
/// `RUST_BACKTRACE=1 cargo test` / `CI=true pytest` are still recognized as the
/// underlying test runner.
fn strip_env_prefix(segment: &str) -> &str {
    let mut rest = segment.trim_start();
    loop {
        let Some(first) = rest.split_whitespace().next() else {
            return rest;
        };
        // An env assignment is a single token containing '=' before any '/' so it
        // isn't confused with a path or a flag like `--threads=4`.
        let is_env_assignment = first.contains('=')
            && !first.starts_with('-')
            && first.split('=').next().is_some_and(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            });
        if !is_env_assignment {
            return rest;
        }
        rest = rest[first.len()..].trim_start();
    }
}

/// Detects test-runner commands across ecosystems. Their output must never be
/// semantically compressed/deduplicated — only verbatim head/tail truncation
/// (with middle test/error lines preserved). Matched even for fully-passing
/// runs so per-suite summaries always survive. Checks each pipeline segment so
/// `cargo test … | grep …` / `pytest … | tail` are caught too.
fn is_test_runner_command(command: &str) -> bool {
    command
        .split('|')
        .map(|seg| strip_env_prefix(seg.trim()).to_ascii_lowercase())
        .any(|seg| {
            seg.starts_with("cargo test")
                || seg.starts_with("cargo nextest")
                || seg.starts_with("nextest")
                || seg.starts_with("pytest")
                || seg.starts_with("python -m pytest")
                || seg.starts_with("python3 -m pytest")
                || seg.starts_with("py.test")
                || seg.starts_with("go test")
                || seg.starts_with("gotestsum")
                || seg.starts_with("npm test")
                || seg.starts_with("npm run test")
                || seg.starts_with("pnpm test")
                || seg.starts_with("pnpm run test")
                || seg.starts_with("yarn test")
                || seg.starts_with("bun test")
                || seg.starts_with("deno test")
                || seg.starts_with("jest")
                || seg.starts_with("npx jest")
                || seg.starts_with("vitest")
                || seg.starts_with("npx vitest")
                || seg.starts_with("mocha")
                || seg.starts_with("npx mocha")
                || seg.starts_with("dotnet test")
                || seg.starts_with("mix test")
                || seg.starts_with("rspec")
                || seg.starts_with("bundle exec rspec")
                || seg.starts_with("phpunit")
                || seg.starts_with("./vendor/bin/phpunit")
                || seg.starts_with("./gradlew test")
                || seg.starts_with("gradle test")
                || seg.starts_with("mvn test")
                || seg.starts_with("ctest")
        })
}

const MAX_VERBATIM_TOKENS: usize = 8000;

/// For verbatim commands: never transform content, only head/tail truncate if huge.
///
/// Even when truncating, every safety- and test-relevant line from the omitted
/// middle is preserved (test-result summaries, panics, failures, errors). This
/// guarantees a large test run — even a fully passing one with dozens of
/// per-suite `test result:` lines — never silently loses its outcome lines,
/// regardless of OS or client.
fn truncate_verbatim(output: &str, original_tokens: usize) -> String {
    if original_tokens <= MAX_VERBATIM_TOKENS {
        return output.to_string();
    }
    let lines: Vec<&str> = output.lines().collect();
    let total = lines.len();
    if total <= 60 {
        return output.to_string();
    }
    let head = 30.min(total);
    let tail = 20.min(total.saturating_sub(head));
    let middle = &lines[head..total - tail];

    // Preserve up to 200 safety/test/diagnostic lines from the omitted middle so
    // buried failures and per-suite summaries survive head/tail truncation.
    let preserved = crate::core::safety_needles::extract_safety_lines(middle, 200);
    let omitted = middle.len() - preserved.len();

    let mut result = String::with_capacity(output.len() / 2);
    for line in &lines[..head] {
        result.push_str(line);
        result.push('\n');
    }
    if preserved.is_empty() {
        result.push_str(&format!(
            "\n[{omitted} lines omitted — output too large for context window]\n\n"
        ));
    } else {
        result.push_str(&format!(
            "\n[{omitted} lines omitted, {} test/diagnostic lines preserved]\n",
            preserved.len()
        ));
        for line in &preserved {
            result.push_str(line);
            result.push('\n');
        }
        result.push('\n');
    }
    for line in lines.iter().skip(total - tail) {
        result.push_str(line);
        result.push('\n');
    }
    result
}

fn truncate_with_safety_scan(lines: &[&str], original_tokens: usize) -> Option<String> {
    use crate::core::safety_needles;

    let first = &lines[..5];
    let last = &lines[lines.len() - 5..];
    let middle = &lines[5..lines.len() - 5];

    let safety_lines = safety_needles::extract_safety_lines(middle, 80);
    let safety_count = safety_lines.len();
    let omitted = middle.len() - safety_count;

    let mut parts = Vec::new();
    parts.push(first.join("\n"));
    if safety_count > 0 {
        parts.push(format!(
            "[{omitted} lines omitted, {safety_count} safety-relevant lines preserved]"
        ));
        parts.push(safety_lines.join("\n"));
    } else {
        parts.push(format!("[{omitted} lines omitted]"));
    }
    parts.push(last.join("\n"));

    let compressed = parts.join("\n");
    let ct = count_tokens(&compressed);
    if ct >= original_tokens {
        return None;
    }
    Some(compressed)
}

fn fold_repetitive_progress(output: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    let mut pending_kind: Option<ProgressKind> = None;
    let mut pending: Vec<&str> = Vec::new();
    let mut omitted_low_signal = 0usize;

    for line in output.lines() {
        if is_low_signal_progress(line) {
            omitted_low_signal += 1;
            continue;
        }

        let kind = classify_foldable_progress(line);
        if kind.is_some() && kind == pending_kind {
            pending.push(line);
            continue;
        }

        flush_progress_run(&mut out, pending_kind, &pending);
        pending.clear();
        pending_kind = kind;
        if kind.is_some() {
            pending.push(line);
        } else {
            out.push(line.to_string());
        }
    }

    flush_progress_run(&mut out, pending_kind, &pending);
    if omitted_low_signal > 0 {
        out.push(format!(
            "[{omitted_low_signal} low-signal progress lines omitted]"
        ));
    }

    let folded = out.join("\n") + "\n";
    (folded.len() < output.len()).then_some(folded)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProgressKind {
    CargoCompile,
    CargoFresh,
    CargoTestOk,
    PytestPassed,
    NpmProgress,
}

fn classify_foldable_progress(line: &str) -> Option<ProgressKind> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("Compiling ") || trimmed.starts_with("Checking ") {
        return Some(ProgressKind::CargoCompile);
    }
    if trimmed.starts_with("Fresh ")
        || trimmed.starts_with("Downloaded ")
        || trimmed.starts_with("Downloading ")
    {
        return Some(ProgressKind::CargoFresh);
    }
    if trimmed.starts_with("test ") && trimmed.ends_with("... ok") {
        return Some(ProgressKind::CargoTestOk);
    }
    if line.contains(" PASSED [") {
        return Some(ProgressKind::PytestPassed);
    }
    if trimmed.starts_with('[') && trimmed.contains('%') && trimmed.contains('/') {
        return Some(ProgressKind::NpmProgress);
    }
    None
}

/// Pure dot-run lines (pytest/unittest progress) of any length. Lines with any
/// other character (e.g. `....F...`, which encodes a failure) are never folded.
fn is_low_signal_progress(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.bytes().all(|b| b == b'.')
}

fn flush_progress_run(out: &mut Vec<String>, kind: Option<ProgressKind>, lines: &[&str]) {
    let Some(kind) = kind else {
        return;
    };
    if lines.is_empty() {
        return;
    }
    let threshold = match kind {
        ProgressKind::CargoCompile | ProgressKind::PytestPassed | ProgressKind::CargoTestOk => 3,
        ProgressKind::CargoFresh | ProgressKind::NpmProgress => 5,
    };
    if lines.len() < threshold {
        out.extend(lines.iter().map(|line| (*line).to_string()));
        return;
    }

    out.push(format!(
        "[{} {} lines folded]",
        lines.len(),
        match kind {
            ProgressKind::CargoCompile => "cargo compile/check",
            ProgressKind::CargoFresh => "cargo download/fresh",
            ProgressKind::CargoTestOk => "cargo test ok",
            ProgressKind::PytestPassed => "pytest PASSED",
            ProgressKind::NpmProgress => "package-manager progress",
        }
    ));
    out.push(lines.first().unwrap_or(&"").to_string());
    if lines.len() > 2 {
        out.push("…".to_string());
    }
    if lines.len() > 1 {
        out.push(lines.last().unwrap_or(&"").to_string());
    }
}

fn maybe_fold_progress(output: &str, original_tokens: usize) -> Option<String> {
    let folded = fold_repetitive_progress(output)?;
    (count_tokens(&folded) < original_tokens).then_some(folded)
}

/// Detects shell command chains (`&&`, `;`) outside of quotes.
///
/// Chained commands must skip pattern-based compression because each pattern
/// matcher assumes a single command's output shape — applying the first
/// segment's pattern to the whole chain silently drops later segments.
pub(crate) fn is_chained_command(command: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = command.as_bytes();
    let len = bytes.len();
    for i in 0..len {
        match bytes[i] {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            b'&' if !in_single && !in_double && i + 1 < len && bytes[i + 1] == b'&' => {
                return true;
            }
            b';' if !in_single && !in_double => return true,
            _ => {}
        }
    }
    false
}

#[cfg(any(test, feature = "test-support"))]
pub fn compress_if_beneficial_pub(command: &str, output: &str) -> String {
    compress_if_beneficial(command, output)
}

/// Whether shell compression hands a successful run of `command` to the
/// per-command pattern table: the command is not a
/// findings-only linter or a test runner, the output policy does not protect
/// it (built-in rules only), and it is not a chain. What depends on the
/// output (a build tool's errors, the size floors) is not checked. The
/// pattern snapshot tests assert it, so a snapshot never pins a compressor
/// production does not run for its command.
#[cfg(any(test, feature = "test-support"))]
pub fn reaches_pattern_table(command: &str) -> bool {
    !is_findings_only_linter(command)
        && !is_test_runner_command(command)
        && !crate::shell::output_policy::classify(command, &[]).is_protected()
        && !is_chained_command(command)
}

/// Preserve build/test output verbatim, applying only the safety-line-preserving
/// head/tail truncation when it is oversized.
///
/// The proxy funnel uses this when a foreign shell tool produced unmistakable
/// build/test output but supplied no recognizable command — the engine's
/// command-gated verbatim guards cannot fire, yet compiler errors, panics and
/// test summaries must still reach the model intact for a bug-fix task.
pub(crate) fn preserve_verbatim_pub(output: &str) -> String {
    truncate_verbatim(output, count_tokens(output))
}
