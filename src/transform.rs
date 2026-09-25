//! 变换参数独立版本化。派生层才做规范化；极端信号被标记而非丢弃。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformSpec {
    /// x' = (x - offset) * scale，坐标基准为线性通道空间。
    Linear { offset: f64, scale: f64 },
    /// x' = asinh((x - offset) / cofactor) / ln(10)，常用双曲反正弦缩放。
    Asinh { offset: f64, cofactor: f64 },
}

impl TransformSpec {
    pub fn apply(&self, x: f64) -> f64 {
        match self {
            TransformSpec::Linear { offset, scale } => (x - offset) * scale,
            TransformSpec::Asinh { offset, cofactor } => {
                ((x - offset) / cofactor).asinh() / std::f64::consts::LN_10
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelTransform {
    pub channel: String,
    pub spec: TransformSpec,
    /// 输出坐标基准，例如 "transform:asinh:channel:FITC-A"。
    pub output_basis: String,
}

impl ChannelTransform {
    pub fn new(channel: &str, spec: TransformSpec) -> Self {
        let tag = match &spec {
            TransformSpec::Linear { .. } => "linear",
            TransformSpec::Asinh { .. } => "asinh",
        };
        ChannelTransform {
            channel: channel.into(),
            output_basis: format!("transform:{tag}:channel:{channel}"),
            spec,
        }
    }

    /// 返回 (变换值, 诊断)。非有限或极端负值仍产出值但附诊断，供门控层决定。
    pub fn apply_with_diagnostics(&self, x: f64) -> (f64, Vec<String>) {
        let mut diag = Vec::new();
        if !x.is_finite() {
            return (f64::NAN, vec!["EXTREME_NON_FINITE".into()]);
        }
        let y = self.spec.apply(x);
        if x < 0.0 {
            diag.push("EXTREME_NEGATIVE_BASELINE".into());
        }
        if y.abs() > 10.0 {
            diag.push("EXTREME_TRANSFORMED_MAGNITUDE".into());
        }
        (y, diag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asinh_zero_is_zero() {
        let t = ChannelTransform::new("FITC", TransformSpec::Asinh { offset: 0.0, cofactor: 150.0 });
        let (y, diag) = t.apply_with_diagnostics(0.0);
        assert!(y.abs() < 1e-12);
        assert!(diag.is_empty());
        assert_eq!(t.output_basis, "transform:asinh:channel:FITC");
    }

    #[test]
    fn extreme_magnitude_is_flagged_not_dropped() {
        let t = ChannelTransform::new("FITC", TransformSpec::Asinh { offset: 0.0, cofactor: 150.0 });
        let (y, diag) = t.apply_with_diagnostics(1e9);
        assert!(y.is_finite());
        assert!(diag.iter().any(|d| d == "EXTREME_TRANSFORMED_MAGNITUDE"));
    }
}
