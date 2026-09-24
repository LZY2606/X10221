//! 显示变换：参数版本化；不合法输入返回诊断而非静默丢弃。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransformSpec {
    Linear { scale: f64, offset: f64 },
    Log10 { decade_floor: f64 },
    Asinh { cofactor: f64 },
}

impl TransformSpec {
    pub fn name(&self) -> &'static str {
        match self {
            TransformSpec::Linear { .. } => "linear",
            TransformSpec::Log10 { .. } => "log10",
            TransformSpec::Asinh { .. } => "asinh",
        }
    }

    pub fn apply(&self, x: f64) -> Result<f64, String> {
        if !x.is_finite() {
            return Err("非有限输入".to_string());
        }
        match self {
            TransformSpec::Linear { scale, offset } => {
                if *scale == 0.0 {
                    return Err("linear scale=0".to_string());
                }
                Ok(scale * x + offset)
            }
            TransformSpec::Log10 { decade_floor } => {
                if x <= *decade_floor {
                    return Err(format!("log10 输入 {} <= 下界 {}", x, decade_floor));
                }
                Ok(x.log10())
            }
            TransformSpec::Asinh { cofactor } => {
                if *cofactor <= 0.0 {
                    return Err("asinh cofactor 必须为正".to_string());
                }
                Ok((x / cofactor).asinh())
            }
        }
    }
}
