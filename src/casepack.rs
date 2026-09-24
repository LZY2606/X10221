//! 仓库自带离线案例包：「流式边界室：补偿矩阵、门控系谱与细胞群不确定性」。
//!
//! 这是领域材料，不是通用表格：事件坐标围绕近奇异补偿、极端信号、边界点、
//! 门嵌套改名、上游失效传播与同版本并发冲突设计。FCS 字节由 [`crate::domain::fcs`]
//! 生成，随后走与用户导入完全相同的解析路径。

use std::collections::BTreeMap;

use crate::domain::fcs::write_fcs_3_1;

#[derive(Debug, Clone)]
pub enum TransformIn {
    Linear { offset: f64, scale: f64, unit: String },
    ArcSinh { cofactor: f64, unit: String },
}

#[derive(Debug, Clone)]
pub struct GateDraft {
    pub gate_id: String,
    pub name: String,
    pub parent: Option<String>,
    pub x_channel: String,
    pub y_channel: String,
    pub polygon: Vec<(f64, f64)>,
    pub ordinal: u64,
}

#[derive(Debug, Clone)]
pub struct SpilloverDraft {
    pub stable_id: String,
    pub channels: Vec<String>,
    pub matrix: Vec<Vec<f64>>,
    pub single_stain_blob_ids: Vec<String>,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct ExtraBlob {
    pub blob_id: String,
    pub kind: String,
    pub filename: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CasePack {
    pub case_id: String,
    pub title: String,
    pub fcs_blob_id: String,
    pub fcs_filename: String,
    pub fcs_bytes: Vec<u8>,
    pub extra_blobs: Vec<ExtraBlob>,
    pub channel_units: BTreeMap<String, String>,
    pub spillover: SpilloverDraft,
    pub matrix_stable_id: String,
    pub transforms: BTreeMap<String, TransformIn>,
    pub gates: Vec<GateDraft>,
}

const CH: &[&str] = &["FSC-A", "SSC-A", "FITC-A", "PE-A", "APC-A"];

/// 固定事件集（原始通道值）。
///
/// 覆盖：碎屑、细胞群、CD3 边界点、饱和极端信号、负极端、NaN 不可变换值。
fn events() -> Vec<Vec<f32>> {
    vec![
        // FSC, SSC, FITC(CD3), PE(CD4), APC(CD8)
        vec![20.0, 15.0, 80.0, 60.0, 40.0],        // 1 debris（Cells 外）
        vec![120.0, 110.0, 90.0, 70.0, 50.0],      // 2 cell CD3-（CD4/CD8 低）
        vec![300.0, 260.0, 95.0, 85.0, 60.0],      // 3 cell CD3-
        vec![450.0, 400.0, 1500.0, 120.0, 90.0],   // 4 CD3+ CD4+
        vec![520.0, 430.0, 1800.0, 1400.0, 100.0], // 5 CD3+ CD4+
        vec![610.0, 470.0, 2100.0, 1700.0, 110.0], // 6 CD3+ CD4+
        vec![680.0, 520.0, 2400.0, 130.0, 1600.0], // 7 CD3+ CD8+
        vec![720.0, 560.0, 2600.0, 140.0, 1900.0], // 8 CD3+ CD8+
        vec![400.0, 350.0, 1000.0, 150.0, 120.0],  // 9 CD3+ 双阴
        vec![500.0, 420.0, 500.0, 1100.0, 80.0],   // 10 CD3 边界附近但在阈值外（FITC 500）
        vec![550.0, 700.0, 1500.0, 0.0, 0.0],      // 11 CD3 门顶边 y=700 上的精确边界点
        vec![640.0, 480.0, 262_143.0, 1800.0, 120.0], // 12 极端饱和信号（18-bit ADC 上限）
        vec![470.0, 390.0, -9000.0, 90.0, 70.0],   // 13 负极端（补偿放大后更明显）
        vec![580.0, 440.0, 2000.0, 1500.0, f32::NAN], // 14 APC=NaN：原始保留，不可变换
    ]
}

pub fn boundary_chamber() -> CasePack {
    let channels: Vec<String> = CH.iter().map(|s| s.to_string()).collect();
    let labels = vec![
        "FSC, linear units".to_string(),
        "SSC, linear units".to_string(),
        "CD3-FITC fluorescence".to_string(),
        "CD4-PE fluorescence".to_string(),
        "CD8-APC fluorescence".to_string(),
    ];
    let fcs_bytes = write_fcs_3_1(
        &channels,
        &labels,
        &events(),
        &BTreeMap::from([
            ("$CYT".to_string(), "FlowLine BoundaryChamber".to_string()),
            ("$SMNO".to_string(), "BC-2026-0919".to_string()),
            ("$DATE".to_string(), "2026-09-19".to_string()),
        ]),
    );

    let mut channel_units = BTreeMap::new();
    channel_units.insert("FSC-A".to_string(), "linear-units".to_string());
    channel_units.insert("SSC-A".to_string(), "linear-units".to_string());
    channel_units.insert("FITC-A".to_string(), "a.u.".to_string());
    channel_units.insert("PE-A".to_string(), "a.u.".to_string());
    channel_units.insert("APC-A".to_string(), "a.u.".to_string());

    // 单染对照文本 blob（机器可读 JSON，记录每条单染的荧光通道与事件来源）。
    let single_stain = br#"{
  "single_stain_controls": [
    {"id": "ss#cd3-fitc", "fluorochrome": "FITC", "channel": "FITC-A", "source": "beads+cells", "events_ref": "fcs#boundary-chamber"},
    {"id": "ss#cd4-pe", "fluorochrome": "PE", "channel": "PE-A", "source": "beads+cells", "events_ref": "fcs#boundary-chamber"},
    {"id": "ss#cd8-apc", "fluorochrome": "APC", "channel": "APC-A", "source": "beads+cells", "events_ref": "fcs#boundary-chamber"}
  ]
}"#.to_vec();

    // spillover 草案 blob（平台导入的原始草稿，与领域层 spillover 版本分开保存）。
    let spill_draft = br#"$SPILL keyword draft
FITC-A,PE-A,APC-A
1,0.02,0.01
0.999999,0.0200001,0.0100001
0.02,0.04,1
note: rows 1 and 2 nearly collinear (near-singular)"#.to_vec();

    // 门控草案 blob（平台原始草案）。
    let gate_draft = br#"{
  "draft_name": "panel-v0",
  "gates": [
    {"id": "gate#cells", "name": "Cells", "parent": null, "axes": ["FSC-A", "SSC-A"]},
    {"id": "gate#cd3", "name": "CD3+", "parent": "gate#cells", "axes": ["FITC-A", "SSC-A"]},
    {"id": "gate#cd4", "name": "CD4+", "parent": "gate#cd3", "axes": ["FITC-A", "PE-A"]},
    {"id": "gate#cd8", "name": "CD8+", "parent": "gate#cd3", "axes": ["APC-A", "PE-A"]}
  ]
}"#.to_vec();

    // 初始版本使用条件良好的 spillover（近奇异版本作为“参数修订”在第二个场景引入）。
    let spillover = SpilloverDraft {
        stable_id: "spill#panel".to_string(),
        channels: vec!["FITC-A".into(), "PE-A".into(), "APC-A".into()],
        matrix: vec![
            vec![1.0, 0.02, 0.01],
            vec![0.05, 1.0, 0.03],
            vec![0.02, 0.04, 1.0],
        ],
        single_stain_blob_ids: vec!["blob#single-stains".into()],
        note: "单染对照导出：CD3-FITC / CD4-PE / CD8-APC；条件良好的初始面板".into(),
    };

    let mut transforms = BTreeMap::new();
    transforms.insert("FSC-A".to_string(), TransformIn::Linear { offset: 0.0, scale: 1.0, unit: "linear-units".into() });
    transforms.insert("SSC-A".to_string(), TransformIn::Linear { offset: 0.0, scale: 1.0, unit: "linear-units".into() });
    for c in ["FITC-A", "PE-A", "APC-A"] {
        transforms.insert(c.to_string(), TransformIn::ArcSinh { cofactor: 150.0, unit: "a.u.".into() });
    }

    let gates = vec![
        GateDraft {
            gate_id: "gate#cells".into(),
            name: "Cells".into(),
            parent: None,
            x_channel: "FSC-A".into(),
            y_channel: "SSC-A".into(),
            // 事件 2..14 在内，事件 1 碎屑在外
            polygon: vec![(50.0, 50.0), (900.0, 50.0), (900.0, 800.0), (50.0, 800.0)],
            ordinal: 1,
        },
        GateDraft {
            gate_id: "gate#cd3".into(),
            name: "CD3+".into(),
            parent: Some("gate#cells".into()),
            x_channel: "FITC-A".into(),
            y_channel: "SSC-A".into(),
            // 变换坐标：asinh(FITC/150) 横轴。阈值 x=1500 → asinh(10)≈2.998；
            // 500 → asinh(3.33)≈1.91；1000 → asinh(6.67)≈2.593（边界点）。
            // y 为线性 SSC；顶边 y=700 精确穿过事件 11（边界点不静默归类）。
            polygon: vec![(2.9, 0.0), (15.0, 0.0), (15.0, 700.0), (2.9, 700.0)],
            ordinal: 1,
        },
        GateDraft {
            gate_id: "gate#cd4".into(),
            name: "CD4+".into(),
            parent: Some("gate#cd3".into()),
            x_channel: "FITC-A".into(),
            y_channel: "PE-A".into(),
            // PE 阈值 asinh(900/150)≈2.492；CD8 群 PE≈130-140 → asinh < 0.85。
            polygon: vec![(2.9, 2.4), (15.0, 2.4), (15.0, 15.0), (2.9, 15.0)],
            ordinal: 1,
        },
        GateDraft {
            gate_id: "gate#cd8".into(),
            name: "CD8+".into(),
            parent: Some("gate#cd3".into()),
            x_channel: "APC-A".into(),
            y_channel: "PE-A".into(),
            // APC 阈值同样约 2.492；事件 14 的 APC=NaN → unknown，不丢弃。
            polygon: vec![(2.4, -5.0), (15.0, -5.0), (15.0, 15.0), (2.4, 15.0)],
            ordinal: 1,
        },
    ];

    CasePack {
        case_id: "boundary-chamber-2026".into(),
        title: "流式边界室：补偿矩阵、门控系谱与细胞群不确定性".into(),
        fcs_blob_id: "fcs#boundary-chamber".into(),
        fcs_filename: "boundary-chamber.fcs".into(),
        fcs_bytes,
        extra_blobs: vec![
            ExtraBlob { blob_id: "blob#single-stains".into(), kind: "single-stains".into(), filename: "single-stain-controls.json".into(), bytes: single_stain },
            ExtraBlob { blob_id: "blob#spill-draft".into(), kind: "spillover-draft".into(), filename: "spillover-draft.txt".into(), bytes: spill_draft },
            ExtraBlob { blob_id: "blob#gate-draft".into(), kind: "gate-draft".into(), filename: "gate-draft.json".into(), bytes: gate_draft },
        ],
        channel_units,
        spillover,
        matrix_stable_id: "matrix#panel".into(),
        transforms,
        gates,
    }
}

/// 近奇异 spillover（参数修订场景使用）：第 1、2 行近似共线。
pub fn near_singular_spillover() -> Vec<Vec<f64>> {
    vec![
        vec![1.0, 0.02, 0.01],
        vec![0.999999, 0.0200001, 0.0100001],
        vec![0.02, 0.04, 1.0],
    ]
}

/// 奇异 spillover（硬拒绝场景）：第 1、3 行完全相同。
pub fn singular_spillover() -> Vec<Vec<f64>> {
    vec![
        vec![1.0, 0.0, 0.0],
        vec![0.5, 0.5, 0.5],
        vec![1.0, 0.0, 0.0],
    ]
}
