//! BuiltinExperimentRunner — executes holdout/A-B experiments locally.
//!
//! sobre: the routing-eval engine this wrapped was removed, so the
//! capability reports Unavailable and every request is rejected.

use crate::core::ocla::traits::{ExperimentRunner, OclaService};
use crate::core::ocla::types::{
    ExperimentRequest, ExperimentResult, OclaCapability, OclaCapabilityKind,
    OclaCapabilityStatus, OclaResult,
};

pub struct BuiltinExperimentRunner;

impl BuiltinExperimentRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BuiltinExperimentRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl OclaService for BuiltinExperimentRunner {
    fn capability(&self) -> OclaCapability {
        // sobre removed `core::eval_ab::routing_eval`, the only engine this
        // capability ever wrapped. The OCLA plumbing is kept intact — the
        // ocla module as a whole is still an open decision — but the
        // capability advertises itself honestly instead of pretending.
        OclaCapability {
            kind: OclaCapabilityKind::ExperimentRunner,
            api_version: crate::core::ocla::types::OCLA_API_VERSION.to_string(),
            status: OclaCapabilityStatus::Unavailable,
            limits: std::collections::BTreeMap::new(),
        }
    }
}

impl ExperimentRunner for BuiltinExperimentRunner {
    fn run_experiment(&self, _request: ExperimentRequest) -> OclaResult<ExperimentResult> {
        Err(crate::core::ocla::types::OclaError::Rejected(
            OclaCapabilityKind::ExperimentRunner,
            "experiment runner is not available in sobre (routing eval removed)".into(),
        ))
    }
}
