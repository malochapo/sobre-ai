mod classification;
pub(crate) mod engine;
mod passthrough;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_engine;

#[cfg(any(test, feature = "test-support"))]
pub use engine::compress_if_beneficial_pub;

pub(super) use classification::is_excluded_command;
pub(super) use engine::compress_and_measure;

pub use classification::has_structural_output;
pub use classification::is_verbatim_output;

/// The built-in passthrough rules, for the generated reference: the command
/// list, the script runners, and the script-name keywords that mark a dev
/// script.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn builtin_passthrough() -> (
    &'static [&'static str],
    &'static [&'static str],
    &'static [&'static str],
) {
    (
        passthrough::BUILTIN_PASSTHROUGH,
        passthrough::SCRIPT_RUNNER_PREFIXES,
        passthrough::DEV_SCRIPT_KEYWORDS,
    )
}
