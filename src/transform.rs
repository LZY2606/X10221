//! 坐标变换：双曲反正弦（asinh）尺度。参数版本化，派生层才做规范化。

use crate::domain::TransformParams;

/// x' = asinh((x - offset) / cofactor)。输入为 NaN/缺失由调用方跳过。
pub fn asinh_value(p: &TransformParams, x: f64) -> f64 {
    ((x - p.offset) / p.cofactor).asinh()
}

/// 标量 asinh，便于测试。
pub fn asinh_scaled(x: f64, cofactor: f64, offset: f64) -> f64 {
    ((x - offset) / cofactor).asinh()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(cofactor: f64, offset: f64) -> TransformParams {
        TransformParams {
            transform_id: "t".into(),
            version: 1,
            family: "asinh".into(),
            cofactor,
            offset,
            input_unit: "adu".into(),
            output_unit: "asinh(adu)".into(),
            basis: "b".into(),
        }
    }

    #[test]
    fn zero_offset_maps_to_zero() {
        let p = params(150.0, 0.0);
        assert!(asinh_value(&p, 0.0).abs() < 1e-12);
        assert!(asinh_value(&p, 1000.0) > asinh_value(&p, 100.0));
    }

    #[test]
    fn changing_cofactor_changes_position() {
        let v1 = params(150.0, 0.0);
        let v2 = params(5000.0, 0.0);
        let a = asinh_value(&v1, 500.0);
        let b = asinh_value(&v2, 500.0);
        assert!(a > b);
    }
}
