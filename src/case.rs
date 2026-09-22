//! 内置离线案例包“流式边界室”。材料是领域文件（FCS + 单染/spillover/门/谱系），
//! 不是通用表格。原始字节通过 include_bytes! 固定，sha256 可复核。

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::fcs::{self, FcsChannelSpec};
use crate::types::{
    Channel, CompMatrix, Dataset, EventData, Gate, Lineage, RawValue, SpillMeta,
    SingleStainControl, TransformParams,
};
use crate::Result;

pub const CASE_ID: &str = "flow-boundary-chamber";

pub const FCS_BYTES: &[u8] = include_bytes!("../cases/flow-boundary-chamber/v1/events.fcs");
pub const SPILL_META_V1: &str = include_str!("../cases/flow-boundary-chamber/v1/spill-meta.json");
pub const SPILL_V1: &str = include_str!("../cases/flow-boundary-chamber/v1/spillover.json");
pub const SPILL_META_V2: &str = include_str!("../cases/flow-boundary-chamber/v2/spill-meta.json");
pub const SPILL_V2: &str = include_str!("../cases/flow-boundary-chamber/v2/spillover.json");
pub const TRANSFORM_V1: &str = include_str!("../cases/flow-boundary-chamber/v1/transform.json");
pub const TRANSFORM_V2: &str = include_str!("../cases/flow-boundary-chamber/v2/transform.json");
pub const GATES_V1: &str = include_str!("../cases/flow-boundary-chamber/v1/gates.json");
pub const GATES_V2: &str = include_str!("../cases/flow-boundary-chamber/v2/gates.json");
pub const LINEAGE_V1: &str = include_str!("../cases/flow-boundary-chamber/v1/lineage.json");
pub const SCENARIO: &str = include_str!("../cases/flow-boundary-chamber/v2/scenario.json");

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let d = h.finalize();
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 解析案例 FCS 为不可变 Dataset。无法转换的 token 保留原文并附诊断。
pub fn dataset_v1() -> Result<Dataset> {
    let f = fcs::parse_fcs(FCS_BYTES)?;
    let mut channels = Vec::new();
    let n_par: usize = f
        .keywords
        .get("$PAR")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    for i in 1..=n_par {
        let name = f
            .keywords
            .get(&format!("$P{i}N"))
            .cloned()
            .unwrap_or_else(|| format!("CH{i}"));
        let long = f.keywords.get(&format!("$P{i}S")).cloned().unwrap_or_else(|| name.clone());
        let range = f
            .keywords
            .get(&format!("$P{i}R"))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(262144.0);
        channels.push(Channel {
            id: name,
            label: long,
            unit: "ADC count".into(),
            range,
            gain_source: format!("$P{i}G={}", f.keywords.get(&format!("$P{i}G")).cloned().unwrap_or_default()),
        });
    }
    let mut events = Vec::new();
    for (idx, (toks, nums)) in f.raw_tokens.iter().zip(f.numerics.iter()).enumerate() {
        let mut values = Vec::new();
        for (tok, num) in toks.iter().zip(nums.iter()) {
            let range = channels.get(values.len()).map(|c| c.range).unwrap_or(262144.0);
            let diagnostic = classify_token(tok, *num, range);
            values.push(RawValue {
                token: tok.clone(),
                numeric: *num,
                diagnostic,
            });
        }
        events.push(EventData {
            event_id: format!("E{:03}", idx + 1),
            ordinal: idx,
            values,
        });
    }
    Ok(Dataset {
        dataset_id: "DS.fbc-events.v1".into(),
        basis: "ADC count (FCS3.1 LIST MODE, per-channel $PnR)".into(),
        channels,
        events,
        source_sha256: sha256_hex(FCS_BYTES),
        source_name: "v1/events.fcs".into(),
    })
}

fn classify_token(token: &str, num: Option<f64>, range: f64) -> String {
    match token {
        "SAT" => "saturation".into(),
        "OVF" => "overflow".into(),
        "BAD" => "bad_token".into(),
        _ => match num {
            Some(n) if n >= range => "at_range".into(),
            Some(n) if n < 0.0 => "negative".into(),
            Some(_) => "none".into(),
            None => "bad_token".into(),
        },
    }
}

#[derive(serde::Deserialize)]
struct GateFile {
    gates: Vec<Gate>,
}

#[derive(serde::Deserialize)]
struct ControlFile {
    #[allow(dead_code)]
    spill_meta_id: String,
    #[allow(dead_code)]
    channels: Vec<String>,
    #[allow(dead_code)]
    controls: Vec<SingleStainControl>,
    #[serde(default)]
    #[allow(dead_code)]
    source_keyword: String,
    #[serde(default)]
    #[allow(dead_code)]
    source_sha256: String,
}

pub fn gates_v1() -> Result<Vec<Gate>> {
    let gf: GateFile = serde_json::from_str(GATES_V1)?;
    Ok(gf.gates)
}
pub fn gates_v2() -> Result<Vec<Gate>> {
    let gf: GateFile = serde_json::from_str(GATES_V2)?;
    Ok(gf.gates)
}
pub fn gates_map(version: u8) -> Result<BTreeMap<String, Gate>> {
    let list = if version == 1 { gates_v1()? } else { gates_v2()? };
    Ok(list.into_iter().map(|g| (g.gate_id.clone(), g)).collect())
}

pub fn lineage_v1() -> Result<Lineage> {
    Ok(serde_json::from_str(LINEAGE_V1)?)
}

pub fn transform_v1() -> Result<TransformParams> {
    Ok(serde_json::from_str(TRANSFORM_V1)?)
}
pub fn transform_v2() -> Result<TransformParams> {
    Ok(serde_json::from_str(TRANSFORM_V2)?)
}

pub fn spill_matrix(v: u8) -> Result<CompMatrix> {
    let s = if v == 1 { SPILL_V1 } else { SPILL_V2 };
    Ok(serde_json::from_str(s)?)
}

pub fn spill_meta(v: u8) -> Result<SpillMeta> {
    let raw = if v == 1 { SPILL_META_V1 } else { SPILL_META_V2 };
    let cf: ControlFile = serde_json::from_str(raw)?;
    Ok(SpillMeta {
        spill_meta_id: if v == 1 { "SM.fbc.v1".into() } else { "SM.fbc.v2".into() },
        channels: cf.channels,
        controls: cf.controls,
        source_keyword: cf.source_keyword,
        source_sha256: sha256_hex(raw.as_bytes()),
    })
}

pub fn scenario() -> serde_json::Value {
    serde_json::from_str(SCENARIO).expect("bundled scenario parses")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_has_24_events_and_six_channels() {
        let ds = dataset_v1().unwrap();
        assert_eq!(ds.events.len(), 24);
        assert_eq!(ds.channels.len(), 6);
        let by_id = |id: &str| ds.events.iter().find(|e| e.event_id == id).unwrap();
        assert_eq!(by_id("E018").values[5].diagnostic, "saturation");
        assert_eq!(by_id("E019").values[3].diagnostic, "overflow");
        assert_eq!(by_id("E020").values[4].diagnostic, "bad_token");
        assert_eq!(by_id("E017").values[2].diagnostic, "at_range");
        // 无法转换的值留在原位置：numeric=None 但事件和 token 都在。
        assert!(by_id("E018").values[5].numeric.is_none());
        assert_eq!(by_id("E018").values[5].token, "SAT");
        assert_eq!(ds.channels[0].unit, "ADC count");
    }

    #[test]
    fn sp_v2_is_near_singular_and_v1_is_fine() {
        let s1 = spill_matrix(1).unwrap();
        let s2 = spill_matrix(2).unwrap();
        let d1 = crate::comp::diagnose(&s1.s);
        let d2 = crate::comp::diagnose(&s2.s);
        assert!(!d1.near_singular);
        assert!(d2.near_singular, "rcond={}", d2.rcond);
    }

    #[test]
    fn gates_and_lineage_load() {
        let gs = gates_v1().unwrap();
        assert_eq!(gs.len(), 5);
        let ln = lineage_v1().unwrap();
        assert_eq!(ln.edges.len(), 5);
        assert_eq!(ln.edges[0].parent, "ROOT");
    }
}
