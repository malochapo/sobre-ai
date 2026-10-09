use std::io::{self, Read};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};

use crate::core::config;

/// The most stderr `sobre -c` keeps.
const STDERR_LIMIT: usize = 512 * 1024;

/// How long the readers get to reach EOF once the command has exited.
const READER_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// What a reader kept of one pipe, and how many bytes it read past its cap.
#[derive(Default)]
struct Kept {
    bytes: Vec<u8>,
    dropped: u64,
}

/// Reads `pipe` to its end on a thread: the first `cap` bytes are kept, the
/// rest is read and counted, never left in the pipe (a full pipe would stop
/// the command). The receiver fires at EOF.
fn drain<R: Read + Send + 'static>(
    pipe: Option<R>,
    cap: usize,
) -> (Arc<Mutex<Kept>>, mpsc::Receiver<()>) {
    let shared = Arc::new(Mutex::new(Kept::default()));
    let (done_tx, done_rx) = mpsc::channel();
    let writer = Arc::clone(&shared);
    std::thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut kept = writer
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let take = cap.saturating_sub(kept.bytes.len()).min(n);
                        kept.bytes.extend_from_slice(&chunk[..take]);
                        kept.dropped += (n - take) as u64;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        }
        let _ = done_tx.send(());
    });
    (shared, done_rx)
}

/// Waits for a `sobre -c` command to exit and collects its output. The
/// command runs as it would without sobre: no timeout of sobre's own (the
/// host has one), and output past `max_bytes` (stdout) or [`STDERR_LIMIT`]
/// is read and dropped rather than stopping it; the result says how much.
/// A background process the command left holding the pipe does not hold
/// this up past [`READER_GRACE`]; the result says so.
pub(super) fn wait_collecting(mut child: Child, max_bytes: usize) -> Output {
    let (out, out_done) = drain(child.stdout.take(), max_bytes);
    let (err, err_done) = drain(child.stderr.take(), STDERR_LIMIT);
    let status = child.wait().unwrap_or_else(|_| synthetic_failure_status());

    let deadline = std::time::Instant::now() + READER_GRACE;
    let wait = |done: &mpsc::Receiver<()>| {
        done.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .is_ok()
    };
    let complete = wait(&out_done) & wait(&err_done);
    let take = |kept: &Arc<Mutex<Kept>>| {
        let mut kept = kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (std::mem::take(&mut kept.bytes), kept.dropped)
    };
    let (mut stdout, out_dropped) = take(&out);
    let (mut stderr, err_dropped) = take(&err);

    if out_dropped > 0 {
        stdout.extend_from_slice(
            format!(
                "\n[sobre: {out_dropped} more bytes of output not kept (past {} MB); \
                 the command ran to the end]\n",
                max_bytes / (1024 * 1024)
            )
            .as_bytes(),
        );
    }
    if err_dropped > 0 {
        stderr.extend_from_slice(
            format!(
                "\n[sobre: {err_dropped} more bytes of stderr not kept (past {} KB)]\n",
                STDERR_LIMIT / 1024
            )
            .as_bytes(),
        );
    }
    if !complete {
        stdout.extend_from_slice(
            b"\n[sobre: a background process the command started still holds its output; \
              what it writes from now on is not shown]\n",
        );
    }

    Output {
        status,
        stdout,
        stderr,
    }
}

/// A synthetic failed `ExitStatus`, used only when `Child::wait()` itself
/// errors (e.g. the process was already reaped by another waiter) and there
/// is no real status to report. The previous fallback shelled out to
/// `Command::new("false").status()` to manufacture one, which panicked via
/// `.expect()` wherever no `false` binary exists on `PATH` — Windows, and
/// minimal/scratch containers. `ExitStatusExt::from_raw` builds the status
/// value directly, with no subprocess involved, so it can't fail.
#[cfg(unix)]
fn synthetic_failure_status() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    // Raw wait(2) status encoding: low 7 bits 0 signals a normal exit
    // (`WIFEXITED`), the next byte up is the exit code (`WEXITSTATUS`) — so
    // `1 << 8` decodes as "exited normally with code 1".
    std::process::ExitStatus::from_raw(1 << 8)
}

#[cfg(not(unix))]
fn synthetic_failure_status() -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(1)
}

#[cfg(test)]
mod nested_sobre_exec_tests {
    #[test]
    fn collapses_single_nested_c() {
        assert_eq!(
            super::collapse_nested_sobre_exec("sobre -c 'git status'").as_deref(),
            Some("git status")
        );
    }

    #[test]
    fn collapses_repeated_nested_c() {
        assert_eq!(
            super::collapse_nested_sobre_exec("sobre -c 'sobre -c \"git status\"'").as_deref(),
            Some("git status")
        );
    }

    #[test]
    fn preserves_inner_shell_quoting() {
        assert_eq!(
            super::collapse_nested_sobre_exec("sobre -c \"git commit -m 'hello world'\"")
                .as_deref(),
            Some("git commit -m 'hello world'")
        );
        assert_eq!(
            super::collapse_nested_sobre_exec("sobre -c git commit -m 'hello world'").as_deref(),
            Some("git commit -m 'hello world'")
        );
    }

    #[test]
    fn collapses_wrapper_given_by_path() {
        assert_eq!(
            super::collapse_nested_sobre_exec("/usr/local/bin/sobre -c 'git status'").as_deref(),
            Some("git status")
        );
    }

    #[test]
    fn leaves_non_wrappers_alone() {
        assert!(super::collapse_nested_sobre_exec("git status").is_none());
    }

    #[test]
    fn wrapped_nested_wrapper_still_owns_one_compression_pass() {
        let _lock = crate::core::data_dir::test_env_lock();
        crate::test_env::set_var(super::super::reentry::WRAP_MARKER, "1");

        assert!(super::should_delegate_wrapped_to_shell_default(false));
        assert!(
            !super::should_delegate_wrapped_to_shell_default(true),
            "collapsed nested wrappers must not fall through to raw shell-default path"
        );

        crate::test_env::remove_var(super::super::reentry::WRAP_MARKER);
    }
}

const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024; // 8 MB
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(2);
const HEAVY_MAX_BYTES: usize = 32 * 1024 * 1024; // 32 MB
const HEAVY_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(10);

/// The most stdout `sobre -c` keeps of `command`.
pub(super) fn exec_byte_cap(command: &str) -> usize {
    if is_heavy_command(command) {
        HEAVY_MAX_BYTES
    } else {
        DEFAULT_MAX_BYTES
    }
}

/// [`shell_timeout_with_override`] with no per-call override.
#[cfg(test)]
fn shell_timeout(command: &str) -> std::time::Duration {
    shell_timeout_with_override(command, None)
}

/// Hard ceiling for a per-call `timeout_ms` override: generous enough for any
/// legitimate build/release job, low enough that a typo'd value cannot wedge
/// the executor for days.
const MAX_CALL_TIMEOUT_MS: u64 = 3_600_000; // 1 hour

/// The timeout `sobre_shell` grants a command (`sobre -c` sets none: the
/// host times the command it wrapped). Heavy builds/tests (cargo
/// install/nextest/build, npm ci, git commit/push, …) get the long ceiling
/// instead of the 2-minute default:
///
/// - per-call `timeout_ms` (clamped to [`MAX_CALL_TIMEOUT_MS`], zero ignored);
/// - else a normal command → config `shell_timeout_secs`, or [`DEFAULT_TIMEOUT`];
/// - else a heavy command → the larger of `shell_timeout_secs` and
///   [`HEAVY_TIMEOUT`].
#[must_use]
pub(crate) fn shell_timeout_with_override(
    command: &str,
    override_ms: Option<u64>,
) -> std::time::Duration {
    if let Some(ms) = override_ms.filter(|n| *n > 0) {
        return std::time::Duration::from_millis(ms.min(MAX_CALL_TIMEOUT_MS));
    }
    let configured = config::Config::load()
        .shell_timeout_secs
        .map(std::time::Duration::from_secs);
    if is_heavy_command(command) {
        configured.map_or(HEAVY_TIMEOUT, |t| t.max(HEAVY_TIMEOUT))
    } else {
        configured.unwrap_or(DEFAULT_TIMEOUT)
    }
}

/// Detect shell loop constructs that are intentionally long-running.
/// Matches `while true`, `while :`, `while sleep`, `for … in`, and similar
/// patterns used for poll loops and background monitors.
fn is_loop_command(lower: &str) -> bool {
    let trimmed = lower.trim_start();
    trimmed.starts_with("while ")
        || trimmed.starts_with("until ")
        || (trimmed.starts_with("for ") && trimmed.contains(" in "))
}

fn is_heavy_command(command: &str) -> bool {
    let cmd = command.trim();
    let lower = cmd.to_lowercase();
    // Shell loops (`while true; do ... done`, `for i in ...`) are
    // intentionally long-running poll/monitor scripts. Killing them at 120s
    // breaks CI watchers and background monitors.
    if is_loop_command(&lower) {
        return true;
    }
    static HEAVY_PREFIXES: &[&str] = &[
        "cargo build",
        "cargo test",
        "cargo nextest",
        "cargo clippy",
        "cargo check",
        "cargo install",
        "cargo bench",
        "npm run build",
        "npm install",
        "npm ci",
        "pnpm install",
        "pnpm build",
        "yarn install",
        "yarn build",
        "bun install",
        "make",
        "cmake",
        "bazel build",
        "bazel test",
        "gradle build",
        "gradle test",
        "mvn package",
        "mvn install",
        "mvn test",
        "go build",
        "go test",
        "dotnet build",
        "dotnet test",
        "swift build",
        "swift test",
        "flutter build",
        "docker build",
        "docker compose build",
        "pip install",
        "poetry install",
        "uv sync",
        "bundle install",
        "mix compile",
        // Git commands that fire build/test hooks: a `pre-commit` running
        // `cargo clippy` or a `pre-push` running a full preflight can take
        // minutes, far past the 2-minute default. Killing git mid-hook leaves
        // the working tree staged-but-uncommitted and the push half-done, so
        // these get the heavy ceiling. `git status`/`log`/`diff` stay default
        // because the prefix is the full `git <verb>`.
        "git commit",
        "git push",
        // Scripts and test runners that scan repos, run audits, or invoke
        // subprocesses with their own timeouts. Killing them mid-run wastes
        // agent tokens on retries.
        "python3 ",
        "python ",
        "pytest",
        "bash scripts/",
        "sh scripts/",
        "./scripts/",
        // `timeout N cmd` wraps an intentionally long command — respect it.
        "timeout ",
        // Task runners wrap builds/test gates; the underlying job is what's
        // heavy, so the wrapper gets the same ceiling. A fast subcommand
        // (`mise ls`) merely inherits a longer kill deadline — harmless.
        "mise ",
        "just ",
    ];

    let matches_heavy = |s: &str| HEAVY_PREFIXES.iter().any(|p| s.starts_with(p));

    if matches_heavy(&lower) {
        return true;
    }

    // Agents often prefix commands with `cd /path && ...` or `cd /path;`.
    // Extract the final segment after the last `&&` or `;` and check that too.
    let final_cmd = lower
        .rsplit_once("&&")
        .or_else(|| lower.rsplit_once(';'))
        .map_or("", |(_, rhs)| rhs.trim());

    !final_cmd.is_empty() && matches_heavy(final_cmd)
}

/// True when this process's stdout is a **regular file** — i.e. the caller
/// redirected output to a file (`cmd > out`, `cmd >> out`).
///
/// Output captured to a file is consumed as *data*, so it must stay byte-faithful:
/// compression would silently drop/abbreviate lines and corrupt the file
/// (e.g. `git status --short > files.txt` losing entries). Pipes (agent capture)
/// and TTYs are NOT regular files and return `false`, so they keep their normal
/// behavior — this only ever *adds* a verbatim guarantee, never removes one.
///
/// Uses only `std`: it wraps the existing stdout descriptor in a `ManuallyDrop`
/// `File` purely to read its metadata (`fstat` on Unix, `GetFileInformation` on
/// Windows) without ever closing the real stdout.
fn stdout_is_regular_file() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::io::{AsRawFd, FromRawFd};
        let fd = io::stdout().as_raw_fd();
        // SAFETY: fd 1 stays valid for the whole process. `ManuallyDrop` prevents
        // the wrapper's `Drop` from closing stdout; we only read metadata.
        let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
        file.metadata().is_ok_and(|m| m.is_file())
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        let handle = io::stdout().as_raw_handle();
        // SAFETY: the stdout handle stays valid for the whole process.
        // `ManuallyDrop` prevents the wrapper's `Drop` from closing it.
        let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_handle(handle) });
        file.metadata().is_ok_and(|m| m.is_file())
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// How [`exec`] treats the command's output.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// `sobre -c`: compressed, unless the command is passthrough or writes
    /// to a file.
    Compress,
    /// `sobre raw`: the exact output.
    Raw,
}

pub fn exec(command: &str, mode: ExecMode) -> i32 {
    let mut collapsed_nested = false;
    let collapsed;
    let command = if let Some(c) = collapse_nested_sobre_exec(command) {
        collapsed_nested = true;
        collapsed = c;
        collapsed.as_str()
    } else {
        command
    };

    let (shell, shell_flag) = super::platform::shell_and_flag();

    if should_delegate_wrapped_to_shell_default(collapsed_nested) {
        return exec_shell_default(command, &shell, &shell_flag);
    }

    if mode == ExecMode::Raw {
        return exec_inherit_tracked(command, &shell, &shell_flag);
    }

    let cfg = config::Config::load();
    let policy = super::output_policy::classify(command, &cfg.passthrough_commands);

    // Passthrough: never compressed. A verbatim command still takes the
    // buffered path, where `compress_if_beneficial` only size-caps it.
    if policy == super::output_policy::OutputPolicy::Passthrough {
        return exec_inherit_tracked(command, &shell, &shell_flag);
    }

    // Compression must never alter bytes destined for a file: a redirect
    // (`cmd > out`, `cmd >> out`) means the output is captured as data, not read by
    // a human or agent. Writing the compressed digest there would silently
    // drop/abbreviate lines and corrupt the file (e.g. contradictory `git diff`
    // dumps). Pass redirected-to-file output through verbatim; pipes (agent
    // capture) and TTYs keep compressing. This is the single choke point, so it
    // holds for every caller (hook, direct CLI).
    if stdout_is_regular_file() {
        return exec_inherit_tracked(command, &shell, &shell_flag);
    }

    super::pipeline::exec_buffered(command, &shell, &shell_flag)
}

fn collapse_nested_sobre_exec(command: &str) -> Option<String> {
    let mut current = command.trim().to_string();
    let mut changed = false;

    while let Some(next) = strip_one_sobre_exec(&current) {
        if next == current {
            break;
        }
        current = next;
        changed = true;
    }

    changed.then_some(current)
}

fn should_delegate_wrapped_to_shell_default(collapsed_nested: bool) -> bool {
    // After collapsing `sobre -c "sobre -c ..."` the current process is the
    // one compression pass that would otherwise be owned by the shell default.
    // Delegating again would drop back to raw execution or re-enter the hook.
    super::reentry::is_wrapped() && !collapsed_nested
}

fn strip_one_sobre_exec(command: &str) -> Option<String> {
    let words = split_simple_shell_words(command)?;
    if words.len() < 3 || !is_sobre_bin(&words[0].value) {
        return None;
    }
    if words[1].value != "-c" {
        return None;
    }
    if words[2..].iter().any(|w| {
        matches!(
            w.value.as_str(),
            "|" | "||" | "&" | "&&" | ";" | "<" | ">" | ">>"
        )
    }) {
        return None;
    }
    if words.len() == 3 {
        Some(words[2].value.trim().to_string())
    } else {
        Some(command[words[2].start..].trim().to_string())
    }
}

fn is_sobre_bin(word: &str) -> bool {
    std::path::Path::new(word)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "sobre" || name == "sobre.exe")
}

struct SimpleShellWord {
    value: String,
    start: usize,
}

fn split_simple_shell_words(command: &str) -> Option<Vec<SimpleShellWord>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut current_start: Option<usize> = None;
    let mut chars = command.char_indices().peekable();
    let mut quote: Option<char> = None;

    while let Some((idx, ch)) = chars.next() {
        match quote {
            Some('\'') if ch == '\'' => quote = None,
            Some('"') if ch == '"' => quote = None,
            None if ch == '\'' || ch == '"' => {
                current_start.get_or_insert(idx);
                quote = Some(ch);
            }
            Some('"') | None if ch == '\\' => {
                current_start.get_or_insert(idx);
                if let Some((_, next)) = chars.next() {
                    current.push(next);
                }
            }
            None if ch.is_whitespace() => {
                if let Some(start) = current_start.take() {
                    words.push(SimpleShellWord {
                        value: std::mem::take(&mut current),
                        start,
                    });
                }
            }
            Some(_) | None => {
                current_start.get_or_insert(idx);
                current.push(ch);
            }
        }
    }

    if quote.is_some() {
        return None;
    }
    if let Some(start) = current_start {
        words.push(SimpleShellWord {
            value: current,
            start,
        });
    }
    (!words.is_empty()).then_some(words)
}

fn exec_inherit(command: &str, shell: &str, shell_flag: &str) -> i32 {
    let mut cmd = Command::new(shell);
    cmd.arg(shell_flag)
        .arg(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    super::reentry::mark_child(&mut cmd);
    super::platform::apply_utf8_locale(&mut cmd);
    let status = cmd.status();

    match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            tracing::error!("sobre: failed to execute: {e}");
            127
        }
    }
}

fn exec_shell_default(command: &str, shell: &str, shell_flag: &str) -> i32 {
    let mut cmd = Command::new(shell);
    cmd.arg(shell_flag)
        .arg(command)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    super::reentry::clear_shell_default_markers(&mut cmd);
    super::platform::apply_utf8_locale(&mut cmd);
    let status = cmd.status();

    match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("sobre: failed to execute '{command}': {e}");
            127
        }
    }
}

fn exec_inherit_tracked(command: &str, shell: &str, shell_flag: &str) -> i32 {
    let code = exec_inherit(command, shell, shell_flag);
    crate::core::tool_lifecycle::record_shell_command(command, 0, 0);
    code
}

/// Label inserted between stdout and stderr of a FAILED command so the agent can
/// attribute the error to the right stream instead of guessing — and never has to
/// re-run the command raw just to locate the failure.
pub(crate) const STDERR_LABEL: &str = "--- stderr ---";

/// Join captured stdout and stderr for display/recovery. On failure (non-zero
/// exit) with both streams present, a labeled delimiter separates them; success
/// output keeps the plain `stdout\nstderr` shape (determinism).
pub(crate) fn combine_streams(stdout: &str, stderr: &str, exit_code: i32) -> String {
    match (stdout.is_empty(), stderr.is_empty()) {
        (_, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) if exit_code != 0 => format!("{stdout}\n{STDERR_LABEL}\n{stderr}"),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

// Buffered command execution and output transformation live in `pipeline`.

#[cfg(test)]
mod exec_tests {
    #[test]
    fn combine_streams_labels_stderr_on_failure() {
        let out = super::combine_streams("build ok", "linker: undefined symbol", 1);
        assert_eq!(
            out,
            format!(
                "build ok\n{}\nlinker: undefined symbol",
                super::STDERR_LABEL
            )
        );
    }

    #[test]
    fn combine_streams_plain_join_on_success() {
        let out = super::combine_streams("step 1", "warning: noop", 0);
        assert_eq!(out, "step 1\nwarning: noop");
        assert!(!out.contains(super::STDERR_LABEL));
    }

    #[test]
    fn combine_streams_single_stream_is_unchanged() {
        assert_eq!(super::combine_streams("only stdout", "", 1), "only stdout");
        assert_eq!(super::combine_streams("", "only stderr", 1), "only stderr");
    }

    #[cfg(unix)]
    fn piped(args: &[&str]) -> std::process::Child {
        std::process::Command::new("sh")
            .args(["-c"])
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn wait_collecting_captures_output() {
        let output = super::wait_collecting(piped(&["echo hello"]), 1024);
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
        assert!(output.status.success());
    }

    /// Output past the cap is read and dropped, labelled; the command
    /// is never stopped for it.
    #[cfg(unix)]
    #[test]
    fn output_past_the_cap_is_dropped_and_the_command_runs_to_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("finished");
        let script = format!(
            "yes aaaa | head -25000; yes bbbbbbbbbb | head -200000 >&2; touch {}",
            marker.display()
        );
        let output = super::wait_collecting(piped(&[&script]), 1024);
        assert!(output.status.success());
        assert!(marker.exists(), "the command must run to its end");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("more bytes of output not kept"),
            "{stdout:.200}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("more bytes of stderr not kept"),
            "{stderr:.200}"
        );
    }

    #[test]
    fn synthetic_failure_status_is_a_failure_without_spawning_anything() {
        let status = super::synthetic_failure_status();
        assert!(!status.success());
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.code(), Some(1));
            assert_eq!(status.signal(), None);
        }
    }

    /// A background process left holding the pipe does not hold the result
    /// back past the grace period.
    #[cfg(unix)]
    #[test]
    fn a_background_process_holding_the_pipe_does_not_wedge_the_wait() {
        let start = std::time::Instant::now();
        let output = super::wait_collecting(piped(&["sleep 30 & echo hi"]), 1024);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "took {:?}",
            start.elapsed()
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hi"), "{stdout}");
        assert!(stdout.contains("still holds its output"), "{stdout}");
    }

    #[test]
    fn heavy_commands_get_higher_byte_limits() {
        // exec_byte_cap owns the byte ceiling; timeout resolution is covered by
        // `shell_timeout_resolves_heavy_normal_and_config_overrides` (which is
        // config-isolated, so these stay deterministic regardless of the
        // operator's config.toml).
        for cmd in [
            "cargo build --release",
            "cargo test --lib",
            "cargo nextest run",
            "npm run build",
            "docker build -t myapp .",
            // Git verbs that fire build/test hooks (pre-commit clippy, pre-push
            // preflight) must not be killed at the default ceiling.
            "git commit --amend --no-edit",
            "git push -u origin HEAD",
            // Agents prefix with `cd /path && ...` — heavy detection must
            // look through it to avoid 120s timeout on builds.
            "cd /some/path && cargo test --lib",
            "cd /foo/bar && cargo build --release",
            "cd /workspace; npm ci",
        ] {
            let bytes = super::exec_byte_cap(cmd);
            assert_eq!(bytes, super::HEAVY_MAX_BYTES, "heavy byte limit for {cmd}");
        }
    }

    #[test]
    fn normal_commands_get_default_byte_limits() {
        // Read-only git verbs stay on the default ceiling — only `commit`/`push`
        // (which fire the cargo-heavy hooks) are promoted.
        for cmd in ["echo hello", "git status", "git log --oneline -5"] {
            let bytes = super::exec_byte_cap(cmd);
            assert_eq!(
                bytes,
                super::DEFAULT_MAX_BYTES,
                "default byte limit for {cmd}"
            );
        }
    }

    #[test]
    fn shell_timeout_resolves_heavy_normal_and_config_overrides() {
        // Private config dir so the operator's config.toml cannot leak in.
        let iso = crate::core::data_dir::isolated_data_dir();

        // Heavy builds/tests and hook-firing git verbs get the heavy ceiling;
        // read-only verbs stay on the default. Preserves the promotion.
        assert_eq!(
            super::shell_timeout("cargo install --path ."),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("cargo nextest run"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("git commit -m 'wip'"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("git push origin main"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(super::shell_timeout("git status"), super::DEFAULT_TIMEOUT);
        assert_eq!(super::shell_timeout("ls -la"), super::DEFAULT_TIMEOUT);
        // `cd ... && heavy` must resolve to HEAVY so agents don't get killed at 120s.
        assert_eq!(
            super::shell_timeout("cd /some/project && cargo test --lib"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("cd /workspace && cargo build --release"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("cd /app; npm ci"),
            super::HEAVY_TIMEOUT
        );

        // The configured timeout sets normal commands; heavy ones never drop
        // below the heavy floor but follow a larger configured value.
        std::fs::write(iso.path().join("config.toml"), "shell_timeout_secs = 30\n").unwrap();
        assert_eq!(super::shell_timeout("cargo build"), super::HEAVY_TIMEOUT);
        assert_eq!(
            super::shell_timeout("git status"),
            std::time::Duration::from_secs(30)
        );
        std::fs::write(iso.path().join("config.toml"), "shell_timeout_secs = 900\n").unwrap();
        assert_eq!(
            super::shell_timeout("cargo build"),
            std::time::Duration::from_mins(15)
        );
    }

    // Task runners (mise/just) wrap builds and test gates that routinely run
    // past the 2-minute default; killing them mid-run loses the whole job.
    // They get the heavy ceiling like the underlying build tools they invoke.
    #[test]
    fn task_runners_get_heavy_ceiling() {
        let _lock = crate::core::data_dir::test_env_lock();

        assert_eq!(super::shell_timeout("mise gate"), super::HEAVY_TIMEOUT);
        assert_eq!(super::shell_timeout("mise run gate"), super::HEAVY_TIMEOUT);
        assert_eq!(super::shell_timeout("just build"), super::HEAVY_TIMEOUT);
    }

    #[test]
    fn scripts_and_timeout_get_heavy_ceiling() {
        let _lock = crate::core::data_dir::test_env_lock();

        assert_eq!(
            super::shell_timeout("python3 scripts/audit.py"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("python scripts/gate.py --root ."),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(super::shell_timeout("pytest tests/"), super::HEAVY_TIMEOUT);
        assert_eq!(
            super::shell_timeout("bash scripts/loc-gate.sh"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("sh scripts/run.sh"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("./scripts/deploy.sh"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("timeout 300 python3 audit.py"),
            super::HEAVY_TIMEOUT
        );

        assert_eq!(
            super::shell_timeout("cd /repo && python3 gate.py"),
            super::HEAVY_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout("cd /repo && timeout 600 make"),
            super::HEAVY_TIMEOUT
        );
    }

    // Per-call `timeout_ms` (sobre_shell tool arg): explicit caller intent beats
    // the built-in tiers in both directions, absurd values clamp to the 1h
    // ceiling, and zero is ignored.
    #[test]
    fn per_call_timeout_override_resolves_and_clamps() {
        let _lock = crate::core::data_dir::test_env_lock();

        assert_eq!(
            super::shell_timeout_with_override("git status", Some(300_000)),
            std::time::Duration::from_mins(5)
        );
        assert_eq!(
            super::shell_timeout_with_override("cargo build", Some(30_000)),
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            super::shell_timeout_with_override("git status", Some(999_000_000)),
            std::time::Duration::from_millis(super::MAX_CALL_TIMEOUT_MS)
        );
        assert_eq!(
            super::shell_timeout_with_override("git status", Some(0)),
            super::DEFAULT_TIMEOUT
        );
        assert_eq!(
            super::shell_timeout_with_override("git status", None),
            super::DEFAULT_TIMEOUT
        );
    }
}
