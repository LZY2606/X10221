//! 派生层：从材料字节与已版本化解释构造派生数据。
//! 来源材料永不修改；这里只产出可重建的派生结果。

use crate::compute::{
    compensate_events, evaluate_compensation, transform_events, CompensationOutcome, DerivedEvent,
    EventFlag,
};
use crate::domain::{EntityVersion, TransformSet};
use crate::fcs::spill::SpilloverMatrix;
use crate::fcs::FcsFile;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedTube {
    pub channel_names: Vec<String>,
    pub channel_units: Vec<String>,
    pub channel_ranges: Vec<Option<f64>>,
    pub events: Vec<Vec<f64>>,
    pub diagnostics: Vec<Diagnostic>,
    pub keywords: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub detail: String,
}

pub fn parse_tube(bytes: &[u8]) -> ParsedTube {
    let fcs = FcsFile::parse(bytes.to_vec());
    ParsedTube {
        channel_names: fcs.channels.iter().map(|c| c.name.clone()).collect(),
        channel_units: fcs.channels.iter().map(|c| c.unit.clone()).collect(),
        channel_ranges: fcs.channels.iter().map(|c| c.range).collect(),
        events: fcs.events.clone(),
        diagnostics: fcs
            .diagnostics
            .iter()
            .map(|d| Diagnostic {
                code: d.code.clone(),
                detail: d.detail.clone(),
            })
            .collect(),
        keywords: fcs.keywords.clone(),
    }
}

pub fn spill_from_version(v: &EntityVersion) -> Result<SpilloverMatrix, String> {
    serde_json::from_value::<SpilloverMatrix>(v.data.clone())
        .map_err(|e| format!("spillover 版本 {} 数据无效: {e}", v.id))
}

pub fn transform_from_version(v: &EntityVersion) -> Result<TransformSet, String> {
    serde_json::from_value::<TransformSet>(v.data.clone())
        .map_err(|e| format!("transform 版本 {} 数据无效: {e}", v.id))
}

pub struct DerivedInputs<'a> {
    pub tube: &'a ParsedTube,
    pub spill: &'a SpilloverMatrix,
    pub spill_version_id: &'a str,
    pub chosen_index: Option<usize>,
    pub transform: &'a TransformSet,
    pub fluor_channels: &'a [String],
}

pub struct DerivedTube {
    pub outcome: CompensationOutcome,
    pub derived: Vec<DerivedEvent>,
}

/// 运行完整派生管线；同时对极端/饱和/负值给出诊断（保留原始值）。
pub fn derive(input: DerivedInputs) -> DerivedTube {
    let DerivedInputs {
        tube,
        spill,
        spill_version_id,
        chosen_index,
        transform,
        fluor_channels,
    } = input;
    let outcome = evaluate_compensation(spill, spill_version_id, chosen_index);
    let fluor_cols: Vec<usize> = fluor_channels
        .iter()
        .map(|name| tube.channel_names.iter().position(|c| c == name).unwrap_or(0))
        .collect();
    let (comp, mut flags) = compensate_events(
        &tube.events,
        &fluor_cols,
        &outcome.chosen_inverse,
        &outcome.verdict.status,
    );
    // 极端信号诊断（按通道范围/符号），不删除任何事件。
    for (i, ev) in tube.events.iter().enumerate() {
        for (c, val) in ev.iter().enumerate() {
            let name = tube.channel_names.get(c).cloned().unwrap_or_default();
            let range = tube.channel_ranges.get(c).copied().flatten();
            if !val.is_finite() {
                flags[i].push(EventFlag {
                    event_index: i as i64,
                    code: "NONFINITE_SIGNAL".into(),
                    detail: format!("通道 {name} 非有限值，原始值保留不绘图"),
                });
            } else if let Some(max) = range {
                if *val >= max {
                    flags[i].push(EventFlag {
                        event_index: i as i64,
                        code: "ADC_SATURATED".into(),
                        detail: format!("通道 {name} 值 {val} 达到 ADC 上限 {max}（{}/），保留并标记饱和", tube.channel_units.get(c).map(|s|s.as_str()).unwrap_or("")),
                    });
                }
            }
            if *val < 0.0 {
                flags[i].push(EventFlag {
                    event_index: i as i64,
                    code: "NEGATIVE_SIGNAL".into(),
                    detail: format!("通道 {name} 原始值 {val} 为负（基线漂移/电子补偿），保留原位置"),
                });
            }
        }
    }
    let transformed = transform_events(
        &tube.events,
        &comp,
        &mut flags,
        transform,
        &tube.channel_names,
    );
    let derived = tube
        .events
        .iter()
        .enumerate()
        .map(|(i, raw)| DerivedEvent {
            index: i as i64,
            raw: raw.clone(),
            compensated: comp[i].clone(),
            transformed: transformed[i].clone(),
            flags: flags[i].clone(),
        })
        .collect();
    DerivedTube { outcome, derived }
}
