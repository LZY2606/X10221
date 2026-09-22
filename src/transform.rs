//! asinh transform: transformed = asinh(x / cofactor).
//! Coordinate basis is recorded on every value; raw channels pass through.

use serde_json::{json, Value};

pub fn asinh_value(x: f64, cofactor: f64) -> f64 {
    (x / cofactor).asinh()
}

pub fn transform_events(
    comp_events: &[Value],
    channels: &[String],
    cofactors: &serde_json::Map<String, Value>,
    passthrough: &[String],
) -> Vec<Value> {
    comp_events
        .iter()
        .map(|row| {
            let idx = row["event_index"].clone();
            let arr = row["compensated"].as_array().cloned().unwrap_or_default();
            let vals: Vec<Value> = arr
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    let name = channels.get(c).cloned().unwrap_or_default();
                    let v = cell["value"].as_f64();
                    match v {
                        Some(v) if v.is_finite() && !passthrough.contains(&name) => {
                            let co = cofactors
                                .get(&name)
                                .and_then(|x| x.as_f64())
                                .or_else(|| cofactors.get("default").and_then(|x| x.as_f64()))
                                .unwrap_or(150.0);
                            json!({
                                "value": asinh_value(v, co),
                                "coordinate_basis": "asinh_compensated",
                                "unit": "asinh_units",
                                "cofactor": co
                            })
                        }
                        _ => cell.clone(),
                    }
                })
                .collect();
            json!({"event_index": idx, "transformed": vals})
        })
        .collect()
}
