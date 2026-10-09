//! Doctor check catalog, split by domain.
//!
//! Every check returns an [`Outcome`] (or `Option`/`Vec` thereof) and is
//! re-exported flat for `doctor/mod.rs`.

mod clients;
mod environment;
mod proxy;
mod security;
mod storage;

pub(crate) use clients::*;
pub(crate) use environment::*;
pub(crate) use proxy::*;
pub(crate) use security::*;
pub(crate) use storage::*;

#[cfg(test)]
use crate::doctor::Outcome;

#[cfg(test)]
mod tests;
