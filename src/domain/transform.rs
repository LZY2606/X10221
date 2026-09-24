//! 显示/门控坐标变换。
//!
//! 原始通道值始终保留；派生层才做规范化。无法转换（NaN / 非有限）的值
//! **留在原位置并附诊断**，绝不丢弃。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformSpec {
    /// 线性：y = (x - offset) / scale，坐标基准：线性通道值（线性单位）。
    Linear { offset: f64, scale: f64, unit: String },
    /// 流式常用反双曲正弦：y = asinh((x - offset) / cofactor) / ln(10) * decade_span
    /// 的简化学术形式 y = asinh(x/cofactor)，坐标基准：asinh 双曲坐标。
    ArcSinh { cofactor: f64, unit: String },
}

impl TransformSpec {
    pub fn basis(&self) -> &str {
        match self {
            TransformSpec::Linear { .. } => "linear",
            TransformSpec::ArcSinh { .. } => "asinh",
        }
    }

    pub fn unit(&self) -> &str {
        match self {
            TransformSpec::Linear { unit, .. } | TransformSpec::ArcSinh { unit, .. } => unit,
        }
    }

    /// 返回 (变换值, 诊断)。无法转换时变换值为 None，诊断说明原因；原始值仍在调用方保留。
    pub fn apply(&self, x: f64) -> (Option<f64>, Option<String>) {
        if x.is_nan() {
            return (None, Some("NaN：无法变换，原始值保留".into()));
        }
        if !x.is_finite() {
            return (None, Some("非有限值（±Inf）：无法变换，原始值保留".into()));
        }
        let y = match self {
            TransformSpec::Linear { offset, scale, .. } => {
                if *scale == 0.0 {
                    return (None, Some("线性变换 scale=0".into()));
                }
                (x - offset) / scale
            }
            TransformSpec::ArcSinh { cofactor, .. } => {
                if *cofactor <= 0.0 {
                    return (None, Some("asinh cofactor 必须为正".into()));
                }
                (x / cofactor).asinh()
            }
        };
        if !y.is_finite() {
            (None, Some("变换结果非有限".into()))
        } else {
            (Some(y), None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_stays_in_place_with_diagnostic() {
        let t = TransformSpec::ArcSinh { cofactor: 150.0, unit: "a.u.".into() };
        let (y, d) = t.apply(f64::NAN);
        assert!(y.is_none());
        assert!(d.unwrap().contains("NaN"));
    }

    #[test]
    fn inf_kept_with_diagnostic() {
        let t = TransformSpec::Linear { offset: 0.0, scale: 1.0, unit: "a.u.".into() };
        assert!(t.apply(f64::INFINITY).0.is_none());
    }

    #[test]
    fn extreme_saturations_transform_to_large_but_finite() {
        let t = TransformSpec::ArcSinh { cofactor: 150.0, unit: "a.u.".into() };
        let (y, d) = t.apply(262_143.0); // 18-bit ADC 饱和
        assert!(d.is_none());
        let y = y.unwrap();
        assert!(y.is_finite());
        assert!(y > 7.0); // 压缩但不丢
    }

    #[test]
    fn arcsinh_zero_and_negative() {
        let t = TransformSpec::ArcSinh { cofactor: 150.0, unit: "a.u.".into() };
        assert_eq!(t.apply(0.0).0.unwrap(), 0.0);
        assert!(t.apply(-5000.0).0.unwrap() < -3.0);
    }
}
