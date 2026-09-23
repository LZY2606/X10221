//! Display transforms. Raw values stay untouched in the store; transforms
//! only happen in the derived layer. Unconvertible inputs yield diagnostics.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Transform {
    Linear,
    Asinh { cofactor: f64 },
}

impl Transform {
    pub fn apply(&self, v: f64) -> f64 {
        match self {
            Transform::Linear => v,
            Transform::Asinh { cofactor } => (v / cofactor).asinh(),
        }
    }

    pub fn basis_label(&self) -> String {
        match self {
            Transform::Linear => "linear".to_string(),
            Transform::Asinh { cofactor } => format!("asinh(cofactor={cofactor})"),
        }
    }
}

/// Diagnostic attached to a derived value that could not be normalised.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValueDiagnostic {
    pub code: String,
    pub detail: String,
}

/// Transform a raw detector value into the gate's coordinate basis.
/// Returns None + diagnostic when the value cannot be converted; the raw
/// value itself remains in the store untouched.
pub fn derive_value(
    raw: f64,
    transform: &Transform,
    range_max: f64,
) -> (Option<f64>, Option<ValueDiagnostic>) {
    if raw.is_nan() {
        return (
            None,
            Some(ValueDiagnostic {
                code: "not_a_number".into(),
                detail: "raw value is NaN; kept in place, excluded from plots".into(),
            }),
        );
    }
    if raw >= range_max {
        let t = transform.apply(raw);
        return (
            Some(t),
            Some(ValueDiagnostic {
                code: "signal_at_range_max".into(),
                detail: format!("raw value {raw} at detector range max {range_max}"),
            }),
        );
    }
    (Some(transform.apply(raw)), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asinh_roundtrip_scale() {
        let t = Transform::Asinh { cofactor: 150.0 };
        assert!((t.apply(0.0)).abs() < 1e-12);
        assert!((t.apply(30000.0) - (200.0f64).asinh()).abs() < 1e-9);
    }

    #[test]
    fn nan_stays_unconvertible() {
        let t = Transform::Linear;
        let (v, d) = derive_value(f64::NAN, &t, 262144.0);
        assert!(v.is_none());
        assert_eq!(d.unwrap().code, "not_a_number");
    }

    #[test]
    fn saturation_is_diagnosed_not_dropped() {
        let t = Transform::Asinh { cofactor: 150.0 };
        let (v, d) = derive_value(262144.0, &t, 262144.0);
        assert!(v.is_some());
        assert_eq!(d.unwrap().code, "signal_at_range_max");
    }
}
