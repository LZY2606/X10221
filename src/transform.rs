//! Display transforms (asinh with cofactor, the flow-cytometry standard).
//! Extreme signals are NOT clamped: values outside the acquisition range stay
//! in place and produce a structured diagnostic instead.

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransformParams {
    pub kind: String, // "asinh"
    pub cofactor: f64,
    pub range: f64,   // acquisition range ($PnR), in raw unit
    pub unit: String, // e.g. "ADC channels"
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TransformDiagnostic {
    pub param: String,
    pub event_index: usize,
    pub raw_value: f64,
    pub unit: String,
    pub range: f64,
    pub message: String,
}

pub fn asinh_transform(value: f64, cofactor: f64) -> f64 {
    (value / cofactor).asinh()
}

/// Returns (transformed_value, Option<diagnostic>). Out-of-range values are
/// returned unmodified (raw representation preserved) plus a diagnostic.
pub fn apply(
    value: f64,
    param: &str,
    event_index: usize,
    p: &TransformParams,
) -> (f64, Option<TransformDiagnostic>) {
    if value < 0.0 || value > p.range {
        return (
            value,
            Some(TransformDiagnostic {
                param: param.into(),
                event_index,
                raw_value: value,
                unit: p.unit.clone(),
                range: p.range,
                message: format!(
                    "value {value} {} outside acquisition range [0, {}]; kept raw, not clamped",
                    p.unit, p.range
                ),
            }),
        );
    }
    (asinh_transform(value, p.cofactor), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_kept_raw_with_diagnostic() {
        let p = TransformParams {
            kind: "asinh".into(),
            cofactor: 150.0,
            range: 262144.0,
            unit: "ADC channels".into(),
        };
        let (v, d) = apply(300000.0, "FL1-A", 3, &p);
        assert_eq!(v, 300000.0); // not clamped
        assert!(d.unwrap().message.contains("kept raw"));
    }
}
