//! 离线案例包生成器：生成真实 FCS3.1 二进制样本 + 案例 JSON（制品版本与反例场景脚本）。
//! 运行：cargo run --bin gen-case
//! 输出：cases/flow-boundary-lab/sample.fcs 与 case.json
//!
//! 案例包是领域材料，不是通用表格：通道、spillover、补偿矩阵、变换、门多边形、
//! 父子关系分开版本化；场景脚本实际经过矩阵奇异、极端信号、边界点、门嵌套改名、
//! 上游过期传播与同版本并发修改。

use serde_json::json;
use std::fs;
use std::path::PathBuf;

fn main() {
    let dir = PathBuf::from("cases/flow-boundary-lab");
    fs::create_dir_all(&dir).unwrap();

    // 4 通道：FSC-A、SSC-A、FITC-A、PE-A。线性整数、$PnR=1024。
    let channels = ["FSC-A", "SSC-A", "FITC-A", "PE-A"];
    // (FSC,SSC,FITC,PE) 原始通道值；含极端信号与饱和哨兵（1023 即 >=1024 范围的饱和点）。
    let events: Vec<[i64; 4]> = vec![
        [120, 100, 60, 40],
        [130, 110, 620, 90],    // FITC 高
        [125, 105, 80, 580],    // PE 高
        [400, 300, 300, 260],   // 双阳（待补偿）
        [410, 305, 320, 200],
        [90, 95, 700, 720],     // 极端双高信号
        [1023, 80, 30, 20],     // FSC 饱和哨兵（$PnR=1024）
        [128, 118, 100, 100],
        [132, 112, 610, 95],
        [126, 108, 82, 560],
        [200, 180, 0, 0],
        [205, 185, 0, 0],
    ];

    let fcs = encode_fcs(&channels, &events);
    fs::write(dir.join("sample.fcs"), &fcs).unwrap();

    let case = build_case_json(&events);
    let pretty = serde_json::to_string_pretty(&case).unwrap();
    fs::write(dir.join("case.json"), pretty).unwrap();
    println!("案例包已生成于 {}", dir.display());
}

fn encode_fcs(channels: &[&str], events: &[[i64; 4]]) -> Vec<u8> {
    // 一个良性 spillover（与后续 case.json 的 comp v1 匹配）：
    // S = [[1,0.05],[0.12,1]]（列=FITC/PE 源，行=FITC/PE 检测器）。
    let spill = "2,FITC-A,PE-A,1.0,0.05,0.12,1.0";
    let n = events.len();
    let par = channels.len();

    let mut kws: Vec<(String, String)> = vec![
        ("$BEGINANALYSIS".into(), "0".into()),
        ("$ENDANALYSIS".into(), "0".into()),
        ("$BEGINSTEXT".into(), "0".into()),
        ("$ENDSTEXT".into(), "0".into()),
        ("$BYTEORD".into(), "1,2,3,4".into()),
        ("$DATATYPE".into(), "I".into()),
        ("$MODE".into(), "L".into()),
        ("$NEXTDATA".into(), "0".into()),
        ("$TOT".into(), n.to_string()),
        ("$PAR".into(), par.to_string()),
        ("$OP".into(), "flowline".into()),
        ("$CYT".into(), "FlowBoundaryLab-Synthetic".into()),
        ("$SPILLOVER".into(), spill.into()),
    ];
    for (i, c) in channels.iter().enumerate() {
        let p = i + 1;
        kws.push((format!("$P{p}N"), c.to_string()));
        kws.push((format!("$P{p}B"), "16".into()));
        kws.push((format!("$P{p}R"), "1024".into()));
        kws.push((format!("$P{p}E"), "0,0".into()));
        kws.push((format!("$P{p}U"), "channel".into()));
    }
    // DATA: 每通道 uint16 little-endian。
    let mut data = Vec::with_capacity(n * par * 2);
    for ev in events {
        for v in ev {
            data.extend_from_slice(&(*v as u16).to_le_bytes());
        }
    }

    // TEXT 段以 '/' 定界，键值对包在一对分隔符之间。
    let mut text = Vec::new();
    text.push(b'/');
    for (k, v) in &kws {
        text.extend_from_slice(k.as_bytes());
        text.push(b'/');
        // 值内若含 '/' 需双写，此处数据安全。
        text.extend_from_slice(v.as_bytes());
        text.push(b'/');
    }

    // 先占位头部，再用真实偏移重写。
    let header_len = 58; // 版本字段 10 + 6 个 8 字节偏移
    let begin_text = header_len;
    let end_text = begin_text + text.len() - 1;
    let begin_data = end_text + 1;
    let end_data = begin_data + data.len() - 1;

    let mut header = Vec::new();
    header.extend_from_slice(b"FCS3.1    ");
    let mut put = |buf: &mut Vec<u8>, s: String| {
        let b = s.as_bytes();
        buf.extend_from_slice(b);
        for _ in b.len()..8 {
            buf.push(b' ');
        }
    };
    put(&mut header, format!("{begin_text:>8}"));
    put(&mut header, format!("{end_text:>8}"));
    put(&mut header, format!("{begin_data:>8}"));
    put(&mut header, format!("{end_data:>8}"));
    put(&mut header, format!("{0:>8}", 0));
    put(&mut header, format!("{0:>8}", 0));
    assert_eq!(header.len(), 58);

    // BEGINDATA/ENDDATA 关键字值与头部一致（这里放 TEXT 内，解析器以头部优先）。
    let mut out = Vec::new();
    out.extend(header);
    out.extend(text);
    // 4 字节对齐不必要；直接接 DATA。
    out.extend(data);
    out
}

fn build_case_json(events: &[[i64; 4]]) -> serde_json::Value {
    let channels = ["FSC-A", "SSC-A", "FITC-A", "PE-A"];
    let _ = events;
    json!({
        "case_id": "flow-boundary-lab-v1",
        "title": "流式边界室：补偿矩阵、门控系谱与细胞群不确定性",
        "domain": "flow-cytometry",
        "coordinate_conventions": {
            "raw": "FCS 线性 channel 整数，$PnR=1024，饱和值=1023 哨兵，单位 channel",
            "transformed": "asinh(value/cofactor)，cofactor=150，坐标基标注在 transform 制品",
            "compensation": "线性空间左乘补偿矩阵 C，单位 fraction(linear)"
        },
        "units": {
            "FSC-A": "channel", "SSC-A": "channel",
            "FITC-A": "channel(linear)", "PE-A": "channel(linear)",
            "spillover": "fraction(linear)", "compensation": "fraction(linear)",
            "transformed": "asinh:cofactor=150", "percent": "% of pinned parent version"
        },
        "fcs_source": "sample.fcs",
        "artifacts": {
            "events": [{
                "id_hint": "events:panel:v1",
                "source": "sample.fcs",
                "note": "导入即原始表示；饱和事件保留哨兵，不做不可逆清洗"
            }],
            "spillover": [{
                "id_hint": "spillover:panel:v1",
                "detectors": ["FITC-A", "PE-A"],
                "matrix": [[1.0, 0.05], [0.12, 1.0]],
                "unit": "fraction(linear)",
                "source_keyword": "$SPILLOVER"
            }],
            "compensation": [
                {
                    "id_hint": "comp:panel:v1",
                    "detectors": ["FITC-A", "PE-A"],
                    "matrix": [[1.0, -0.05], [-0.12, 1.006]],
                    "basis": "spillover:panel:v1 的人工确认逆",
                    "unit": "fraction(linear)"
                },
                {
                    "id_hint": "comp:panel:v2",
                    "detectors": ["FITC-A", "PE-A"],
                    "matrix": [[1.0, 1.0], [1.0, 1.0000001]],
                    "basis": "故意近奇异：近重复行，cond2≈1e7",
                    "unit": "fraction(linear)",
                    "scenario": "near-singular"
                },
                {
                    "id_hint": "comp:panel:v3",
                    "detectors": ["FITC-A", "PE-A"],
                    "matrix": [[1.0, 1.0], [1.0, 1.0]],
                    "basis": "故意严格奇异：完全重复行",
                    "unit": "fraction(linear)",
                    "scenario": "strict-singular"
                }
            ],
            "transform": [
                {
                    "id_hint": "transform:panel:v1",
                    "params": [
                        {"detector": "FITC-A", "method": "asinh", "cofactor": 150.0,
                         "coordinate_basis": "asinh:cofactor=150"},
                        {"detector": "PE-A", "method": "asinh", "cofactor": 150.0,
                         "coordinate_basis": "asinh:cofactor=150"},
                        {"detector": "FSC-A", "method": "linear", "cofactor": 1.0,
                         "coordinate_basis": "linear"},
                        {"detector": "SSC-A", "method": "linear", "cofactor": 1.0,
                         "coordinate_basis": "linear"}
                    ]
                },
                {
                    "id_hint": "transform:panel:v2",
                    "params": [
                        {"detector": "FITC-A", "method": "asinh", "cofactor": 300.0,
                         "coordinate_basis": "asinh:cofactor=300"},
                        {"detector": "PE-A", "method": "asinh", "cofactor": 300.0,
                         "coordinate_basis": "asinh:cofactor=300"},
                        {"detector": "FSC-A", "method": "linear", "cofactor": 1.0,
                         "coordinate_basis": "linear"},
                        {"detector": "SSC-A", "method": "linear", "cofactor": 1.0,
                         "coordinate_basis": "linear"}
                    ],
                    "scenario": "param-revision"
                }
            ],
            "gate_polygon": [
                {
                    "id_hint": "gate-poly:lymph:v1",
                    "name": "lymph",
                    "x_detector": "FSC-A", "y_detector": "SSC-A",
                    "coordinate_basis": "linear",
                    "vertices": [[80.0,70.0],[160.0,70.0],[160.0,140.0],[80.0,140.0]],
                    "boundary_policy": "inside",
                    "note": "矩形淋巴样群；事件#7 FSC 饱和被排除但保留"
                },
                {
                    "id_hint": "gate-poly:lymph:v2",
                    "name": "lymph",
                    "x_detector": "FSC-A", "y_detector": "SSC-A",
                    "coordinate_basis": "linear",
                    "vertices": [[80.0,70.0],[220.0,70.0],[220.0,200.0],[80.0,200.0]],
                    "boundary_policy": "inside",
                    "scenario": "upstream-revision"
                },
                {
                    "id_hint": "gate-poly:cd3:v1",
                    "name": "cd3",
                    "x_detector": "FITC-A", "y_detector": "PE-A",
                    "coordinate_basis": "asinh:cofactor=150",
                    "vertices": [[0.8,0.0],[3.0,0.0],[3.0,3.0],[0.8,3.0]],
                    "boundary_policy": "candidate",
                    "note": "边界点策略 candidate：#2/#9 等落在 x=0.8 边附近，保留全部合法候选"
                }
            ],
            "gate_link": [
                {"id_hint": "gate-link:lymph:v1", "child": "lymph",
                 "parent": "root:all-events:v1", "display_name": "Lymphocytes"},
                {"id_hint": "gate-link:cd3:v1", "child": "cd3",
                 "parent": "gate-poly:lymph:v1", "display_name": "CD3+"},
                {"id_hint": "gate-link:cd3:v2", "child": "cd3",
                 "parent": "gate-poly:lymph:v1", "display_name": "T cells (renamed)",
                 "scenario": "nested-rename"}
            ]
        },
        "scenarios": [
            {"code": "S0-import", "title": "初始导入：六类来源分开版本化",
             "expect": "events/spillover/comp/transform/gate-poly/gate-link 各有独立 stable id"},
            {"code": "S1-singular", "title": "严格奇异补偿矩阵",
             "expect_status": "error",
             "expect": "拒绝输出补偿值，无候选，规则 NEAR-SINGULAR-NO-INV"},
            {"code": "S2-near-singular", "title": "近奇异补偿矩阵不能默默求逆",
             "expect_status": "blocked",
             "expect_candidates": ["reject", "pseudoinverse"],
             "expect": "cond2>1e6；保留 reject/pseudoinverse 全部合法候选等待裁定"},
            {"code": "S3-extreme", "title": "极端信号与饱和哨兵",
             "expect": "事件#7 FSC=1023 为哨兵保留原位置；事件#6 极端双高可补偿并诊断"},
            {"code": "S4-boundary", "title": "边界点与多候选",
             "expect_status": "blocked",
             "expect_candidates": ["boundary-inside", "boundary-outside"],
             "expect": "CD3 门 boundary 事件列出，两个合法候选都保留"},
            {"code": "S5-rename", "title": "门嵌套改名不改几何",
             "expect": "gate-link:cd3 v1->v2 仅显示名变化；gate-poly:cd3:v1 身份与结果不变"},
            {"code": "S6-stale", "title": "上游修订导致下游过期",
             "expect": "lymph v2 提交后旧 cd3 结果 status=stale 且 stale_reason 非空，旧内容不覆盖"},
            {"code": "S7-concurrent", "title": "同版本并发修改",
             "expect": "第二次以 base_version 旧值提交 comp 返回 VERSION_CONFLICT 并给双方差异"},
            {"code": "S8-conflict-decision", "title": "互相冲突的人工决定",
             "expect": "draft-a 采纳 pseudoinverse 与 draft-b 采纳 reject 各自保留为不同分支"},
            {"code": "S9-revision", "title": "参数修订只影响局部依赖",
             "expect": "transform v2(cofactor=300) 仅使依赖变换坐标的 cd3 失效；FSC/SSC 线性门不受影响"}
        ],
        "replay_order": [
            "S0-import", "S9-revision", "S3-extreme", "S1-singular", "S2-near-singular",
            "S4-boundary", "S5-rename", "S6-stale", "S7-concurrent", "S8-conflict-decision"
        ]
    })
}
