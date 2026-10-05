//! Output-token savings reporting (#895).
//!
//! lean-ctx shapes *output* tokens through the terse prose style
//! (`compression_level`). There is no A/B measurement, so the reduction is the
//! shared model-based heuristic, presented as a *band*, never a hard number,
//! and clearly labelled so it is never mistaken for a measurement.

/// Relative half-width applied to the model-based point estimate to express its
/// inherent uncertainty as a band.
const ESTIMATE_REL_UNCERTAINTY: f64 = 0.35;

/// Model-based output-savings estimate, ready for the CLI / dashboard to render.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Savings {
    pub point_pct: f64,
    pub low_pct: f64,
    pub high_pct: f64,
}

/// Computes the output-savings estimate.
#[must_use]
pub(crate) fn current() -> Savings {
    let model = crate::core::stats::CostModel::default();
    #[allow(clippy::cast_precision_loss)]
    let verbose = model.avg_verbose_output_per_call as f64;
    #[allow(clippy::cast_precision_loss)]
    let concise = model.avg_concise_output_per_call as f64;
    let point = if verbose > 0.0 {
        (verbose - concise) / verbose * 100.0
    } else {
        0.0
    };
    let half = point * ESTIMATE_REL_UNCERTAINTY;
    Savings {
        point_pct: point,
        low_pct: (point - half).max(0.0),
        high_pct: point + half,
    }
}

/// Stable JSON shape for a [`Savings`], shared by the `lean-ctx output-savings`
/// CLI and the dashboard `/api/roi` route so both speak one schema.
#[must_use]
pub(crate) fn to_json(s: &Savings) -> serde_json::Value {
    serde_json::json!({
        "status": "estimated",
        "point_pct": round2(s.point_pct),
        "low_pct": round2(s.low_pct),
        "high_pct": round2(s.high_pct),
    })
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_is_a_band_around_the_point() {
        let s = current();
        assert!(s.low_pct <= s.point_pct && s.point_pct <= s.high_pct);
        assert!(s.low_pct >= 0.0);
        assert_eq!(to_json(&s)["status"], "estimated");
    }
}
