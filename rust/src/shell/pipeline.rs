use std::io::{self, IsTerminal, Read, Write};
use std::process::{Command, Stdio};

use crate::core::tokens::count_tokens;

use super::exec::{combine_streams, exec_byte_cap, wait_collecting};

pub(super) fn exec_buffered(command: &str, shell: &str, shell_flag: &str) -> i32 {
    #[cfg(windows)]
    super::platform::set_console_utf8();

    let mut cmd = Command::new(shell);

    // PowerShell runs the command inline (`-Command`), which the execution
    // policy does not govern, so no policy override is needed.
    #[cfg(windows)]
    {
        if super::platform::is_powershell(shell) {
            cmd.arg("-NoProfile");
            cmd.arg(shell_flag);
            cmd.arg(format!(
                "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; {command}"
            ));
        } else {
            cmd.arg(shell_flag);
            cmd.arg(command);
        }
    }
    #[cfg(not(windows))]
    {
        cmd.arg(shell_flag);
        cmd.arg(command);
    }

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    // Buffered path serves agents and pipes; interactive TTY stays intact.
    let isolate = !io::stdin().is_terminal();
    if isolate {
        // Piped stdin lets callers legitimately pipe data.
        cmd.stdin(Stdio::piped());
    }
    // The command stays in sobre's process group and gets the environment
    // it was given: a host that stops the command it wrapped stops it all,
    // and it runs as it would have without sobre.
    super::reentry::mark_child(&mut cmd);
    super::platform::apply_utf8_locale(&mut cmd);
    let child = cmd.spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("sobre: failed to execute: {e}");
            return 127;
        }
    };

    // Relay parent stdin to child stdin.
    if isolate && let Some(child_stdin) = child.stdin.take() {
        std::thread::Builder::new()
            .name("stdin-relay".into())
            .spawn(move || {
                super::platform::block_sigpipe_in_current_thread();
                let mut child_w = child_stdin;
                let mut parent_r = io::stdin().lock();
                let mut buf = [0u8; 8192];
                loop {
                    match parent_r.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if child_w.write_all(&buf[..n]).is_err() {
                                break;
                            }
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
                drop(child_w);
            })
            .ok();
    }

    let output = wait_collecting(child, exec_byte_cap(command));

    let exit_code = super::exec::exit_code(output.status);
    let stdout =
        super::platform::resolve_carriage_returns(&super::platform::decode_output(&output.stdout));
    let stderr =
        super::platform::resolve_carriage_returns(&super::platform::decode_output(&output.stderr));

    let full_output = combine_streams(&stdout, &stderr, exit_code);
    let input_tokens = count_tokens(&full_output);

    crate::core::diagnostics_store::record_from_shell(command, &full_output, exit_code);
    crate::core::gotcha_tracker::record_shell_outcome(command, &full_output, exit_code);

    let (compressed, output_tokens) =
        super::compress::compress_and_measure(command, &stdout, &stderr, exit_code);

    crate::core::tool_lifecycle::record_shell_command(command, input_tokens, output_tokens);

    if !compressed.is_empty() {
        let _ = io::stdout().write_all(compressed.as_bytes());
        if !compressed.ends_with('\n') {
            let _ = io::stdout().write_all(b"\n");
        }
    }
    // Same rule as the MCP path: whatever compression cut is saved, and a
    // compressed output always says so (on stderr, so stdout stays the
    // command's own).
    if compressed.trim_end() != full_output.trim_end() {
        let kept = if super::tee_policy::should_tee(&full_output, &compressed, false) {
            super::redact::save_tee(command, &full_output)
                .map(|p| format!("; full output: {p} (redacted, 24h)"))
                .unwrap_or_default()
        } else {
            String::new()
        };
        eprintln!("[sobre: output compressed, {input_tokens} → {output_tokens} tokens{kept}]");
    }

    exit_code
}
