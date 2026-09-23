//! 变换层：补偿后线性计数 -> 无量纲显示坐标。
//!
//! 规则：无法转换的值留在原位置（null 坐标）并附诊断，绝不为画图而裁剪/丢弃。

use super::model::{TransformParams, TransformSpec};

#[derive(Debug, Clone, PartialEq)]
pub struct TransformDiagnostic {
    pub channel: String,
    pub event_index: usize,
    pub code: String,
    pub message: String,
    /// 原始值（未规范化前）
    pub raw_value: f64,
}

/// 单通道单事件的变换；不可转换时返回 None。
pub fn transform_value(spec: &TransformSpec, x_max: f64, x: f64) -> Option<f64> {
    if !x.is_finite() {
        return None;
    }
    match spec {
        // 混合 asinh：在原点附近近似线性，高点饱和到 m；保证全定义域可转换。
        TransformSpec::Asinh { m, a, b, x0 } => {
            let z = (x - x0) / b;
            if !z.is_finite() {
                return None;
            }
            let z_max = (x_max - x0) / b;
            let blend = a * z + (1.0 - a) * z.asinh();
            let blend_max = a * z_max + (1.0 - a) * z_max.asinh();
            if blend_max.abs() < f64::EPSILON {
                return None;
            }
            Some(blend / blend_max * m)
        }
        // log10：x - x0 <= 0 不可转换。
        TransformSpec::Log10 { b, x0 } => {
            let z = (x - x0) / b;
            if z <= 0.0 {
                None
            } else {
                Some(z.log10())
            }
        }
    }
}

/// 对补偿后事件矩阵应用变换；返回坐标矩阵（不可转换为 null）与诊断列表。
pub fn apply_transform(
    events: &[Vec<f64>],
    channel_names: &[String],
    params: &TransformParams,
) -> (Vec<Vec<Option<f64>>>, Vec<TransformDiagnostic>) {
    let mut coords = vec![vec![None; channel_names.len()]; events.len()];
    let mut diags = Vec::new();
    for (ei, row) in events.iter().enumerate() {
        for (ci, name) in channel_names.iter().enumerate() {
            if !params.applies_to.contains(name) {
                // 不参与荧光变换的通道（FSC/SSC）保持线性坐标，仍是合法坐标
                coords[ei][ci] = Some(row[ci]);
                continue;
            }
            let v = row[ci];
            match transform_value(&params.spec, params.x_max, v) {
                Some(y) => coords[ei][ci] = Some(y),
                None => {
                    coords[ei][ci] = None;
                    let code = if !v.is_finite() {
                        "non_finite"
                    } else {
                        "outside_transform_domain"
                    };
                    diags.push(TransformDiagnostic {
                        channel: name.clone(),
                        event_index: ei,
                        code: code.into(),
                        message: match &params.spec {
                            TransformSpec::Log10 { .. } => {
                                "log10 要求 x-x0>0；该值保留在原位置，不裁剪".into()
                            }
                            TransformSpec::Asinh { .. } => "asinh 参数导致不可定义".into(),
                        },
                        raw_value: v,
                    });
                }
            }
        }
    }
    (coords, diags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asinh_maps_zero_low_and_max() {
        let p = TransformParams {
            applies_to: vec!["FITC-A".into()],
            x_max: 65535.0,
            spec: TransformSpec::Asinh {
                m: 5.0,
                a: 0.05,
                b: 150.0,
                x0: 0.0,
            },
        };
        assert_eq!(transform_value(&p.spec, p.x_max, 0.0), Some(0.0));
        let hi = transform_value(&p.spec, p.x_max, 65535.0).unwrap();
        assert!((hi - 5.0).abs() < 1e-12);
        assert!(transform_value(&p.spec, p.x_max, 1000.0).unwrap() > 0.0);
    }

    #[test]
    fn log10_keeps_nonpositive_in_place_with_diagnostic() {
        let p = TransformParams {
            applies_to: vec!["FITC-A".into()],
            x_max: 65535.0,
            spec: TransformSpec::Log10 { b: 1.0, x0: 0.0 },
        };
        assert!(transform_value(&p.spec, p.x_max, 1000.0).unwrap() > 0.0);
        assert_eq!(transform_value(&p.spec, p.x_max, 0.0), None);
        assert_eq!(transform_value(&p.spec, p.x_max, -5.0), None);
        let (coords, diags) = apply_transform(
            &[vec![0.0], vec![100.0]],
            &["FITC-A".into()],
            &p,
        );
        assert_eq!(coords[0][0], None);
        assert_eq!(coords[1][0], Some(2.0));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "outside_transform_domain");
    }
}
