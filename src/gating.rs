//! 门控几何与群体百分比。
//! 边界点规则：点恰在边或顶点上按“边界包含”计为 Inside；近阈值容差内为 Boundary 并单独标记。
//! 规则 POLICY-GATE-DENOM：群体百分比的分母钉住父群结果版本（node id + result seq + 计数快照）。

use crate::model::{EventPoint, GatePolygon, TransformSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    Outside,
    Boundary,
    Inside,
}

const BOUNDARY_EPS: f64 = 1e-9;

/// 射线法 + 边距判定（点集按闭合多边形处理）。
pub fn point_in_polygon(x: f64, y: f64, poly: &[[f64; 2]]) -> Containment {
    let n = poly.len();
    if n < 3 {
        return Containment::Outside;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (poly[i][0], poly[i][1]);
        let (xj, yj) = (poly[j][0], poly[j][1]);

        if point_on_segment(x, y, xi, yi, xj, yj) {
            return Containment::Boundary;
        }

        let intersect = (yi > y) != (yj > y)
            && x < (xj - xi) * (y - yi) / (yj - yi) + xi;
        if intersect {
            inside = !inside;
        }
        j = i;
    }
    if inside {
        Containment::Inside
    } else {
        Containment::Outside
    }
}

fn point_on_segment(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> bool {
    let cross = (px - x1) * (y2 - y1) - (py - y1) * (x2 - x1);
    if cross.abs() > BOUNDARY_EPS {
        return false;
    }
    let dot = (px - x1) * (px - x2) + (py - y1) * (py - y2);
    dot <= BOUNDARY_EPS
}

/// 应用门控；任一坐标为 null（极端/无法变换信号）时排除并记录，绝不静默丢弃。
pub fn gate_events(
    events: &[EventPoint],
    channel_index: &[(String, usize)],
    gate: &GatePolygon,
    transforms: &[TransformSpec],
) -> GateOutcome {
    let xi = channel_index
        .iter()
        .find(|(c, _)| *c == gate.x_channel)
        .map(|(_, i)| *i);
    let yi = channel_index
        .iter()
        .find(|(c, _)| *c == gate.y_channel)
        .map(|(_, i)| *i);
    let (Some(xi), Some(yi)) = (xi, yi) else {
        return GateOutcome {
            included: vec![],
            boundary: vec![],
            excluded_unplottable: events.iter().map(|e| e.index).collect(),
        };
    };
    let tx = transforms.iter().find(|t| t.channel() == gate.x_channel);
    let ty = transforms.iter().find(|t| t.channel() == gate.y_channel);

    let mut included = Vec::new();
    let mut boundary = Vec::new();
    let mut excluded_unplottable = Vec::new();

    for ev in events {
        let raw_x = ev.values.get(xi).copied().flatten();
        let raw_y = ev.values.get(yi).copied().flatten();
        let x = match (raw_x, tx) {
            (Some(v), Some(spec)) => spec.apply(v),
            (Some(v), None) => Some(v),
            _ => None,
        };
        let y = match (raw_y, ty) {
            (Some(v), Some(spec)) => spec.apply(v),
            (Some(v), None) => Some(v),
            _ => None,
        };
        match (x, y) {
            (Some(x), Some(y)) => match point_in_polygon(x, y, &gate.vertices) {
                Containment::Inside => included.push(ev.index),
                Containment::Boundary => {
                    boundary.push(ev.index);
                    included.push(ev.index);
                }
                Containment::Outside => {}
            },
            _ => excluded_unplottable.push(ev.index),
        }
    }
    GateOutcome {
        included,
        boundary,
        excluded_unplottable,
    }
}

#[derive(Debug, Clone)]
pub struct GateOutcome {
    pub included: Vec<usize>,
    /// 同时计入 included，但保留边界身份用于审阅
    pub boundary: Vec<usize>,
    pub excluded_unplottable: Vec<usize>,
}

/// 钉住父群版本的百分比。POLICY-GATE-DENOM
#[derive(Debug, Clone, serde::Serialize)]
pub struct PinnedFraction {
    pub numerator: i64,
    pub denominator: i64,
    pub percent: f64,
    /// 分母来源：父群稳定身份 + 父结果版本 + 计数快照
    pub denominator_source: DenominatorSource,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DenominatorSource {
    pub parent_node_id: String,
    pub parent_result_version: String,
    pub parent_count_snapshot: i64,
    pub rule: String,
}

pub fn pinned_fraction(
    count: i64,
    parent_node_id: &str,
    parent_result_version: &str,
    parent_count_snapshot: i64,
) -> PinnedFraction {
    let percent = if parent_count_snapshot > 0 {
        100.0 * count as f64 / parent_count_snapshot as f64
    } else {
        f64::NAN
    };
    PinnedFraction {
        numerator: count,
        denominator: parent_count_snapshot,
        percent,
        denominator_source: DenominatorSource {
            parent_node_id: parent_node_id.to_string(),
            parent_result_version: parent_result_version.to_string(),
            parent_count_snapshot,
            rule: "POLICY-GATE-DENOM".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Vec<[f64; 2]> {
        vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]
    }

    #[test]
    fn interior_outside_and_boundary() {
        let p = square();
        assert_eq!(point_in_polygon(5.0, 5.0, &p), Containment::Inside);
        assert_eq!(point_in_polygon(-1.0, 5.0, &p), Containment::Outside);
        assert_eq!(point_in_polygon(10.0, 5.0, &p), Containment::Boundary);
        assert_eq!(point_in_polygon(0.0, 0.0, &p), Containment::Boundary);
    }

    #[test]
    fn fraction_denominator_is_pinned() {
        let f = pinned_fraction(30, "cells", "cells@v2:r7", 120);
        assert!((f.percent - 25.0).abs() < 1e-9);
        assert_eq!(f.denominator_source.parent_result_version, "cells@v2:r7");
        assert_eq!(f.denominator, 120);
        assert_eq!(f.denominator_source.rule, "POLICY-GATE-DENOM");
    }
}
