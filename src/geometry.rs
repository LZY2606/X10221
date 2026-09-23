//! 坐标变换与门多边形点包含。
//!
//! 边界点（恰好落在多边形边上）不允许由包含约定默默决定归属：
//! 返回 `OnBoundary`，由上层为“计入”和“排除”保留全部合法候选。

use crate::domain::{TransformChannel, TransformPayload};

#[derive(Debug, Clone, PartialEq)]
pub enum TransformError {
    Overflow { flag: String, raw: f64 },
    NegativeForLog { raw: f64 },
}

/// 单通道派生层规范化：导入层保留原值，这里只在“可以唯一决定”时给值。
pub fn transform_value(raw: f64, flag: Option<&str>, spec: &TransformChannel) -> Result<f64, TransformError> {
    if let Some(f) = flag {
        if f == "OVF" {
            return Err(TransformError::Overflow { flag: f.into(), raw });
        }
    }
    match spec.kind.as_str() {
        "linear" => Ok(raw),
        "log10_offset" => {
            if raw < 0.0 {
                return Err(TransformError::NegativeForLog { raw });
            }
            let off = spec.offset.unwrap_or(0.0);
            Ok((raw + off).log10())
        }
        other => panic!("未知变换类型 {other}（案例生成器只产出 linear/log10_offset）"),
    }
}

pub fn spec_for<'a>(t: &'a TransformPayload, channel: &str) -> Option<&'a TransformChannel> {
    t.channels.iter().find(|c| c.channel == channel)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    Inside,
    Outside,
    /// 恰在边上：计入与排除都是合法解释。
    OnBoundary,
}

const EPS: f64 = 1e-9;

/// 射线法；点在边上单独报告。
pub fn point_in_polygon(poly: &[[f64; 2]], p: [f64; 2]) -> Containment {
    let n = poly.len();
    if n < 3 {
        return Containment::Outside;
    }
    let (x, y) = (p[0], p[1]);
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);

        if point_on_segment(xi, yi, xj, yj, x, y) {
            return Containment::OnBoundary;
        }
        if (yi > y) != (yj > y) {
            let x_intersect = xi + (y - yi) * (xj - xi) / (yj - yi);
            if x_intersect > x {
                inside = !inside;
            }
        }
        j = i;
    }
    if inside {
        Containment::Inside
    } else {
        Containment::Outside
    }
}

fn point_on_segment(x1: f64, y1: f64, x2: f64, y2: f64, px: f64, py: f64) -> bool {
    let cross = (px - x1) * (y2 - y1) - (py - y1) * (x2 - x1);
    if cross.abs() > EPS {
        return false;
    }
    let dot = (px - x1) * (px - x2) + (py - y1) * (py - y2);
    dot <= EPS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn inside_outside_and_boundary() {
        let p = square();
        assert_eq!(point_in_polygon(&p, [5.0, 5.0]), Containment::Inside);
        assert_eq!(point_in_polygon(&p, [11.0, 5.0]), Containment::Outside);
        assert_eq!(point_in_polygon(&p, [10.0, 5.0]), Containment::OnBoundary);
        assert_eq!(point_in_polygon(&p, [0.0, 0.0]), Containment::OnBoundary);
        assert_eq!(point_in_polygon(&p, [5.0, 0.0]), Containment::OnBoundary);
    }

    #[test]
    fn overflow_and_negative_stay_unconverted() {
        let log = TransformChannel {
            channel: "FITC".into(),
            kind: "log10_offset".into(),
            offset: Some(1.0),
            input_unit: "a.u.".into(),
            output_basis: "decades".into(),
        };
        assert_eq!(
            transform_value(262143.0, Some("OVF"), &log),
            Err(TransformError::Overflow { flag: "OVF".into(), raw: 262143.0 })
        );
        assert!(matches!(
            transform_value(-5.0, None, &log),
            Err(TransformError::NegativeForLog { raw: -5.0 })
        ));
        let lin = TransformChannel {
            channel: "FSC-A".into(),
            kind: "linear".into(),
            offset: None,
            input_unit: "a.u.".into(),
            output_basis: "instrument.raw".into(),
        };
        assert_eq!(transform_value(50000.0, None, &lin), Ok(50000.0));
        // 线性轴上的溢出仍然被标记拦截（不能为了画图而悄悄截断）
        assert_eq!(
            transform_value(262143.0, Some("OVF"), &lin),
            Err(TransformError::Overflow { flag: "OVF".into(), raw: 262143.0 })
        );
    }
}
