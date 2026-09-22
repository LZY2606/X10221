//! 离线案例包：“流式边界室：补偿矩阵、门控系谱与细胞群不确定性”。
//!
//! 这不是通用表格，而是按 FCS 叙事组织的领域材料：
//! 原始通道值（保留原始字符串）、spillover 元数据、补偿矩阵、
//! 变换参数、门多边形、父子关系分别版本化。
//!
//! 固定事件覆盖：矩阵奇异、极端信号、边界点、门嵌套改名、
//! 上游过期传播、同版本并发修改。

use crate::domain::*;

pub const CASE_ID: &str = "flow-boundary-chamber-v1";

pub fn channels() -> Vec<ChannelSpec> {
    vec![
        ChannelSpec {
            code: "FSC-A".into(),
            label: "Forward Scatter Area".into(),
            unit: "ADC-count".into(),
            basis: "raw-linear-ADC".into(),
            linear_max: 1_000_000.0,
        },
        ChannelSpec {
            code: "SSC-A".into(),
            label: "Side Scatter Area".into(),
            unit: "ADC-count".into(),
            basis: "raw-linear-ADC".into(),
            linear_max: 1_000_000.0,
        },
        ChannelSpec {
            code: "FITC".into(),
            label: "FITC (530/30)".into(),
            unit: "ADC-count".into(),
            basis: "raw-linear-ADC".into(),
            linear_max: 262_144.0,
        },
        ChannelSpec {
            code: "PE".into(),
            label: "PE (585/40)".into(),
            unit: "ADC-count".into(),
            basis: "raw-linear-ADC".into(),
            linear_max: 262_144.0,
        },
    ]
}

fn rv(channel: &str, raw: &str, max: f64) -> RawValue {
    let mut v = RawValue::new(raw.to_string(), max);
    v.channel = channel.to_string();
    v
}

fn event(id: &str, fsc: &str, ssc: &str, fitc: &str, pe: &str) -> FcsEvent {
    let ch = channels();
    let m = |code: &str| ch.iter().find(|c| c.code == code).unwrap().linear_max;
    FcsEvent {
        event_id: id.to_string(),
        values: vec![
            rv("FSC-A", fsc, m("FSC-A")),
            rv("SSC-A", ssc, m("SSC-A")),
            rv("FITC", fitc, m("FITC")),
            rv("PE", pe, m("PE")),
        ],
    }
}

/// v1 基线：12 个事件，内含极端/越界/不可解析/边界点。
pub fn dataset_v1() -> DatasetVersion {
    DatasetVersion {
        version_id: "DS-v1".into(),
        supersedes: None,
        channels: channels(),
        events: vec![
            event("E01", "52000", "48000", "1020", "980"),
            event("E02", "51000", "47000", "860", "1200"),
            event("E03", "48000", "50000", "9500", "3050"),
            event("E04", "55000", "52000", "2900", "9200"),
            event("E05", "47000", "46000", "9050", "3200"),
            event("E06", "50000", "50000", "1100", "900"),
            event("E07", "8000", "7000", "300", "200"),
            event("E08", "46000", "49000", "1200", "1300"),
            event("E09", "43000", "58000", "2000", "5000"),
            // 极端信号：超过 FITC ADC 满量程，保留并附 overflow 诊断（绝不裁剪丢弃）。
            event("E10", "53000", "45000", "300000", "1500"),
            // 不可解析值：留在原位置并附诊断，散点不绘制该轴但记录保留。
            event("E11", "51500", "50500", "202.7", "NA"),
            // 边界点：补偿后恰好落在 FITC+ 门边界（v1 矩阵下 FITC_comp=200）。
            event("E12", "50500", "49500", "248", "200"),
        ],
        note: "FCS 初始导入：原始 ADC 计数，原始字符串保留，无不可逆清洗".into(),
    }
}

pub fn spillover_v1() -> SpilloverMeta {
    SpilloverMeta {
        version_id: "SM-v1".into(),
        supersedes: None,
        panel_name: "2-color FITC/PE 教学面板".into(),
        fluorochromes: vec!["FITC".into(), "PE".into()],
        single_stain_controls: vec![
            SingleStainControl {
                fluorochrome: "FITC".into(),
                dataset_version: "DS-v1".into(),
                event_ids: vec!["E03".into(), "E05".into()],
                unit: "ADC-count".into(),
            },
            SingleStainControl {
                fluorochrome: "PE".into(),
                dataset_version: "DS-v1".into(),
                event_ids: vec!["E04".into()],
                unit: "ADC-count".into(),
            },
        ],
        acquired_on: "2026-09-19".into(),
        cytometer: "离线教学细胞仪 sim-4ch".into(),
        note: "spillover 元数据与数值补偿矩阵分开版本化".into(),
    }
}

/// 良态补偿矩阵 v1：S=[[1,0.30],[0.20,1]]，det=0.94。
pub fn comp_v1() -> CompensationMatrix {
    CompensationMatrix {
        version_id: "COMP-v1".into(),
        supersedes: None,
        channels: vec!["FITC".into(), "PE".into()],
        matrix: vec![vec![1.0, 0.30], vec![0.20, 1.0]],
        unit: "fraction".into(),
        basis: "spillover-fraction".into(),
        note: "单染估计的溢漏矩阵".into(),
    }
}

pub fn transform_v1() -> TransformParams {
    TransformParams {
        version_id: "TF-v1".into(),
        supersedes: None,
        kind: "asinh".into(),
        cofactor: 150.0,
        input_unit: "ADC-count(compensated-linear)".into(),
        output_unit: "asinh-scale".into(),
        note: "Bi-exponential 风格 asinh 压缩 v1（cofactor=150）".into(),
    }
}

/// 门 v1 基线：根门 → 细胞门（原始 ADC 空间）→ FITC+/PE+（补偿线性空间）。
pub fn gates_v1() -> Vec<GateVersion> {
    vec![
        GateVersion {
            gate_id: "G-ROOT".into(),
            version_id: "GV-ROOT-v1".into(),
            supersedes: None,
            name: "所有事件".into(),
            kind: GateKind::Root,
            space: GateSpace {
                x_channel: "FSC-A".into(),
                y_channel: "SSC-A".into(),
                basis: "raw-linear-ADC".into(),
                x_unit: "ADC-count".into(),
                y_unit: "ADC-count".into(),
                dataset_version: "DS-v1".into(),
                compensation_version: None,
                transform_version: None,
            },
            boundary: BoundaryPolicy::Inclusive,
            polygon: Vec::new(),
            note: "根门恒包含全部事件，分母=事件总数".into(),
        },
        GateVersion {
            gate_id: "G-CELLS".into(),
            version_id: "GV-CELLS-v1".into(),
            supersedes: None,
            name: "细胞门".into(),
            kind: GateKind::Polygon,
            space: GateSpace {
                x_channel: "FSC-A".into(),
                y_channel: "SSC-A".into(),
                basis: "raw-linear-ADC".into(),
                x_unit: "ADC-count".into(),
                y_unit: "ADC-count".into(),
                dataset_version: "DS-v1".into(),
                compensation_version: None,
                transform_version: None,
            },
            boundary: BoundaryPolicy::Inclusive,
            polygon: vec![
                Point { x: 40_000.0, y: 40_000.0 },
                Point { x: 60_000.0, y: 40_000.0 },
                Point { x: 60_000.0, y: 60_000.0 },
                Point { x: 40_000.0, y: 60_000.0 },
            ],
            note: "FSC/SSC 细胞门（E07 碎片在门外）".into(),
        },
        GateVersion {
            gate_id: "G-FITC".into(),
            version_id: "GV-FITC-v1".into(),
            supersedes: None,
            name: "FITC 阳性".into(),
            kind: GateKind::Polygon,
            space: GateSpace {
                x_channel: "FITC".into(),
                y_channel: "PE".into(),
                basis: "compensated-linear".into(),
                x_unit: "ADC-count(compensated-linear)".into(),
                y_unit: "ADC-count(compensated-linear)".into(),
                dataset_version: "DS-v1".into(),
                compensation_version: Some("COMP-v1".into()),
                transform_version: None,
            },
            // FITC_comp >= 200 的矩形半平面（x∈[200,300000]，y∈[-5000,300000]）。
            boundary: BoundaryPolicy::Ambiguous,
            polygon: vec![
                Point { x: 200.0, y: -5_000.0 },
                Point { x: 300_000.0, y: -5_000.0 },
                Point { x: 300_000.0, y: 300_000.0 },
                Point { x: 200.0, y: 300_000.0 },
            ],
            note: "FITC+ 门，左边界 x=200 设为 ambiguous：E12 恰好落在边界".into(),
        },
        GateVersion {
            gate_id: "G-PE".into(),
            version_id: "GV-PE-v1".into(),
            supersedes: None,
            name: "PE 阳性".into(),
            kind: GateKind::Polygon,
            space: GateSpace {
                x_channel: "PE".into(),
                y_channel: "FITC".into(),
                basis: "compensated-linear".into(),
                x_unit: "ADC-count(compensated-linear)".into(),
                y_unit: "ADC-count(compensated-linear)".into(),
                dataset_version: "DS-v1".into(),
                compensation_version: Some("COMP-v1".into()),
                transform_version: None,
            },
            boundary: BoundaryPolicy::Inclusive,
            polygon: vec![
                Point { x: 2_000.0, y: -5_000.0 },
                Point { x: 300_000.0, y: -5_000.0 },
                Point { x: 300_000.0, y: 300_000.0 },
                Point { x: 2_000.0, y: 300_000.0 },
            ],
            note: "PE+ 门，父门为 G-FITC（FITC+PE+ 双阳性谱系）".into(),
        },
    ]
}

pub fn relations_v1() -> GateRelationVersion {
    GateRelationVersion {
        version_id: "REL-v1".into(),
        supersedes: None,
        edges: vec![
            ("G-CELLS".into(), "G-ROOT".into()),
            ("G-FITC".into(), "G-CELLS".into()),
            ("G-PE".into(), "G-FITC".into()),
        ],
        note: "门控系谱：ROOT → CELLS → FITC+ → PE+".into(),
    }
}

/// v1 初始导入的完整修订包。
pub fn baseline_bundle() -> RevisionBundle {
    RevisionBundle {
        dataset: Some(dataset_v1()),
        spillover_meta: Some(spillover_v1()),
        compensation: Some(comp_v1()),
        transform: Some(transform_v1()),
        gates: gates_v1(),
        relations: Some(relations_v1()),
    }
}

/// 近奇异补偿矩阵 v2：两行几乎成比例，det = 1 - 0.9999 = 0.0001（< 1e-3）。
pub fn comp_v2_near_singular() -> CompensationMatrix {
    CompensationMatrix {
        version_id: "COMP-v2".into(),
        supersedes: Some("COMP-v1".into()),
        channels: vec!["FITC".into(), "PE".into()],
        matrix: vec![vec![1.0, 1.0], vec![0.9999, 1.0]],
        unit: "fraction".into(),
        basis: "spillover-fraction".into(),
        note: "重新拟合的补偿矩阵：条件数极差，det≈1e-4，禁止默默求逆".into(),
    }
}

/// 显式正则化后的补偿 v3（仅在审阅者接受 regularize 候选后由系统生成）。
pub fn comp_v3_regularized() -> CompensationMatrix {
    let mut c = comp_v2_near_singular();
    c.version_id = "COMP-v3".into();
    c.supersedes = Some("COMP-v2".into());
    c.note = "对 v2 显式加脊 0.01 后的人工修正矩阵（有争议）".into();
    c
}

/// 变换参数修订 v2：cofactor 150 → 100（只影响引用 asinh 空间的门）。
pub fn transform_v2() -> TransformParams {
    TransformParams {
        version_id: "TF-v2".into(),
        supersedes: Some("TF-v1".into()),
        kind: "asinh".into(),
        cofactor: 100.0,
        input_unit: "ADC-count(compensated-linear)".into(),
        output_unit: "asinh-scale".into(),
        note: "asinh cofactor 150 → 100，按依赖关系计算失效范围".into(),
    }
}

/// 把 FITC/PE 门重新钉到指定补偿版本（门形状不变，仅输入引用改变）。
pub fn gates_repinned(comp_v: &str, suffix: &str, note: &str) -> Vec<GateVersion> {
    gates_v1()
        .into_iter()
        .filter(|g| matches!(g.kind, GateKind::Polygon) && g.space.compensation_version.is_some())
        .map(|mut g| {
            g.version_id = format!("{}-{}", g.version_id.trim_end_matches("-v1"), suffix);
            g.supersedes = Some(
                gates_v1()
                    .iter()
                    .find(|x| x.gate_id == g.gate_id)
                    .unwrap()
                    .version_id
                    .clone(),
            );
            g.space.compensation_version = Some(comp_v.to_string());
            g.note = note.to_string();
            g
        })
        .collect()
}

/// 门嵌套改名：稳定 gate_id 不变，仅显示名变化。
pub fn gate_renamed_bundle() -> RevisionBundle {
    let mut g = gates_v1()
        .into_iter()
        .find(|g| g.gate_id == "G-FITC")
        .unwrap();
    g.version_id = "GV-FITC-v2-rename".into();
    g.supersedes = Some("GV-FITC-v1".into());
    g.name = "FITC 阳性（复核改名）".into();
    g.note = "仅改名：gate_id 保持 G-FITC，谱系与分母引用不被破坏".into();
    RevisionBundle { gates: vec![g], ..Default::default() }
}

/// 上游门修订：细胞门多边形扩大（触发下游过期，不覆盖旧结果）。
pub fn gate_revised_cells_bundle() -> RevisionBundle {
    let mut g = gates_v1()
        .into_iter()
        .find(|g| g.gate_id == "G-CELLS")
        .unwrap();
    g.version_id = "GV-CELLS-v2".into();
    g.supersedes = Some("GV-CELLS-v1".into());
    g.polygon = vec![
        Point { x: 30_000.0, y: 30_000.0 },
        Point { x: 70_000.0, y: 30_000.0 },
        Point { x: 70_000.0, y: 70_000.0 },
        Point { x: 30_000.0, y: 70_000.0 },
    ];
    g.note = "上游细胞门扩大；FITC/PE 下游结果按依赖标记过期，不覆盖".into();
    RevisionBundle { gates: vec![g], ..Default::default() }
}

/// 局部源数据改动：只替换数据集（DS-v2），门仍钉 DS-v1，
/// 由依赖关系决定失效范围——散射门与荧光门分别按钉版失效。
pub fn dataset_v2_patched() -> DatasetVersion {
    let mut d = dataset_v1();
    d.version_id = "DS-v2".into();
    d.supersedes = Some("DS-v1".into());
    d.note = "局部源数据改动：E05 FITC 9050→9200（仪器重对齐），其余字节不变".into();
    for e in d.events.iter_mut() {
        if e.event_id == "E05" {
            if let Some(v) = e.values.iter_mut().find(|v| v.channel == "FITC") {
                v.raw = "9200".into();
                v.numeric = Some(9200.0);
                v.diagnostics.clear();
            }
        }
    }
    d
}

pub fn source_patch_bundle() -> RevisionBundle {
    RevisionBundle { dataset: Some(dataset_v2_patched()), ..Default::default() }
}
