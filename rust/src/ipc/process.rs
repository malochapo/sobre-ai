//! Process control through the OS itself (`kill`, `pgrep`, `ps`, `tasklist`,
//! `OpenProcess`). Not `sysinfo`: it is a heavy dependency for what the
//! uninstall and stop paths need, a liveness check, a signal and a lookup by
//! name.

use anyhow::Result;

/// Run a command with a hard timeout, capturing its output.
///
/// Returns `Some(output)` if the child exits within `timeout`, or `None` if it
/// had to be killed (timed out) or could not be spawned. This is the safe way
/// to invoke `launchctl`, which must never be able to hang a `sobre`
/// command.
///
/// Note: intended for commands with small output. The child's stdout/stderr are
/// piped; a process that writes more than the pipe buffer (~64 KiB) without
/// exiting could block. All current callers emit at most a few lines.
#[cfg(any(target_os = "macos", test))]
pub fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> Option<std::process::Output> {
    use std::process::Stdio;
    use std::time::Instant;

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            // Process exited: pipes are at EOF, so reading output won't block.
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
}

/// Spawn a long-lived background process (the proxy) detached from the
/// current process so it survives the parent's exit.
///
/// On Unix a child simply outlives its parent, so this is a plain spawn. On
/// Windows the child inherits the parent's console and — crucially — its Job
/// object. AI clients (OpenCode, Codex, Claude Code) run MCP servers inside
/// kill-on-close Jobs; without detachment the auto-started proxy dies the
/// moment the client recycles its MCP process, which users observe as
/// "Cannot connect to API: The socket connection was closed unexpectedly"
/// plus repeated proxy cold-starts.
///
/// Strategy on Windows:
///  1. `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB`
///     — fully detached, escapes the parent's Job.
///  2. If the Job denies breakaway, `CreateProcess` fails with
///     `ERROR_ACCESS_DENIED`; retry without the breakaway flag (still
///     console-detached, which covers non-Job parents).
pub fn spawn_detached(cmd: &mut std::process::Command) -> std::io::Result<std::process::Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

        let detached = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        match cmd
            .creation_flags(detached | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
        {
            Ok(child) => Ok(child),
            Err(_) => cmd.creation_flags(detached).spawn(),
        }
    }
    #[cfg(not(windows))]
    {
        cmd.spawn()
    }
}

/// Check whether a process with the given PID is still running.
pub fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: `kill` takes the PID and signal (0 = existence probe) by
        // value; it dereferences no pointers and reports failure via its return
        // value, so it cannot cause undefined behaviour.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
        };

        // SAFETY: every Win32 call below takes integer args plus the local
        // `exit_code` out-pointer; the handle is null-checked and closed on
        // every return path, so no resource leaks or invalid pointers occur.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let wait = WaitForSingleObject(handle, 0);
            if wait == WAIT_TIMEOUT {
                CloseHandle(handle);
                return true;
            }
            let mut exit_code: u32 = 0;
            GetExitCodeProcess(handle, &raw mut exit_code);
            CloseHandle(handle);
            exit_code == STILL_ACTIVE as u32
        }
    }
}

/// Ask a process to terminate gracefully (SIGTERM on Unix, nothing on Windows
/// since we prefer HTTP shutdown; the caller should have already tried that).
pub fn terminate_gracefully(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: `kill` takes the PID and signal by value; no pointer is
        // dereferenced and errors surface via the return value.
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if ret != 0 {
            anyhow::bail!(
                "Failed to send SIGTERM to PID {pid}: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        force_kill(pid)
    }
}

/// Unconditionally kill a process.
pub fn force_kill(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: `kill` takes the PID and signal by value; no pointer is
        // dereferenced and errors surface via the return value.
        let ret = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        if ret != 0 {
            anyhow::bail!(
                "Failed to send SIGKILL to PID {pid}: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };

        // SAFETY: the Win32 calls take integer args only; the handle is
        // null-checked and closed before returning on every path.
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                anyhow::bail!(
                    "Failed to open PID {pid} for termination: {}",
                    std::io::Error::last_os_error()
                );
            }
            let ok = TerminateProcess(handle, 1);
            CloseHandle(handle);
            if ok == 0 {
                anyhow::bail!(
                    "Failed to terminate PID {pid}: {}",
                    std::io::Error::last_os_error()
                );
            }
            Ok(())
        }
    }
}

/// PIDs this process must never signal: itself, its ancestor chain, and every
/// member of its own process group.
///
/// The ancestor chain matters whenever `sobre uninstall` runs *under*
/// sobre itself — a `sobre -c` wrapper (Cursor/Gemini hooks) can be the
/// parent, so the process tree is `sobre -c … → sh → sobre uninstall`.
/// Excluding only `getpid()` SIGTERMs the wrapper parent, which takes the
/// whole pipeline down mid-run (exit 143).
///
/// The process *group* matters because agent harnesses (Cursor's shell) can
/// reparent intermediaries to PID 1 mid-run — the `ps ppid` walk then stops
/// before reaching the outer wrapper, but the wrapper still shares the
/// foreground pgid; signalling it kills the pipeline all the same
/// (reproduced twice on the first fix).
fn protected_self_pids() -> std::collections::HashSet<u32> {
    let mut protected = std::collections::HashSet::new();
    protected.insert(std::process::id());
    #[cfg(unix)]
    {
        let mut pid = std::process::id();
        for _ in 0..16 {
            let Ok(output) = std::process::Command::new("ps")
                .args(["-o", "ppid=", "-p", &pid.to_string()])
                .output()
            else {
                break;
            };
            let Ok(ppid) = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u32>()
            else {
                break;
            };
            if ppid <= 1 || !protected.insert(ppid) {
                break;
            }
            pid = ppid;
        }

        // SAFETY: getpgrp() takes no arguments and cannot fail.
        let own_pgid = unsafe { libc::getpgrp() };
        if own_pgid > 0 {
            protected.extend(process_group_pids(own_pgid));
        }
    }
    protected
}

#[cfg(unix)]
fn process_group_pids(pgid: libc::pid_t) -> Vec<u32> {
    std::process::Command::new("pgrep")
        .args(["-g", &pgid.to_string()])
        .output()
        .ok()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse::<u32>().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// PIDs of the current user's processes named `name`, excluding this process
/// and its ancestor chain.
pub fn find_pids_by_name(name: &str) -> Vec<u32> {
    let protected = protected_self_pids();
    let mut pids = Vec::new();

    #[cfg(unix)]
    {
        // The current user's processes named exactly `name`. Matching on the
        // full command line would also catch a script whose path merely ends
        // in `/sobre`, and other users' processes are never ours.
        // SAFETY: geteuid() takes no arguments and cannot fail.
        let uid = unsafe { libc::geteuid() };
        if let Ok(output) = std::process::Command::new("pgrep")
            .args(["-x", "-u", &uid.to_string(), name])
            .output()
        {
            collect_pids(&output.stdout, &protected, &mut pids);
        }

        pids.sort_unstable();
        pids.dedup();
    }

    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("tasklist");
        cmd.args(["/FI", &format!("IMAGENAME eq {name}.exe")]);
        if let Ok(user) = std::env::var("USERNAME") {
            cmd.args(["/FI", &format!("USERNAME eq {user}")]);
        }
        if let Ok(output) = cmd.args(["/FO", "CSV", "/NH"]).output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() >= 2 {
                    let pid_str = parts[1].trim().trim_matches('"');
                    if let Ok(pid) = pid_str.parse::<u32>()
                        && !protected.contains(&pid)
                    {
                        pids.push(pid);
                    }
                }
            }
        }
    }

    pids
}

#[cfg(unix)]
fn collect_pids(stdout: &[u8], protected: &std::collections::HashSet<u32>, out: &mut Vec<u32>) {
    let text = String::from_utf8_lossy(stdout);
    for line in text.lines() {
        if let Ok(pid) = line.trim().parse::<u32>()
            && !protected.contains(&pid)
        {
            out.push(pid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_alive() {
        assert!(is_alive(std::process::id()));
    }

    #[test]
    fn bogus_pid_is_not_alive() {
        assert!(!is_alive(u32::MAX - 42));
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_returns_output_for_fast_command() {
        let mut cmd = std::process::Command::new("echo");
        cmd.arg("hello");
        let out = run_with_timeout(cmd, std::time::Duration::from_secs(5))
            .expect("fast command should complete");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn run_with_timeout_kills_slow_command() {
        let mut cmd = std::process::Command::new("sleep");
        cmd.arg("30");
        let start = std::time::Instant::now();
        let result = run_with_timeout(cmd, std::time::Duration::from_millis(300));
        assert!(result.is_none(), "slow command must time out");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "timeout must not wait for the full command"
        );
    }

    /// Follow-up: agent harnesses reparent intermediaries to PID 1, so
    /// the ppid walk alone misses the outer wrapper — the shared foreground
    /// process group must be protected too.
    #[cfg(unix)]
    #[test]
    fn protected_pids_cover_own_process_group() {
        // SAFETY: getpgrp() takes no arguments and cannot fail.
        let pgid = unsafe { libc::getpgrp() };
        // Bracket `protected_self_pids` with two group snapshots. A single
        // snapshot is racy under `cargo test` parallelism: sibling tests spawn
        // short-lived subprocesses (git/ps/pgrep) that transiently join this
        // foreground process group, so a member seen once may already be gone
        // when `protected_self_pids` runs — or appear only afterwards. Only PIDs
        // present in BOTH snapshots were alive across the whole window and must
        // therefore be covered by the protected set.
        let members_before = process_group_pids(pgid);
        let protected = protected_self_pids();
        let members_after = process_group_pids(pgid);
        for pid in members_before {
            if !members_after.contains(&pid) {
                continue; // transient foreign subprocess from a parallel test
            }
            assert!(
                protected.contains(&pid),
                "stable group member {pid} missing from protected set"
            );
        }
    }

    /// `uninstall` running *under* a sobre shell wrapper
    /// (`sobre -c … → sh → sobre uninstall`) must not SIGTERM its own
    /// ancestor chain — that kills the pipeline mid-run (exit 143).
    #[test]
    fn protected_pids_cover_self_and_ancestors() {
        let protected = protected_self_pids();
        assert!(protected.contains(&std::process::id()));
        #[cfg(unix)]
        {
            // The direct parent (cargo's test runner) must be protected too.
            let out = std::process::Command::new("ps")
                .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
                .output()
                .expect("ps runs");
            if let Ok(ppid) = String::from_utf8_lossy(&out.stdout).trim().parse::<u32>()
                && ppid > 1
            {
                assert!(
                    protected.contains(&ppid),
                    "parent {ppid} missing from {protected:?}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn find_pids_never_reports_own_process_tree() {
        // Regardless of what matches by name, the returned set must be
        // disjoint from the protected self/ancestor set.
        let protected = protected_self_pids();
        for pid in find_pids_by_name("sobre") {
            assert!(!protected.contains(&pid), "own tree pid {pid} reported");
        }
    }
}
