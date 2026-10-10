pub mod compress;
mod exec;
pub mod output_policy;
mod pipeline;
pub(crate) mod platform;
mod redact;
pub(crate) mod reentry;
pub(crate) mod tee_policy;

#[cfg(any(test, feature = "test-support"))]
pub use compress::compress_if_beneficial_pub;
pub use exec::{ExecMode, exec};
pub(crate) use exec::{STDERR_LABEL, combine_streams};
pub(crate) use exec::{exit_code, shell_timeout_with_override};
/// Quoting for a given shell flag, for the integration tests' Windows cases.
#[cfg(any(test, feature = "test-support"))]
pub fn join_command_for(args: &[String], shell_flag: &str) -> String {
    platform::join_command_for(args, shell_flag)
}
pub use platform::{
    decode_output, is_non_interactive, join_command, resolve_carriage_returns, shell_and_flag,
    shell_name,
};
pub(crate) use redact::cleanup_old_tee_logs;
pub use redact::save_tee;
