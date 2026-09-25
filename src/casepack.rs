//! 离线案例包："流式边界室：补偿矩阵、门控系谱与细胞群不确定性"。
//! 不是通用表格：所有事件、矩阵、门坐标、spillover 与场景脚本都是流式领域材料。
//!
//! 固定案例实际经过：矩阵奇异、极端信号、边界点、门嵌套改名、
//! 上游过期传播、同版本并发修改；同时保留被采纳与未采纳分支。

use crate::comp::{CompMatrix, SingleStainRef, SpilloverMeta};
use crate::fcs::{write_fcs_float, FcsChannel};
use crate::gate::{GateDef, GateShape, Point};
use crate::lineage::{LineageDoc, LineageEdge, LineageNode};
use crate::materials::GateWrapper;
use crate::transform::{ChannelTransform, TransformSpec};

pub const CASE_ID: &str = "flow-boundary-suite";

#[derive(Clone, Copy)]
pub struct CaseEvent {
    pub fitc: f32,
    pub pe: f32,
    pub percp: f32,
}

/// 10 个事件：含极端低值、溢出、门边界点。
pub fn events() -> Vec<CaseEvent> {
    vec![
        CaseEvent { fitc: 100.0, pe: 120.0, percp: 90.0 },    // 0 噪声
        CaseEvent { fitc: 900.0, pe: 200.0, percp: 110.0 },   // 1 CD3 阳性
        CaseEvent { fitc: 1200.0, pe: 950.0, percp: 130.0 },  // 2 CD4 双阳
        CaseEvent { fitc: 1100.0, pe: 300.0, percp: 900.0 },  // 3 CD8（CD3+ 非 CD4）
        CaseEvent { fitc: 300.0, pe: 900.0, percp: 120.0 },   // 4 PE 单阳（落在 CD3 多边形外）
        CaseEvent { fitc: 950.0, pe: 450.0, percp: 140.0 },   // 5 CD3 边界点（边上）
        CaseEvent { fitc: 1300.0, pe: 1000.0, percp: 450.0 }, // 6 CD4 边界点（边上）
        CaseEvent { fitc: -50.0, pe: 80.0, percp: 70.0 },     // 7 极端负基线
        CaseEvent { fitc: 500.0, pe: f32::INFINITY, percp: 100.0 }, // 8 PE 溢出
        CaseEvent { fitc: 250.0, pe: 350.0, percp: 400.0 },   // 9 噪声
    ]
}

pub fn channel_specs() -> Vec<(String, Option<String>)> {
    vec![
        ("FITC-A".to_string(), Some("CD3".to_string())),
        ("PE-A".to_string(), Some("CD4".to_string())),
        ("PerCP-A".to_string(), Some("CD8".to_string())),
    ]
}

pub fn fcs_bytes() -> Vec<u8> {
    let chans: Vec<FcsChannel> = channel_specs()
        .iter()
        .map(|(n, m)| FcsChannel { name: n.clone(), marker: m.clone(), range: "262144".into(), gain: Some("1".into()) })
        .collect();
    let specs = channel_specs();
    let evs = events();
    let rows: Vec<Vec<f32>> = evs.iter().map(|e| vec![e.fitc, e.pe, e.percp]).collect();
    let _ = chans;
    write_fcs_float(&specs, &[262144, 262144, 262144], &rows)
}

pub fn spillover() -> SpilloverMeta {
    SpilloverMeta {
        channels: vec!["FITC-A".into(), "PE-A".into(), "PerCP-A".into()],
        single_stain_refs: vec![
            SingleStainRef { channel: "FITC-A".into(), material_version_id: "control:cd3:v1".into(), control_label: "CD3 single stain".into() },
            SingleStainRef { channel: "PE-A".into(), material_version_id: "control:cd4:v1".into(), control_label: "CD4 single stain".into() },
            SingleStainRef { channel: "PerCP-A".into(), material_version_id: "control:cd8:v1".into(), control_label: "CD8 single stain".into() },
        ],
        source_keyword: Some("$SPILLOVER,3,3,FITC-A,PE-A,PerCP-A,1,0.02,0.01,0.06,1,0.03,0.02,0.05,1".into()),
    }
}

/// 良态补偿矩阵（基线）。
pub fn good_matrix() -> CompMatrix {
    CompMatrix {
        channels: vec!["FITC-A".into(), "PE-A".into(), "PerCP-A".into()],
        rows: vec![
            vec![1.0, 0.02, 0.01],
            vec![0.06, 1.0, 0.03],
            vec![0.02, 0.05, 1.0],
        ],
    }
}

/// 近奇异矩阵（参数修订）：第二、三行在 FITC 方向几乎共线，保留多候选。
pub fn near_singular_matrix() -> CompMatrix {
    CompMatrix {
        channels: vec!["FITC-A".into(), "PE-A".into(), "PerCP-A".into()],
        rows: vec![
            vec![1.0, 0.02, 0.01],
            vec![0.99999, 1.0, 0.03],
            vec![0.99998, 0.05, 1.0],
        ],
    }
}

/// 严格奇异矩阵（第二分支，未采纳）：第二行是第一行的 2 倍。
pub fn singular_matrix() -> CompMatrix {
    CompMatrix {
        channels: vec!["FITC-A".into(), "PE-A".into(), "PerCP-A".into()],
        rows: vec![
            vec![1.0, 0.5, 0.25],
            vec![2.0, 1.0, 0.5],
            vec![0.2, 0.3, 1.0],
        ],
    }
}

pub fn transforms() -> Vec<ChannelTransform> {
    vec![
        ChannelTransform::new("FITC-A", TransformSpec::Asinh { offset: 0.0, cofactor: 150.0 }),
        ChannelTransform::new("PE-A", TransformSpec::Asinh { offset: 0.0, cofactor: 150.0 }),
        ChannelTransform::new("PerCP-A", TransformSpec::Asinh { offset: 0.0, cofactor: 150.0 }),
    ]
}

/// 变换参数修订：FITC cofactor 调整（只影响以 FITC 为坐标的门——局部失效）。
pub fn transforms_v2() -> Vec<ChannelTransform> {
    vec![
        ChannelTransform::new("FITC-A", TransformSpec::Asinh { offset: 0.0, cofactor: 300.0 }),
        ChannelTransform::new("PE-A", TransformSpec::Asinh { offset: 0.0, cofactor: 150.0 }),
        ChannelTransform::new("PerCP-A", TransformSpec::Asinh { offset: 0.0, cofactor: 150.0 }),
    ]
}

/// 事件 5（fitc 950, pe 450）恰在 CD3 门的 x=1.0 竖边上（边界点反例）。
pub fn gate_cd3() -> GateDef {
    GateDef {
        x_basis: "transform:asinh:channel:FITC-A".into(),
        y_basis: "transform:asinh:channel:PE-A".into(),
        shape: GateShape::Polygon {
            vertices: vec![
                Point { x: 1.0, y: 0.0 },
                Point { x: 4.0, y: 0.0 },
                Point { x: 4.0, y: 2.5 },
                Point { x: 1.0, y: 2.5 },
            ],
        },
        boundary_inclusive: false,
    }
}

/// 上游门修订（局部源数据改动）：左边界从 1.0 改到 1.05，事件 5 被排除。
pub fn gate_cd3_v2() -> GateDef {
    let mut g = gate_cd3();
    g.shape = GateShape::Polygon {
        vertices: vec![
            Point { x: 1.05, y: 0.0 },
            Point { x: 4.0, y: 0.0 },
            Point { x: 4.0, y: 2.5 },
            Point { x: 1.05, y: 2.5 },
        ],
    };
    g
}

/// CD4 门：事件 6（fitc 1300, pe 1000）恰在下边 y≈1.227 上（边界点）。
pub fn gate_cd4() -> GateDef {
    GateDef {
        x_basis: "transform:asinh:channel:FITC-A".into(),
        y_basis: "transform:asinh:channel:PE-A".into(),
        shape: GateShape::Polygon {
            vertices: {
                let (x_hi, _) = crate::transform::ChannelTransform::new(
                    "FITC-A",
                    TransformSpec::Asinh { offset: 0.0, cofactor: 150.0 },
                )
                .apply_with_diagnostics(1300.0);
                let (y_edge, _) = crate::transform::ChannelTransform::new(
                    "PE-A",
                    TransformSpec::Asinh { offset: 0.0, cofactor: 150.0 },
                )
                .apply_with_diagnostics(1000.0);
                vec![
                    Point { x: 1.2, y: y_edge },
                    Point { x: x_hi + 0.3, y: y_edge },
                    Point { x: x_hi + 0.3, y: 2.0 },
                    Point { x: 1.2, y: 2.0 },
                ]
            },
        },
        boundary_inclusive: false,
    }
}

pub fn gate_cd8() -> GateDef {
    GateDef {
        x_basis: "transform:asinh:channel:FITC-A".into(),
        y_basis: "transform:asinh:channel:PerCP-A".into(),
        shape: GateShape::Polygon {
            vertices: vec![
                Point { x: 1.0, y: 1.0 },
                Point { x: 4.0, y: 1.0 },
                Point { x: 4.0, y: 3.0 },
                Point { x: 1.0, y: 3.0 },
            ],
        },
        boundary_inclusive: false,
    }
}

/// 门树：root -> lymphocytes -> cd3 -> {cd4, cd8}。
/// 门嵌套改名场景：节点 id 不变，只改 label。
pub fn lineage() -> LineageDoc {
    LineageDoc {
        nodes: vec![
            LineageNode::Root { id: "all".into(), label: "All events".into() },
            LineageNode::Gate { id: "lym", label: "Lymphocytes".into(), gate_material_version_id: "PENDING:lym".into() },
            LineageNode::Gate { id: "cd3", label: "CD3+".into(), gate_material_version_id: "PENDING:cd3".into() },
            LineageNode::Gate { id: "cd4", label: "CD4+".into(), gate_material_version_id: "PENDING:cd4".into() },
            LineageNode::Gate { id: "cd8", label: "CD8+".into(), gate_material_version_id: "PENDING:cd8".into() },
        ],
        edges: vec![
            LineageEdge { parent: "all".into(), child: "lym".into() },
            LineageEdge { parent: "lym".into(), child: "cd3".into() },
            LineageEdge { parent: "cd3".into(), child: "cd4".into() },
            LineageEdge { parent: "cd3".into(), child: "cd8".into() },
        ],
    }
}

/// lymphocytes 门：宽松矩形，覆盖全部有效事件（极端负/溢出事件由根群排除）。
pub fn gate_lym() -> GateDef {
    GateDef {
        x_basis: "transform:asinh:channel:FITC-A".into(),
        y_basis: "transform:asinh:channel:PE-A".into(),
        shape: GateShape::Rectangle { x0: -0.2, x1: 4.0, y0: -0.2, y1: 4.0 },
        boundary_inclusive: false,
    }
}

pub fn gate_wrapper(gate_id: &str, def: GateDef) -> GateWrapper {
    GateWrapper { gate_id: gate_id.into(), def }
}

/// 分幕脚本中使用的稳定名字。
pub mod scene {
    pub const BRANCH_MAIN: &str = "draft-a";
    pub const BRANCH_SINGULAR: &str = "draft-b-singular";
}
