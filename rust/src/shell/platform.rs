use std::io::{self, IsTerminal};

/// Sets `LC_CTYPE=C.UTF-8` when no UTF-8 locale is inherited from the parent
/// process. Without this, commands treat bytes >127 as non-printable (C locale),
/// mangling Cyrillic, CJK, emoji, etc.
pub(crate) fn apply_utf8_locale(cmd: &mut std::process::Command) {
    let has_utf8 = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_CTYPE"))
        .or_else(|_| std::env::var("LANG"))
        .is_ok_and(|v| v.to_ascii_lowercase().contains("utf"));

    if !has_utf8 {
        cmd.env("LC_CTYPE", "C.UTF-8");
    }
}

/// Neutralizes inherited startup-file hooks so a non-login/non-interactive
/// `sh -c` / `bash -c` cannot be hijacked into sourcing a profile or rc file.
/// sobre runs the system POSIX shell profile-free and deterministic;
/// a stray inherited `BASH_ENV` (read by bash even for `bash -c`) or `ENV`
/// pointing at e.g. an `exec nu` snippet would otherwise silently replace the
/// shell. Cleared to empty so the expansion names no file. No-op in effect for
/// PowerShell/cmd, which ignore both variables.
pub(crate) fn apply_profile_free_env(cmd: &mut std::process::Command) {
    cmd.env("BASH_ENV", "").env("ENV", "");
}

/// Block SIGPIPE in the calling thread only. CLI runs restore the default
/// SIGPIPE disposition (so `sobre … | head` exits quietly), which would
/// otherwise kill the whole process when this thread writes to a child that
/// already exited. Blocked here, that write fails with EPIPE instead, and the
/// thread-directed signal is discarded when the thread ends. Call it only on
/// a thread dedicated to writing into a child's stdin.
pub(crate) fn block_sigpipe_in_current_thread() {
    #[cfg(unix)]
    // SAFETY: sigemptyset/sigaddset initialise a local set; pthread_sigmask
    // only changes the calling thread's mask.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&raw mut set);
        libc::sigaddset(&raw mut set, libc::SIGPIPE);
        libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut());
    }
}

pub fn decode_output(bytes: &[u8]) -> String {
    match String::from_utf8(bytes.to_vec()) {
        Ok(s) => s,
        Err(_) => {
            #[cfg(windows)]
            {
                decode_windows_output(bytes)
            }
            #[cfg(not(windows))]
            {
                String::from_utf8_lossy(bytes).into_owned()
            }
        }
    }
}
/// Resolve carriage-return (`\r`) progress overwrites in captured output.
///
/// CLI tools like `git`, `cargo`, and `npm` use `\r` to draw in-place progress
/// updates. When stdout is captured (not a TTY), these bytes survive as literal
/// `\r` characters and produce glued-together lines. This function
/// emulates terminal behaviour: for each line, only the content after the last
/// `\r` is kept.
pub fn resolve_carriage_returns(s: &str) -> String {
    if !s.contains('\r') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    for (i, line) in s.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(pos) = line.rfind('\r') {
            out.push_str(&line[pos + 1..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

#[cfg(windows)]
fn decode_windows_output(bytes: &[u8]) -> String {
    use std::os::windows::ffi::OsStringExt;

    let lossy = String::from_utf8_lossy(bytes);
    let replacement_count = lossy.chars().filter(|&c| c == '\u{FFFD}').count();
    if replacement_count == 0 {
        return lossy.into_owned();
    }

    // SAFETY: declares Win32 API symbols that exist in kernel32; signatures
    // match the documented ABI.
    unsafe extern "system" {
        fn GetACP() -> u32;
        fn MultiByteToWideChar(
            cp: u32,
            flags: u32,
            src: *const u8,
            srclen: i32,
            dst: *mut u16,
            dstlen: i32,
        ) -> i32;
    }

    // SAFETY: `GetACP` takes no arguments and only returns the active code
    // page; it cannot fail or cause undefined behaviour.
    let codepage = unsafe { GetACP() };
    // SAFETY: called with a null destination and length 0 to measure the
    // required buffer size; `bytes` is a live slice and every pointer/length
    // argument is valid.
    let wide_len = unsafe {
        MultiByteToWideChar(
            codepage,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    if wide_len <= 0 {
        return lossy.into_owned();
    }
    let mut wide: Vec<u16> = vec![0u16; wide_len as usize];
    // SAFETY: `wide` is sized to the previously measured length and `bytes` is
    // a live slice; the source and destination pointers/lengths are valid and
    // do not overlap.
    unsafe {
        MultiByteToWideChar(
            codepage,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            wide_len,
        );
    }
    std::ffi::OsString::from_wide(&wide)
        .to_string_lossy()
        .into_owned()
}

#[cfg(windows)]
pub(super) fn set_console_utf8() {
    // SAFETY: declares a Win32 API symbol that exists in kernel32; the
    // signature matches the documented ABI.
    unsafe extern "system" {
        fn SetConsoleOutputCP(id: u32) -> i32;
    }
    // SAFETY: `SetConsoleOutputCP` takes a code-page id (65001 = UTF-8) by
    // value; it cannot cause undefined behaviour.
    unsafe {
        SetConsoleOutputCP(65001);
    }
}

/// Returns true if stdin is NOT a terminal (pipe, /dev/null, etc.)
pub fn is_non_interactive() -> bool {
    !io::stdin().is_terminal()
}

/// Returns `true` when `shell_path` points to a PowerShell executable.
pub(crate) fn is_powershell(shell_path: &str) -> bool {
    let name = std::path::Path::new(shell_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    name.contains("powershell") || name.contains("pwsh")
}

/// Windows only: argument that passes one command string to the shell binary.
/// `exe_basename` must already be ASCII-lowercase (e.g. `bash.exe`, `cmd.exe`).
fn windows_shell_flag_for_exe_basename(exe_basename: &str) -> &'static str {
    if exe_basename.contains("powershell") || exe_basename.contains("pwsh") {
        "-Command"
    } else if exe_basename == "cmd.exe" || exe_basename == "cmd" {
        "/C"
    } else {
        "-c"
    }
}

pub fn shell_and_flag() -> (String, String) {
    let shell = detect_shell();
    let flag = if cfg!(windows) {
        let name = std::path::Path::new(&shell)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        windows_shell_flag_for_exe_basename(&name).to_string()
    } else {
        "-c".to_string()
    };
    (shell, flag)
}

/// Returns a short, human-readable shell name (e.g. "bash", "zsh", "powershell", "cmd").
pub fn shell_name() -> String {
    let shell = detect_shell();
    let basename = std::path::Path::new(&shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("sh")
        .to_ascii_lowercase();
    basename
        .strip_suffix(".exe")
        .unwrap_or(&basename)
        .to_string()
}

pub(super) fn detect_shell() -> String {
    if let Ok(shell) = std::env::var("SHELL") {
        let bin = std::path::Path::new(&shell)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("sh");

        if bin == "sobre" {
            return find_real_shell();
        }
        // `$SHELL` is the user's *interactive* shell preference, not the
        // agent's execution shell. Agents emit bash/POSIX command syntax, so an
        // interactive-only shell like Nushell or Fish silently mis-runs it.
        // Honor `$SHELL` only when it is POSIX-compatible; otherwise fall back
        // to a real POSIX shell (deterministic, agent-trained).
        if shell_acceptable_for_exec(&shell) {
            return shell;
        }
        return find_real_shell();
    }

    find_real_shell()
}

/// Whether a `$SHELL` value may be auto-selected as the command-execution shell.
///
/// On Unix this rejects non-POSIX interactive shells (Nushell, Fish, Elvish,
/// xonsh, PowerShell) so sobre never feeds them bash/POSIX syntax. On
/// Windows the intended shells *are* PowerShell/cmd, so any `$SHELL` is honored
/// (the POSIX gate would wrongly reject them).
#[cfg(unix)]
fn shell_acceptable_for_exec(shell: &str) -> bool {
    is_posix_compatible_shell(shell)
}

#[cfg(windows)]
fn shell_acceptable_for_exec(_shell: &str) -> bool {
    true
}

/// `true` if `shell`'s basename is a POSIX-compatible shell that can run
/// agent-authored bash/POSIX commands. Excludes interactive-only shells
/// (`nu`, `fish`, `elvish`, `xonsh`) and non-POSIX shells (`pwsh`,
/// `powershell`, `cmd`) where that syntax fails.
#[cfg(unix)]
fn is_posix_compatible_shell(shell: &str) -> bool {
    let name = std::path::Path::new(shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    matches!(
        name,
        "bash" | "zsh" | "sh" | "dash" | "ash" | "ksh" | "ksh93" | "mksh" | "busybox"
    )
}

#[cfg(unix)]
fn find_real_shell() -> String {
    for shell in &["/bin/zsh", "/bin/bash", "/bin/sh"] {
        if std::path::Path::new(shell).exists() {
            return shell.to_string();
        }
    }
    "/bin/sh".to_string()
}

#[cfg(windows)]
fn find_real_shell() -> String {
    if is_running_in_msys_or_gitbash()
        && let Some(shell) = ["bash.exe", "sh.exe"].into_iter().find_map(path_of)
    {
        return shell;
    }
    if let Ok(pwsh) = which_powershell() {
        return pwsh;
    }
    if let Ok(comspec) = std::env::var("COMSPEC") {
        return comspec;
    }
    "cmd.exe".to_string()
}

#[cfg(windows)]
fn is_running_in_msys_or_gitbash() -> bool {
    std::env::var("MSYSTEM").is_ok() || std::env::var("MINGW_PREFIX").is_ok()
}

#[cfg(windows)]
fn which_powershell() -> Result<String, ()> {
    ["pwsh.exe", "powershell.exe"]
        .into_iter()
        .find_map(path_of)
        .ok_or(())
}

/// Where `program` is on PATH.
#[cfg(windows)]
fn path_of(program: &str) -> Option<String> {
    which::which(program)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Join multiple CLI arguments into a single command string, using quoting
/// conventions appropriate for the detected shell.
///
/// On Unix, this always produces POSIX-compatible quoting.
/// On Windows, the quoting adapts to the actual shell (PowerShell, cmd.exe,
/// or Git Bash / MSYS).
pub fn join_command(args: &[String]) -> String {
    let (_, flag) = shell_and_flag();
    join_command_for(args, &flag)
}

pub(crate) fn join_command_for(args: &[String], shell_flag: &str) -> String {
    match shell_flag {
        "-Command" => join_powershell(args),
        "/C" => join_cmd(args),
        _ => join_posix(args),
    }
}

fn join_posix(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_posix(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn join_powershell(args: &[String]) -> String {
    if args.len() == 1 && args[0].contains(' ') {
        return args[0].clone();
    }
    let quoted: Vec<String> = args.iter().map(|a| quote_powershell(a)).collect();
    format!("& {}", quoted.join(" "))
}

fn join_cmd(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_cmd(a))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_posix(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@,+%^".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn quote_powershell(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@,+%^".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "''"))
}

fn quote_cmd(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_string();
    }
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@,+%^\\".contains(&b))
    {
        return s.to_string();
    }
    format!("\"{}\"", s.replace('"', "\\\""))
}

#[cfg(test)]
mod join_command_tests {
    use super::*;

    #[test]
    fn posix_simple_args() {
        let args: Vec<String> = vec!["git".into(), "status".into()];
        assert_eq!(join_command_for(&args, "-c"), "git status");
    }

    #[test]
    fn posix_path_with_spaces() {
        let args: Vec<String> = vec!["/usr/local/my app/bin".into(), "--help".into()];
        assert_eq!(
            join_command_for(&args, "-c"),
            "'/usr/local/my app/bin' --help"
        );
    }

    #[test]
    fn posix_single_quotes_escaped() {
        let args: Vec<String> = vec!["echo".into(), "it's".into()];
        assert_eq!(join_command_for(&args, "-c"), "echo 'it'\\''s'");
    }

    #[test]
    fn posix_empty_arg() {
        let args: Vec<String> = vec!["cmd".into(), String::new()];
        assert_eq!(join_command_for(&args, "-c"), "cmd ''");
    }

    #[test]
    fn powershell_simple_args() {
        let args: Vec<String> = vec!["npm".into(), "install".into()];
        assert_eq!(join_command_for(&args, "-Command"), "& npm install");
    }

    #[test]
    fn powershell_path_with_spaces() {
        let args: Vec<String> = vec![
            "C:\\Program Files\\nodejs\\npm.cmd".into(),
            "install".into(),
        ];
        assert_eq!(
            join_command_for(&args, "-Command"),
            "& 'C:\\Program Files\\nodejs\\npm.cmd' install"
        );
    }

    #[test]
    fn powershell_single_quotes_escaped() {
        let args: Vec<String> = vec!["echo".into(), "it's done".into()];
        assert_eq!(join_command_for(&args, "-Command"), "& echo 'it''s done'");
    }

    #[test]
    fn cmd_simple_args() {
        let args: Vec<String> = vec!["npm.cmd".into(), "install".into()];
        assert_eq!(join_command_for(&args, "/C"), "npm.cmd install");
    }

    #[test]
    fn cmd_path_with_spaces() {
        let args: Vec<String> = vec![
            "C:\\Program Files\\nodejs\\npm.cmd".into(),
            "install".into(),
        ];
        assert_eq!(
            join_command_for(&args, "/C"),
            "\"C:\\Program Files\\nodejs\\npm.cmd\" install"
        );
    }

    #[test]
    fn cmd_double_quotes_escaped() {
        let args: Vec<String> = vec!["echo".into(), "say \"hello\"".into()];
        assert_eq!(join_command_for(&args, "/C"), "echo \"say \\\"hello\\\"\"");
    }

    #[test]
    fn unknown_flag_uses_posix() {
        let args: Vec<String> = vec!["ls".into(), "-la".into()];
        assert_eq!(join_command_for(&args, "--exec"), "ls -la");
    }

    #[test]
    fn powershell_single_full_command_not_quoted() {
        let args: Vec<String> = vec!["git commit -m \"feat: add feature\"".into()];
        let result = join_command_for(&args, "-Command");
        assert_eq!(result, "git commit -m \"feat: add feature\"");
        assert!(
            !result.starts_with("& '"),
            "must not wrap full command in & '...'"
        );
    }

    #[test]
    fn powershell_single_no_spaces_still_uses_call_operator() {
        let args: Vec<String> = vec!["git".into()];
        assert_eq!(join_command_for(&args, "-Command"), "& git");
    }
}

#[cfg(test)]
mod is_powershell_tests {
    use super::is_powershell;

    #[test]
    fn detects_pwsh_exe() {
        assert!(is_powershell("pwsh.exe"));
    }

    #[test]
    fn detects_powershell_exe() {
        assert!(is_powershell("powershell.exe"));
    }

    #[test]
    fn rejects_cmd() {
        assert!(!is_powershell("cmd.exe"));
    }

    #[test]
    fn rejects_bash() {
        assert!(!is_powershell("/usr/bin/bash"));
    }

    #[test]
    fn case_insensitive() {
        assert!(is_powershell("PWSH.EXE"));
        assert!(is_powershell("PowerShell.exe"));
    }

    #[test]
    fn full_path_with_pwsh() {
        assert!(is_powershell(
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        ));
        assert!(is_powershell("/usr/local/bin/pwsh"));
    }
}

#[cfg(test)]
mod windows_shell_flag_tests {
    use super::windows_shell_flag_for_exe_basename;

    #[test]
    fn cmd_uses_slash_c() {
        assert_eq!(windows_shell_flag_for_exe_basename("cmd.exe"), "/C");
        assert_eq!(windows_shell_flag_for_exe_basename("cmd"), "/C");
    }

    #[test]
    fn powershell_uses_command() {
        assert_eq!(
            windows_shell_flag_for_exe_basename("powershell.exe"),
            "-Command"
        );
        assert_eq!(windows_shell_flag_for_exe_basename("pwsh.exe"), "-Command");
    }

    #[test]
    fn posix_shells_use_dash_c() {
        assert_eq!(windows_shell_flag_for_exe_basename("bash.exe"), "-c");
        assert_eq!(windows_shell_flag_for_exe_basename("bash"), "-c");
        assert_eq!(windows_shell_flag_for_exe_basename("sh.exe"), "-c");
        assert_eq!(windows_shell_flag_for_exe_basename("zsh.exe"), "-c");
        assert_eq!(windows_shell_flag_for_exe_basename("fish.exe"), "-c");
    }
}

#[cfg(test)]
mod platform_tests {

    #[test]
    fn join_command_preserves_structure() {
        let args = vec![
            "git".to_string(),
            "commit".to_string(),
            "-m".to_string(),
            "my message".to_string(),
        ];
        let joined = super::join_command(&args);
        assert!(joined.contains("git"));
        assert!(joined.contains("commit"));
        assert!(joined.contains("my message") || joined.contains("'my message'"));
    }

    #[test]
    fn quote_posix_handles_em_dash() {
        let result = super::quote_posix("closing — see #407");
        assert!(
            result.starts_with('\''),
            "em-dash args must be single-quoted: {result}"
        );
    }

    #[test]
    fn quote_posix_handles_nested_single_quotes() {
        let result = super::quote_posix("it's a test");
        assert!(
            result.contains("\\'"),
            "single quotes must be escaped: {result}"
        );
    }

    #[test]
    fn quote_posix_safe_chars_unquoted() {
        let result = super::quote_posix("simple_word");
        assert_eq!(result, "simple_word");
    }

    #[test]
    fn quote_posix_empty_string() {
        let result = super::quote_posix("");
        assert_eq!(result, "''");
    }

    #[test]
    fn quote_posix_dollar_expansion_protected() {
        let result = super::quote_posix("$HOME/test");
        assert!(
            result.starts_with('\''),
            "dollar signs must be single-quoted: {result}"
        );
    }

    #[test]
    fn quote_posix_backtick_protected() {
        let result = super::quote_posix("echo `date`");
        assert!(
            result.starts_with('\''),
            "backticks must be single-quoted: {result}"
        );
    }

    #[test]
    fn quote_posix_double_quotes_protected() {
        let result = super::quote_posix(r#"he said "hello""#);
        assert!(
            result.starts_with('\''),
            "double quotes must be single-quoted: {result}"
        );
    }

    // A non-interactive `bash -c` sources $BASH_ENV. sobre must run
    // profile-free, so `apply_profile_free_env` has to neutralize an inherited
    // BASH_ENV before it can pull in a contaminating startup file.
    #[cfg(unix)]
    #[test]
    fn profile_free_env_blocks_bash_env_contamination() {
        let Some(bash) = ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
        else {
            return; // no bash on this host → nothing to guard against
        };

        let startup = std::env::temp_dir().join(format!(
            "sobre_bashenv_{}_{}.sh",
            std::process::id(),
            "guard"
        ));
        std::fs::write(&startup, "echo CONTAMINATED\n").expect("write startup file");

        let mut cmd = std::process::Command::new(bash);
        cmd.arg("-c").arg("echo clean").env("BASH_ENV", &startup);
        super::apply_profile_free_env(&mut cmd);
        let out = cmd.output().expect("run bash");
        let stdout = String::from_utf8_lossy(&out.stdout);

        let _ = std::fs::remove_file(&startup);

        assert!(stdout.contains("clean"), "command output missing: {stdout}");
        assert!(
            !stdout.contains("CONTAMINATED"),
            "apply_profile_free_env must neutralize BASH_ENV, got: {stdout}"
        );
    }
}

#[cfg(all(test, unix))]
mod posix_shell_gate_tests {
    use super::{detect_shell, is_posix_compatible_shell};

    #[test]
    fn accepts_posix_shells() {
        for s in [
            "/bin/bash",
            "/bin/zsh",
            "/bin/sh",
            "/usr/bin/dash",
            "/bin/ash",
            "/usr/bin/ksh",
            "/usr/bin/mksh",
            "bash",
            "zsh",
        ] {
            assert!(is_posix_compatible_shell(s), "{s} must be POSIX-compatible");
        }
    }

    #[test]
    fn rejects_interactive_and_nonposix_shells() {
        // Agent bash/POSIX syntax fails in these — never auto-select them.
        for s in [
            "/usr/bin/nu",
            "/opt/homebrew/bin/nu",
            "/usr/bin/fish",
            "/usr/local/bin/elvish",
            "/usr/bin/xonsh",
            "/usr/bin/pwsh",
            "powershell.exe",
            "cmd.exe",
        ] {
            assert!(
                !is_posix_compatible_shell(s),
                "{s} must be rejected by the POSIX gate"
            );
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn detect_shell_falls_back_when_shell_is_nushell() {
        // Serialize env mutation (set_var/remove_var are process-global).
        let _lock = crate::core::data_dir::test_env_lock();
        let saved_shell = std::env::var_os("SHELL");
        crate::test_env::set_var("SHELL", "/usr/bin/nu");
        let resolved = detect_shell();
        assert!(
            is_posix_compatible_shell(&resolved),
            "a non-POSIX $SHELL (nu) must resolve to a POSIX shell, got {resolved}"
        );
        assert!(
            !resolved.ends_with("/nu") && resolved != "/usr/bin/nu",
            "must not run agent commands in Nushell, got {resolved}"
        );

        // A POSIX $SHELL is honored verbatim.
        crate::test_env::set_var("SHELL", "/bin/sh");
        assert_eq!(detect_shell(), "/bin/sh");

        match saved_shell {
            Some(v) => crate::test_env::set_var("SHELL", v),
            None => crate::test_env::remove_var("SHELL"),
        }
    }
}

#[cfg(test)]
mod carriage_return_tests {
    use super::resolve_carriage_returns;

    #[test]
    fn no_cr_passthrough() {
        assert_eq!(resolve_carriage_returns("hello\nworld\n"), "hello\nworld\n");
    }

    #[test]
    fn progress_overwrite_keeps_last_segment() {
        assert_eq!(resolve_carriage_returns("first\rsecond"), "second");
    }

    #[test]
    fn git_rebase_progress() {
        let input = "Rebasing (1/1)\rSuccessfully rebased and updated refs/heads/feat.";
        assert_eq!(
            resolve_carriage_returns(input),
            "Successfully rebased and updated refs/heads/feat."
        );
    }

    #[test]
    fn crlf_line_endings_stripped() {
        assert_eq!(
            resolve_carriage_returns("line1\r\nline2\r\n"),
            "line1\nline2\n"
        );
    }

    #[test]
    fn multiple_cr_on_one_line() {
        assert_eq!(resolve_carriage_returns("a\rb\rc"), "c");
    }

    #[test]
    fn mixed_lines() {
        let input = "clean line\nprogress\rdone\nalso clean\n";
        assert_eq!(
            resolve_carriage_returns(input),
            "clean line\ndone\nalso clean\n"
        );
    }

    #[test]
    fn trailing_cr_stripped_as_crlf() {
        assert_eq!(resolve_carriage_returns("visible\r"), "visible");
    }

    #[test]
    fn empty_string_passthrough() {
        assert_eq!(resolve_carriage_returns(""), "");
    }
}
