//! 派生变换：asinh((x-offset)/cofactor)/ln10，输出为 asinh 对数尺度。
//! 极端信号（负补偿值、超量程）不丢弃，照常变换并打诊断标记。

use crate::domain::ChannelTransform;

pub fn apply_transform(x: f64, t: &ChannelTransform) -> f64 {
    ((x - t.offset) / t.cofactor).asinh() / std::f64::consts::LN_10
}

/// 极端信号诊断：非有限、负补偿、超 262144（18-bit ADC 上限）线性量程。
pub fn extreme_diagnostic(x: f64) -> Option<&'static str> {
    if !x.is_finite() {
        Some("non-finite signal retained")
    } else if x < 0.0 {
        Some("negative compensated signal (over-compensation), retained")
    } else if x > 262_144.0 {
        Some("signal exceeds 18-bit ADC range 262144, retained")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asinh_monotone_and_negative_ok() {
        let t = ChannelTransform {
            channel: "FL1".into(),
            method: "asinh".into(),
            cofactor: 150.0,
            offset: 0.0,
            output_unit: "asinh:log10-scale".into(),
        };
        assert!(apply_transform(10_000.0, &t) > apply_transform(100.0, &t));
        assert!(apply_transform(-500.0, &t).is_finite());
        assert!(extreme_diagnostic(-500.0).is_some());
        assert!(extreme_diagnostic(300_000.0).is_some());
        assert!(extreme_diagnostic(100.0).is_none());
    }
}
