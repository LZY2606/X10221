//! Generate the offline domain case pack:
//!   cases/flow-boundary-chamber/sample.fcs + three single-stain controls,
//! plus scenario.md. Run once: `cargo run --example gen_case` (not needed to
//! build/test; generated files are committed).

use flowline::engine::fcs::{ChannelSpec, FcsFile, FcsValue};
use std::fs;
use std::path::Path;

fn ch(name: &str) -> ChannelSpec {
    ChannelSpec {
        n: name.to_string(),
        detector: Some(name.trim_end_matches("-A").to_string()),
        range: 262_144.0,
        unit: Some("ADC counts".to_string()),
    }
}

fn row(tokens: &[&str]) -> Vec<FcsValue> {
    tokens.iter().map(|t| FcsValue::from_token(t)).collect()
}

fn main() {
    let dir = Path::new("cases/flow-boundary-chamber");
    fs::create_dir_all(dir).unwrap();
    let channels = vec![
        ch("FSC-A"),
        ch("SSC-A"),
        ch("FITC-A"),
        ch("PE-A"),
    ];

    // 12 events. FSC/SSC in raw ADC counts:
    //  debris (1) | myeloid (2,3) | lymphocyte population (4..12).
    // Event 12 carries the unconvertible sentinel "OVER" on FITC-A; it is kept
    // verbatim and diagnosed, never coerced. E11 is an extreme-but-finite event.
    // Fluorescence tuples below are MEASURED (pre-compensation).
    let events = vec![
        // FSC   SSC   FITC   PE
        row(&["2000", "1000", "80", "60"]),              // 1 debris
        row(&["60000", "55000", "90", "120"]),           // 2 myeloid
        row(&["55000", "62000", "70", "140"]),           // 3 myeloid
        row(&["90000", "18000", "950", "80"]),           // 4 T CD4
        row(&["93000", "17500", "980", "85"]),           // 5 T CD4
        row(&["88000", "19000", "1010", "75"]),          // 6 T CD4
        row(&["96000", "16000", "970", "90"]),           // 7 T CD8
        row(&["91000", "20000", "940", "95"]),           // 8 T CD8
        row(&["100000", "15000", "100", "130"]),         // 9 B
        row(&["92000", "18500", "110", "125"]),          // 10 B
        row(&["95000", "17000", "260000", "100"]),       // 11 extreme
        row(&["97000", "16500", "OVER", "90"]),          // 12 sentinel
    ];
    let sample = FcsFile {
        version: "FCS3.1".to_string(),
        keywords: vec![
            ("$FIL".to_string(), "sample.fcs".to_string()),
            ("$SRC".to_string(), "synthetic flow boundary chamber".to_string()),
            ("$CYT".to_string(), "flowline-case".to_string()),
            ("$SMNO".to_string(), "FBX-BASELINE".to_string()),
            ("$DATE".to_string(), "2026-09-23".to_string()),
        ],
        channels: channels.clone(),
        events,
    };
    fs::write(dir.join("sample.fcs"), sample.to_bytes()).unwrap();

    // Single-stain controls. Spillover percentages come from these metadata;
    // only a few anchor events are needed offline.
    let controls = [
        (
            "sst-fitc.fcs",
            "FITC-SINGLE-STAIN",
            vec![row(&["90000", "1000", "50000", "5000"]), row(&["91000", "1100", "60000", "6000"])],
        ),
        (
            "sst-pe.fcs",
            "PE-SINGLE-STAIN",
            vec![row(&["90000", "1000", "1500", "50000"]), row(&["90500", "1050", "1800", "60000"])],
        ),
        (
            "sst-unstained.fcs",
            "UNSTAINED",
            vec![row(&["90000", "1000", "80", "60"])],
        ),
    ];
    for (name, smno, ev) in controls {
        let f = FcsFile {
            version: "FCS3.1".to_string(),
            keywords: vec![
                ("$FIL".to_string(), name.to_string()),
                ("$SMNO".to_string(), smno.to_string()),
            ],
            channels: channels.clone(),
            events: ev,
        };
        fs::write(dir.join(name), f.to_bytes()).unwrap();
    }

    let md = r#"流式边界室：补偿矩阵、门控系谱与细胞群不确定性
==================================================

材料（来源层，原始表示保留；派生层才规范化）
- sample.fcs         12 个事件，4 通道（FSC-A, SSC-A, FITC-A, PE-A），单位 ADC counts
- sst-fitc.fcs       FITC 单染对照
- sst-pe.fcs         PE 单染对照
- sst-unstained.fcs  未染色对照

固定边界情形
- 极端信号：事件 11 FITC-A=260000（满量程有限值）；事件 12 FITC-A="OVER"
  （不可转换 token，留在原位置并附诊断，不为画图丢弃）。
- 边界点：事件 8 落在淋巴细胞门边界；事件 6 落在 CD3 门边界；
  事件 5 落在 CD4 门边界（closed gate，boundary 单独计数且纳入分子）。
- 近奇异补偿：分支 branch-singular 仅改动 spillover 这一直接来源
  （S 的两列几乎相同，rcond < 1e-3），系统不得默默求逆，必须保留全部
  ridge λ 候选并等待人工裁定。
- 门嵌套改名：gate_renamed 只改 label，稳定身份 gate#cd3 与下游系谱不变，
  下游结果不得失效。
- 上游过期传播：revision-transform 改动 transform 参数后，荧光门下游
  population 标记 stale 但旧版本保留；FSC/SSC 门不受影响（局部失效）。
- 同版本并发修改：两个窗口基于同一 head_hash 保存，后提交者收到 409 与差异。

关键规则
1. 近奇异补偿矩阵不能被默默求逆。
2. 门的百分比分母必须钉住父群版本（denominator_version），不跟随“最新”。
3. 修订上游门后下游结果标记过期而不覆盖。
"#;
    fs::write(dir.join("scenario.md"), md).unwrap();
    println!("case pack written to {}", dir.display());
}
