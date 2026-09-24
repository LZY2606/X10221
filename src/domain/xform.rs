use crate::domain::types::{FcsEvents, Scaled};

/// 派生层规范化：asinh(x / cofactor)。无法转换或缺失 cofactor 时保留诊断，不丢点。
pub fn asinh_x(v: f64, cofactor: f64) -> f64 {
    let z = v / cofactor;
    (z + (z * z + 1.0).sqrt()).asinh()
}

pub fn transform_point(raw: &[Scaled], channels: &[String], cofactors: &std::collections::BTreeMap<String, f64>) -> Vec<Scaled> {
    raw.iter()
        .enumerate()
        .map(|(i, s)| {
            let name = &channels[i];
            match (s.value, cofactors.get(name)) {
                (Some(v), Some(c)) if v.is_finite() && *c > 0.0 => {
                    let t = asinh_x(v, *c);
                    Scaled {
                        raw: s.raw.clone(),
                        value: Some(t),
                        unit: format!("asinh(cofactor={})", format_c(*c)),
                        diagnostics: s.diagnostics.clone(),
                    }
                }
                (Some(_), _) => Scaled {
                    raw: s.raw.clone(),
                    value: None,
                    unit: "asinh(untransformed)".to_string(),
                    diagnostics: {
                        let mut d = s.diagnostics.clone();
                        d.push(format!("通道 {} 缺少正 cofactor，保持原值不强行变换", name));
                        d
                    },
                },
                _ => s.clone(),
            }
        })
        .collect()
}

fn format_c(c: f64) -> String {
    if c == c.trunc() {
        format!("{}", c as i64)
    } else {
        format!("{}", c)
    }
}

pub fn transform_all(events: &FcsEvents, cofactors: &std::collections::BTreeMap<String, f64>) -> Vec<Vec<Scaled>> {
    let names: Vec<String> = events.channels.iter().map(|c| c.name.clone()).collect();
    events.points.iter().map(|p| transform_point(&p.channels, &names, cofactors)).collect()
}
