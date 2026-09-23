// 推导层：规范化只发生在这里。规则：
// R-NEAR-SINGULAR：近奇异补偿矩阵不能被默默求逆 -> 显式候选 + finding(blocked)。
// R-DENOM-PIN：门的百分比分母钉住父群版本。
// R-STALE：上游修订后下游结果标记过期而不覆盖。

use crate::model::{ArtifactContent, ArtifactRef, ChannelValue, CompMatrix, GateSpec, RawEvents, TransformSpec};

pub const RULE_NEAR_SINGULAR: &str = "R-NEAR-SINGULAR";
pub const RULE_DENOM_PIN: &str = "R-DENOM-PIN";
pub const RULE_STALE: &str = "R-STALE";
pub const RULE_BOUNDARY: &str = "R-BOUNDARY";
pub const RULE_EXTREME: &str = "R-EXTREME";
pub const RULE_UNCONVERTIBLE: &str = "R-UNCONVERTIBLE";

#[derive(Debug, Clone, PartialEq)]
pub struct InversionReport {
    pub inverse: Option<Vec<Vec<f64>>>,
    pub min_pivot: f64,
    pub reciprocal_condition: f64,
    pub threshold: f64,
    pub near_singular: bool,
}

/// 默认近奇异阈值（最小主元）。
pub const NEAR_SINGULAR_THRESHOLD: f64 = 1e-5;

/// 选主元 LU 求逆。返回最小主元与条件数倒数估计；不静默返回伪逆。
pub fn invert_matrix(m: &[Vec<f64>], threshold: f64) -> InversionReport {
    let n = m.len();
    let max_scale = m.iter().flat_map(|r| r.iter()).fold(1.0_f64, |a, &b| a.max(b.abs()));
    if n == 0 || m.iter().any(|r| r.len() != n) {
        return InversionReport { inverse: None, min_pivot: 0.0, reciprocal_condition: 0.0, threshold, near_singular: true };
    }
    let mut a: Vec<Vec<f64>> = m.to_vec();
    let mut inv: Vec<Vec<f64>> = (0..n).map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect()).collect();
    let mut perm: Vec<usize> = (0..n).collect();
    let mut min_pivot = f64::INFINITY;
    for col in 0..n {
        let mut pivot = col;
        let mut best = a[col][col].abs();
        for row in (col + 1)..n {
            if a[row][col].abs() > best {
                best = a[row][col].abs();
                pivot = row;
            }
        }
        if best < 1e-15 {
            return InversionReport { inverse: None, min_pivot: 0.0, reciprocal_condition: 0.0, threshold, near_singular: true };
        }
        if pivot != col {
            a.swap(col, pivot);
            inv.swap(col, pivot);
            perm.swap(col, pivot);
        }
        let piv = a[col][col];
        min_pivot = min_pivot.min(piv.abs() / max_scale.max(1.0));
        for row in 0..n {
            if row == col { continue; }
            let factor = a[row][col] / piv;
            if factor != 0.0 {
                for k in 0..n {
                    a[row][k] -= factor * a[col][k];
                    inv[row][k] -= factor * inv[col][k];
                }
            }
        }
    }
    for i in 0..n {
        let d = a[i][i];
        for j in 0..n {
            inv[i][j] /= d;
        }
    }
    let near_singular = min_pivot < threshold;
    InversionReport {
        inverse: if near_singular { None } else { Some(inv) },
        min_pivot,
        reciprocal_condition: min_pivot,
        threshold,
        near_singular,
    }
}

/// Tikhonov 正则化候选（显式、可追溯，仅作为候选之一）。
pub fn regularized_inverse(m: &[Vec<f64>], lambda: f64) -> Option<Vec<Vec<f64>>> {
    let n = m.len();
    let mut ata = vec![vec![0.0_f64; n]; n];
    for r in 0..n {
        for c in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += m[k][r] * m[k][c];
            }
            ata[r][c] = s + if r == c { lambda * lambda } else { 0.0 };
        }
    }
    let rep = invert_matrix(&ata, 0.0);
    let inv_ata = rep.inverse?;
    let mut out = vec![vec![0.0_f64; n]; n];
    for r in 0..n {
        for c in 0..n {
            let mut s = 0.0;
            for k in 0..n {
                s += inv_ata[r][k] * m[c][k];
            }
            out[r][c] = s;
        }
    }
    Some(out)
}

#[derive(Debug, Clone)]
pub struct CompResult {
    /// 每事件每补偿通道的补偿后值（None 表示无法计算，保留原始值+诊断）。
    pub values: Vec<Vec<Option<f64>>>,
    pub diagnostics: Vec<(u32, String, String)>,
    pub report: InversionReport,
}

/// 应用补偿：compensated = inv(S) * measured。非有限/缺失值不参与、不丢弃。
pub fn apply_compensation(raw: &RawEvents, comp: &CompMatrix, threshold: f64) -> CompResult {
    let report = invert_matrix(&comp.matrix, threshold);
    let n = comp.channels.len();
    let mut values = Vec::with_capacity(raw.events.len());
    let mut diagnostics = Vec::new();
    for ev in &raw.events {
        let mut row = vec![None; n];
        if let Some(inv) = &report.inverse {
            let mut measured = vec![0.0_f64; n];
            let mut usable = true;
            for (i, ch) in comp.channels.iter().enumerate() {
                match ev.channels.iter().find(|c| &c.channel == ch) {
                    Some(ChannelValue { value: Some(v), .. }) if v.is_finite() => measured[i] = *v,
                    Some(ChannelValue { raw_token, .. }) => {
                        usable = false;
                        diagnostics.push((ev.index, ch.clone(), format!("unconvertible raw token {raw_token:?}; left in place during compensation")));
                    }
                    None => {
                        usable = false;
                        diagnostics.push((ev.index, ch.clone(), "channel missing".into()));
                    }
                }
            }
            if usable {
                for r in 0..n {
                    let mut s = 0.0;
                    for c in 0..n {
                        s += inv[r][c] * measured[c];
                    }
                    row[r] = Some(s);
                    if s < 0.0 {
                        diagnostics.push((ev.index, comp.channels[r].clone(), format!("negative compensated signal {s:.3} (extreme/over-subtraction preserved)")));
                    }
                }
            }
        }
        values.push(row);
    }
    CompResult { values, diagnostics, report }
}

/// asinh 双曲反正弦变换：transformed = asinh(x / cofactor)。x 为 None 时保持 None。
pub fn transform_value(x: Option<f64>, t: &crate::model::ChannelTransform) -> Option<f64> {
    match t.method.as_str() {
        "linear" => x,
        "asinh" => x.map(|v| {
            let cf = t.cofactor.unwrap_or(150.0).max(1e-12);
            (v / cf).asinh()
        }),
        other => {
            let _ = other;
            x
        }
    }
}

pub fn transform_all(raw: &RawEvents, comp_values: &[Vec<Option<f64>>], comp: &CompMatrix, spec: &TransformSpec) -> Vec<Vec<(String, Option<f64>, String, String)>> {
    raw.events
        .iter()
        .enumerate()
        .map(|(ei, ev)| {
            ev.channels
                .iter()
                .map(|cv| {
                    let v = if let Some(pos) = comp.channels.iter().position(|c| c == &cv.channel) {
                        comp_values.get(ei).and_then(|r| r[pos])
                    } else {
                        cv.value
                    };
                    let t = spec.channels.get(&cv.channel);
                    let out = match t {
                        Some(t) => transform_value(v, t),
                        None => v,
                    };
                    let unit = t.map(|t| t.unit.clone()).unwrap_or_else(|| cv.unit.clone());
                    let basis = t.map(|t| format!("{} via {}", cv.basis, t.basis)).unwrap_or_else(|| cv.basis.clone());
                    (cv.channel.clone(), out, unit, basis)
                })
                .collect()
        })
        .collect()
}

/// 点在多边形内；边界点（顶点/边）计入门内 —— R-BOUNDARY。
pub fn point_in_gate(x: f64, y: f64, gate: &GateSpec) -> bool {
    let v = &gate.vertices;
    if v.len() < 3 {
        return false;
    }
    // 先判边界（含顶点与边，含数值公差）。
    for i in 0..v.len() {
        let a = v[i];
        let b = v[(i + 1) % v.len()];
        if on_segment(x, y, a, b) {
            return true;
        }
    }
    let mut inside = false;
    let mut j = v.len() - 1;
    for i in 0..v.len() {
        let (xi, yi) = (v[i][0], v[i][1]);
        let (xj, yj) = (v[j][0], v[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn on_segment(px: f64, py: f64, a: [f64; 2], b: [f64; 2]) -> bool {
    let cross = (px - a[0]) * (b[1] - a[1]) - (py - a[1]) * (b[0] - a[0]);
    let tol = 1e-9_f64;
    if cross.abs() > tol * 10.0 {
        return false;
    }
    let dot = (px - a[0]) * (px - b[0]) + (py - a[1]) * (py - b[1]);
    dot <= 1e-9
}

/// 群结果（不可覆盖；版本修订时产生新行，旧行仍可读）。
#[derive(Debug, Clone)]
pub struct PopulationResult {
    pub node: String,
    pub gate_ref: String,
    pub transform_ref: String,
    pub comp_ref: String,
    pub parent_node: Option<String>,
    /// 分母所钉住的父群版本（事件序号 + 结果引用）。
    pub parent_result_ref: String,
    pub member_events: Vec<u32>,
    pub count: u32,
    pub parent_count: u32,
    pub percent_of_parent: f64,
    pub percent_unit: String,
    pub boundary_events: Vec<u32>,
    pub extreme_events: Vec<u32>,
    pub excluded_unconvertible: Vec<u32>,
}

/// 在给定（已解析的）依赖下计算一个群。
pub fn evaluate_gate(
    node: &str,
    gate: &GateSpec,
    coords: &[Vec<(String, Option<f64>, String, String)>],
    raw: &RawEvents,
) -> (Vec<u32>, Vec<u32>, Vec<u32>, Vec<u32>) {
    let mut members = Vec::new();
    let mut boundary = Vec::new();
    let mut extreme = Vec::new();
    let mut excluded = Vec::new();
    for (ei, row) in coords.iter().enumerate() {
        let x = row.iter().find(|(c, _, _, _)| c == &gate.x_channel);
        let y = row.iter().find(|(c, _, _, _)| c == &gate.y_channel);
        let (xv, yv) = match (x, y) {
            (Some((_, Some(a), _, _)), Some((_, Some(b), _, _))) => (*a, *b),
            _ => {
                excluded.push(ei as u32);
                continue;
            }
        };
        let in_gate = point_in_gate(xv, yv, gate);
        if in_gate {
            members.push(ei as u32);
        }
        // 边界事件（恰好落在顶点/边上，无论门内都标注；此处门内含边界）。
        for verts in gate.vertices.windows(2).chain(std::iter::once([gate.vertices[0], *gate.vertices.last().unwrap()]).as_slice().windows(2)) {
            let _ = verts;
        }
        for i in 0..gate.vertices.len() {
            let a = gate.vertices[i];
            let b = gate.vertices[(i + 1) % gate.vertices.len()];
            if on_segment(xv, yv, a, b) {
                boundary.push(ei as u32);
                break;
            }
        }
        // 极端信号：原始 ADC 饱和或补偿后为负。
        let ev = &raw.events[ei];
        let is_extreme = ev.channels.iter().any(|c| {
            c.diagnostics.iter().any(|d| d.contains("outside") || d.contains("non-finite"))
        }) || xv < 0.0 || yv < 0.0;
        if is_extreme {
            extreme.push(ei as u32);
        }
    }
    (members, dedup(boundary), dedup(extreme), dedup(excluded))
}

fn dedup(mut v: Vec<u32>) -> Vec<u32> {
    v.sort_unstable();
    v.dedup();
    v
}

/// 从工件集合抽取已解析内容。
pub fn parse_artifacts(arts: &[(ArtifactRef, ArtifactContent)]) -> ParsedDeps<'_> {
    let mut raw = None;
    let mut comp = None;
    let mut transform = None;
    let mut gates = std::collections::BTreeMap::new();
    let mut hierarchy = None;
    for (_, c) in arts {
        match c {
            ArtifactContent::Raw(r) => raw = Some(r),
            ArtifactContent::Comp(c) => comp = Some(c),
            ArtifactContent::Transform(t) => transform = Some(t),
            ArtifactContent::Gate(g) => {
                gates.insert(g.node.clone(), g);
            }
            ArtifactContent::Hierarchy(h) => hierarchy = Some(h),
            _ => {}
        }
    }
    ParsedDeps { raw, comp, transform, gates, hierarchy }
}

pub struct ParsedDeps<'a> {
    pub raw: Option<&'a RawEvents>,
    pub comp: Option<&'a CompMatrix>,
    pub transform: Option<&'a TransformSpec>,
    pub gates: std::collections::BTreeMap<String, &'a GateSpec>,
    pub hierarchy: Option<&'a crate::model::HierarchySpec>,
}
