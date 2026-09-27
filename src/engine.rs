use crate::domain::{Diagnostic, Membership};
use serde_json::{json, Map, Value};

pub fn determinant3(values: &[Vec<f64>]) -> Option<f64> {
    if values.len() != 3 || values.iter().any(|row| row.len() != 3) {
        return None;
    }
    let a = values[0][0];
    let b = values[0][1];
    let c = values[0][2];
    let d = values[1][0];
    let e = values[1][1];
    let f = values[1][2];
    let g = values[2][0];
    let h = values[2][1];
    let i = values[2][2];
    Some(a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g))
}

pub fn condition_proxy(values: &[Vec<f64>]) -> Option<f64> {
    let det = determinant3(values)?;
    let max_abs = values
        .iter()
        .flat_map(|row| row.iter())
        .copied()
        .fold(0.0_f64, f64::max);
    if det.abs() < f64::EPSILON || max_abs <= 0.0 {
        return None;
    }
    Some(max_abs.powi(3) / det.abs())
}

pub fn near_singular(values: &[Vec<f64>]) -> bool {
    match condition_proxy(values) {
        Some(condition) => condition > 100.0,
        None => true,
    }
}

pub fn inverse3(values: &[Vec<f64>]) -> Option<[[f64; 3]; 3]> {
    if near_singular(values) {
        return None;
    }
    let det = determinant3(values)?;
    let a = values[0][0];
    let b = values[0][1];
    let c = values[0][2];
    let d = values[1][0];
    let e = values[1][1];
    let f = values[1][2];
    let g = values[2][0];
    let h = values[2][1];
    let i = values[2][2];
    let mut inverse = [[0.0; 3]; 3];
    inverse[0][0] = (e * i - f * h) / det;
    inverse[0][1] = (c * h - b * i) / det;
    inverse[0][2] = (b * f - c * e) / det;
    inverse[1][0] = (f * g - d * i) / det;
    inverse[1][1] = (a * i - c * g) / det;
    inverse[1][2] = (c * d - a * f) / det;
    inverse[2][0] = (d * h - e * g) / det;
    inverse[2][1] = (b * g - a * h) / det;
    inverse[2][2] = (a * e - b * d) / det;
    Some(inverse)
}

pub fn compensate(values: &[Vec<f64>], vector: [f64; 3]) -> Option<[f64; 3]> {
    let inverse = inverse3(values)?;
    let mut result = [0.0; 3];
    for row in 0..3 {
        result[row] = inverse[row][0] * vector[0]
            + inverse[row][1] * vector[1]
            + inverse[row][2] * vector[2];
    }
    Some(result)
}

pub fn asinh_scale(value: f64, cofactor: f64) -> f64 {
    (value / cofactor).asinh()
}

pub fn channel_value(channels: &Map<String, Value>, channel: &str) -> Option<f64> {
    channels.get(channel)?.get("value")?.as_f64()
}

pub fn point_membership(point: [f64; 2], polygon: &[[f64; 2]]) -> Membership {
    let mut inside = false;
    let mut boundary = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let pi = polygon[i];
        let pj = polygon[j];
        if on_segment(point, pi, pj) {
            boundary = true;
        }
        if (pi[1] > point[1]) != (pj[1] > point[1]) {
            let crossing = pi[0]
                + (pj[0] - pi[0]) * (point[1] - pi[1]) / (pj[1] - pi[1]);
            if (point[0] - crossing).abs() < f64::EPSILON {
                boundary = true;
            }
            if point[0] < crossing {
                inside = !inside;
            }
        }
        j = i;
    }
    if boundary {
        Membership::Boundary
    } else if inside {
        Membership::Inside
    } else {
        Membership::Outside
    }
}

fn on_segment(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> bool {
    let cross = (point[1] - start[1]) * (end[0] - start[0])
        - (point[0] - start[0]) * (end[1] - start[1]);
    if cross.abs() > 1e-9 {
        return false;
    }
    let dot = (point[0] - start[0]) * (point[0] - end[0])
        + (point[1] - start[1]) * (point[1] - end[1]);
    dot <= 1e-9
}

pub fn matrix_diagnostic(version: i64, values: &[Vec<f64>]) -> Option<Diagnostic> {
    let det = determinant3(values).unwrap_or_default();
    let condition = condition_proxy(values);
    if near_singular(values) {
        return Some(Diagnostic {
            code: "NEAR_SINGULAR_COMPENSATION_NOT_INVERTED".to_string(),
            severity: "error".to_string(),
            message: format!(
                "compensation-matrix@v{version} determinant={det:.6}; inversion refused; condition proxy={}",
                condition.map(|value| format!("{value:.1}")).unwrap_or_else(|| "infinite".to_string())
            ),
            affected: ["compensation:FITC-A", "compensation:PE-A", "compensation:PerCP-A"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect(),
        });
    }
    None
}

pub fn raw_channel_diagnostic(event_id: &str, channel: &str, raw: &Value) -> Option<Value> {
    if raw.get("value")?.is_null() {
        return Some(json!({
            "code": "UNCONVERTIBLE_VALUE_PRESERVED",
            "severity": "warning",
            "message": format!("{event_id}.{channel} remains at its raw instrument representation"),
            "affected": [format!("raw:{event_id}.{channel}"), format!("compensated:{event_id}.{channel}"), format!("transformed:{event_id}.{channel}")]
        }));
    }
    let value = raw.get("value")?.as_f64()?;
    if value >= 100_000.0 {
        Some(json!({
            "code": "EXTREME_SIGNAL_PRESERVED".to_string(),
            "severity": "warning",
            "message": format!("{event_id}.{channel} is retained at {value} ADC count instead of being clipped"),
            "affected": [format!("raw:{event_id}.{channel}"), format!("compensated:{event_id}.{channel}"), format!("transformed:{event_id}.{channel}")]
        }))
    } else {
        None
    }
}
