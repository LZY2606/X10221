//! Compensated fluorescence transform.
//!
//! `f(x) = asinh(x / (c * ln(256))) / ln(256) * T`. Output values live in the
//! named transformed coordinate basis of [`TransformParams::output_basis`];
//! input keeps compensated detector counts.

use crate::domain::{EventValue, TransformParams};

pub fn forward(params: &TransformParams, compensated_counts: f64) -> f64 {
    let denom = params.linear_width_counts * 256.0_f64.ln();
    let t = params.top_scale_counts;
    (compensated_counts / denom).asinh() / 256.0_f64.ln() * t
}

pub fn backward(params: &TransformParams, transformed: f64) -> f64 {
    let denom = params.linear_width_counts * 256.0_f64.ln();
    let t = params.top_scale_counts;
    (transformed / t * 256.0_f64.ln()).sinh() * denom
}

/// Project one event value onto a gate axis. Returns None when the value
/// cannot be plotted (literal / NaN) — the value is never silently dropped.
pub fn project(params: &TransformParams, value: &EventValue) -> Option<f64> {
    let counts = value.as_number()?;
    Some(forward(params, counts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ArtifactKind;
    use crate::domain::ArtifactRef;

    fn params() -> TransformParams {
        TransformParams {
            method: "asinh".into(),
            channels: vec![],
            derived_from: ArtifactRef {
                kind: ArtifactKind::Compensation,
                id: "x".into(),
            },
            input_unit: "counts_compensated".into(),
            output_basis: "asinh:T=262144:W=1500".into(),
            top_scale_counts: 262_144.0,
            linear_width_counts: 1500.0,
        }
    }

    #[test]
    fn zero_stays_zero_and_roundtrips() {
        let p = params();
        let z = forward(&p, 0.0);
        assert!(z.abs() < 1e-9);
        assert!(backward(&p, forward(&p, 12345.6)).abs_sub(12345.6) < 1e-6);
    }

    #[test]
    fn literals_do_not_plot() {
        let p = params();
        assert!(project(
            &p,
            &EventValue::Literal {
                literal: "OVERFLOW".into(),
                diagnostic: "above detector range".into()
            }
        )
        .is_none());
    }
}
