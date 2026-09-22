//! 通道变换：补偿后信号 -> 显示坐标。
//!
//! 当前实现流式平台常用的反双曲正弦（arcsinh）：
//!   x' = asinh(x / cofactor)
//! 单位与坐标基准随参数材料版本化；非有限值留在原位置并附诊断。

use serde::{Deserialize, Serialize};

pub const TRANSFORM_RULE: &str = "transform.asinh.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformParams {
    /// 每个通道的 cofactor（与补偿后同单位）。
    pub cofactors: Vec<ChannelCofactor>,
    pub output_unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelCofactor {
    pub channel: String,
    pub cofactor: f64,
    pub input_unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformDiagnostic {
    pub coord: usize,
    pub code: String,
    pub detail: String,
}

pub fn transform_value(cofactor: f64, x: f64) -> f64 {
    (x / cofactor).asinh()
}

pub fn transform_all(
    params: &TransformParams,
    values: &[f64],
) -> (Vec<f64>, Vec<TransformDiagnostic>) {
    let mut out = Vec::with_capacity(values.len());
    let mut diags = Vec::new();
    for (i, v) in values.iter().enumerate() {
        let cofactor = params
            .cofactors
            .get(i)
            .map(|c| c.cofactor)
            .filter(|c| *c > 0.0 && c.is_finite())
            .unwrap_or(150.0);
        if !v.is_finite() {
            out.push(*v);
            diags.push(TransformDiagnostic {
                coord: i,
                code: "value.unconvertible".to_string(),
                detail: "极端/非有限信号无法进入变换，保留原值供审阅".to_string(),
            });
        } else {
            out.push(transform_value(cofactor, *v));
        }
    }
    (out, diags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_maps_to_zero_and_monotonic() {
        assert_eq!(transform_value(150.0, 0.0), 0.0);
        let a = transform_value(150.0, 100.0);
        let b = transform_value(150.0, 1000.0);
        assert!(b > a);
        // 线性近似：大值处斜率更小（压缩动态范围）。
        assert!(transform_value(150.0, -500.0) < 0.0);
    }

    #[test]
    fn non_finite_preserved() {
        let p = TransformParams {
            cofactors: vec![ChannelCofactor {
                channel: "FL1".to_string(),
                cofactor: 150.0,
                input_unit: "fluorescence".to_string(),
            }],
            output_unit: "asinh(fluorescence)".to_string(),
        };
        let (out, diags) = transform_all(&p, &[f64::NAN]);
        assert!(out[0].is_nan());
        assert_eq!(diags[0].code, "value.unconvertible");
    }
}
