//! Derived normalization layer. The asinh transform is the standard
//! flow-cytometry biexponential-style mapping: out = asinh(x/m) * m / (asinh(T/m)).
//! Original raw values are never rewritten; transformed values live in this layer.

use crate::engine::model::TransformParam;

pub fn asinh_value(x: f64, m: f64, t: f64) -> f64 {
    let denom = (t / m).asinh();
    if denom == 0.0 {
        return f64::NAN;
    }
    (x / m).asinh() * m / denom
}

pub fn apply_param(p: &TransformParam, x: f64) -> f64 {
    match p.kind.as_str() {
        "asinh" => asinh_value(x, p.m, p.t),
        "linear" => x,
        _ => f64::NAN,
    }
}

/// Extreme-signal diagnostics: off-scale high/low values are retained in raw
/// form and tagged; they are never clipped just to fit a plot.
pub fn extreme_flags(x: f64, raw_text: &str, channel_max: f64, channel_min: f64) -> (Vec<String>, Option<String>) {
    let mut flags = Vec::new();
    let mut diag = None;
    if !x.is_finite() {
        flags.push("non_finite".to_string());
        diag = Some(format!("raw value `{raw_text}` is not finite; kept at original position"));
    } else {
        if x > channel_max {
            flags.push("off_scale_high".to_string());
            diag = Some(format!(
                "raw value {x} exceeds declared detector max {channel_max}; not clipped"
            ));
        } else if x < channel_min {
            flags.push("off_scale_low".to_string());
            diag = Some(format!(
                "raw value {x} is below declared detector min {channel_min}; not clipped"
            ));
        }
    }
    (flags, diag)
}
