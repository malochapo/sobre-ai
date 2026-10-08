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
