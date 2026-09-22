//! 固定离线案例包：“流式边界室：补偿矩阵、门控系谱与细胞群不确定性”。
//!
//! 材料是领域量（通道/事件/spillover/门），不是通用表格。
//! 事件值经过设计，使 G2 边界上恰好存在边界点，且包含极端信号与不可转换表示。

use crate::model::*;
use serde_json::json;

const COFACTOR: f64 = 150.0;
const SCALE: f64 = 5.0;

pub fn asinh(x: f64) -> f64 {
    (x / COFACTOR).asinh() * SCALE
}

fn cell(ch: &str, raw: &str) -> RawCell {
    parse_raw_cell(ch, raw, "linear_counts")
}

/// 构造基线案例包。raw 三元组 = (FITC, PE, APC)；FSC/SSC 另外给出。
pub fn baseline_case() -> CasePack {
    let channels = vec![
        Channel { id: "FSC".into(), label: "FSC-A".into(), fluorophore: None, unit: "linear_counts".into() },
        Channel { id: "SSC".into(), label: "SSC-A".into(), fluorophore: None, unit: "linear_counts".into() },
        Channel { id: "FITC".into(), label: "FITC-A".into(), fluorophore: Some("FITC".into()), unit: "linear_counts".into() },
        Channel { id: "PE".into(), label: "PE-A".into(), fluorophore: Some("PE".into()), unit: "linear_counts".into() },
        Channel { id: "APC".into(), label: "APC-A".into(), fluorophore: Some("APC".into()), unit: "linear_counts".into() },
    ];

    // 原始 (FITC,PE,APC) 线性计数。E04 取在 G2 边 y=14.353 上（边界点）。
    let triples: &[(&str, f64, f64, f64, &str, &str)] = &[
        ("E01", 200.0, 150.0, 120.0, "350", "120"),
        ("E02", 180.0, 160.0, 110.0, "340", "110"),
        ("E03", 150.0, 140.0, 130.0, "330", "130"),
        ("E04", 6000.0, 1300.0, 100.0, "420", "90"),
        ("E05", 8000.0, 2000.0, 150.0, "430", "95"),
        ("E06", 7000.0, 1500.0, 120.0, "410", "100"),
        ("E07", 6500.0, 1600.0, 2500.0, "400", "110"),
        ("E08", 7500.0, 1900.0, 3000.0, "415", "105"),
        ("E09", 500.0, 400.0, 300.0, "300", "200"),
        ("E10", 260000.0, 240000.0, 250000.0, "500", "250"),
        ("E11", -50.0, -20.0, -10.0, "320", "80"),
        ("E12", 5800.0, 1200.0, 90.0, "620", "310"),
        ("E13", 900.0, 800.0, 600.0, "280", "60"),
        ("E14", 0.0, 0.0, 0.0, "360", "85"),
        ("E15", 6200.0, 1550.0, 105.0, "390", "120"),
    ];

    let mut raw_events = Vec::new();
    for (id, fitc, pe, apc, fsc, ssc) in triples {
        let values = vec![
            cell("FSC", fsc),
            cell("SSC", ssc),
            cell("FITC", &fitc.to_string()),
            cell("PE", &pe.to_string()),
            cell("APC", &apc.to_string()),
        ];
        raw_events.push(RawEvent { id: id.to_string(), values });
    }
    // E14 的 FITC 使用仪器溢出标记：无法转换，留在原位置并附诊断。
    if let Some(ev) = raw_events.iter_mut().find(|e| e.id == "E14") {
        ev.values[2] = cell("FITC", "OVRFLW");
    }

    // 良态 spillover（基线）。行=检测通道，列=染料。
    let spillover = SpilloverSpec {
        id: "SPL-BASE".into(),
        channels: vec!["FITC".into(), "PE".into(), "APC".into()],
        matrix: vec![
            vec![1.0, 0.12, 0.0],
            vec![0.08, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ],
        single_stain_refs: vec!["SS-FITC".into(), "SS-PE".into(), "SS-APC".into()],
    };

    let transforms = vec![
        TransformSpec { channel: "FITC".into(), basis: "asinh_compensated".into(), cofactor: COFACTOR, scale: SCALE },
        TransformSpec { channel: "PE".into(), basis: "asinh_compensated".into(), cofactor: COFACTOR, scale: SCALE },
        TransformSpec { channel: "APC".into(), basis: "asinh_compensated".into(), cofactor: COFACTOR, scale: SCALE },
    ];

    // 门顶点全部以 asinh_compensated 坐标给出（已对基线补偿后的设计事件取值）。
    let g2_y_edge = asinh(820.0); // ≈14.353：E04 补偿后 PE 恰在边上
    let gates = vec![
        GateDef {
            id: "G1".into(),
            name: "淋巴细胞门".into(),
            parent: None,
            x_channel: "FSC".into(),
            y_channel: "SSC".into(),
            vertices_basis: "linear_counts".into(),
            vertices: vec![[300.0, 50.0], [500.0, 50.0], [500.0, 160.0], [300.0, 160.0]],
        },
        GateDef {
            id: "G2".into(),
            name: "CD3+".into(),
            parent: Some("G1".into()),
            x_channel: "FITC".into(),
            y_channel: "PE".into(),
            vertices_basis: "asinh_compensated".into(),
            vertices: vec![
                [15.0, g2_y_edge],
                [25.0, g2_y_edge],
                [25.0, 25.0],
                [15.0, 25.0],
            ],
        },
        GateDef {
            id: "G3".into(),
            name: "CD4+".into(),
            parent: Some("G2".into()),
            x_channel: "APC".into(),
            y_channel: "FITC".into(),
            vertices_basis: "asinh_compensated".into(),
            vertices: vec![[15.0, 15.0], [25.0, 15.0], [25.0, 25.0], [15.0, 25.0]],
        },
    ];

    CasePack {
        case_id: "flow-boundary-chamber-v1".into(),
        title: "流式边界室：补偿矩阵、门控系谱与细胞群不确定性".into(),
        channels,
        raw_events,
        spillover,
        transforms,
        gates,
    }
}

/// 奇异 spillover（第三列为第一、二列线性组合），用于 singular 场景。
pub fn singular_spillover() -> serde_json::Value {
    json!({
        "id": "SPL-SINGULAR",
        "channels": ["FITC", "PE", "APC"],
        "matrix": [
            [1.0, 0.2, 1.2],
            [0.1, 1.0, 1.1],
            [0.0, 0.0, 0.0]
        ],
        "single_stain_refs": ["SS-FITC", "SS-PE", "SS-APC"]
    })
}

/// 近奇异 spillover（前两列几乎共线），保留多个合法补偿候选。
pub fn near_singular_spillover() -> serde_json::Value {
    json!({
        "id": "SPL-NEAR",
        "channels": ["FITC", "PE", "APC"],
        "matrix": [
            [1.0, 0.9999, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 0.0, 1.0]
        ],
        "single_stain_refs": ["SS-FITC", "SS-PE", "SS-APC"]
    })
}
