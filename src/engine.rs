//! 复核引擎：把事件账本重放为“钉住版本”的可见状态。
//! - 不修改历史：任何重评都产生新事件/新候选，旧结论仍可在旧 seq 读回。
//! - 失效范围按依赖图计算，只影响直接/间接下游节点。
use crate::model::*;
use crate::store::RowEvent;
use rusqlite::Transaction;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// 重放后的分支状态（截止某一 seq）。
#[derive(Clone, Debug, Default)]
pub struct State {
    pub seq: i64,
    /// (kind,id) -> 生效版本
    pub materials: BTreeMap<(String, String), u32>,
    pub names: BTreeMap<String, String>,
    /// subject(如 compensation:comp-v3) -> 生效裁定
    pub decisions: BTreeMap<String, Decision>,
    /// node_id -> 最近一次接受的 cfg_hash
    pub refreshed: BTreeMap<String, RefreshRec>,
    /// 已被撤销的裁定 subject（同一 subject 撤销后可重新裁定）
    pub undone: BTreeSet<String>,
    /// subject -> seq，变更/失效事件的位置
    pub change_seq: BTreeMap<String, i64>,
}

#[derive(Clone, Debug)]
pub struct Decision {
    pub candidate: String,
    pub event_seq: i64,
}

#[derive(Clone, Debug)]
pub struct RefreshRec {
    pub cfg_hash: String,
    pub seq: i64,
}

pub fn apply_event(st: &mut State, ev: &RowEvent) {
    st.seq = ev.seq;
    match ev.kind.as_str() {
        evkind::IMPORT => {
            if let (Some(k), Some(id), Some(v)) = (&ev.mat_kind, &ev.mat_id, ev.mat_version) {
                st.materials.insert((k.clone(), id.clone()), v as u32);
                st.change_seq.insert(format!("{}:{}", k, id), ev.seq);
                if k == "gate" {
                    if let Some(name) = ev.payload.get("name").and_then(|x| x.as_str()) {
                        st.names.insert(id.clone(), name.to_string());
                    }
                }
            }
        }
        evkind::COMP_DECISION => {
            let candidate = ev
                .payload
                .get("candidate")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            st.decisions.insert(ev.subject.clone(), Decision { candidate, event_seq: ev.seq });
            st.undone.remove(&ev.subject);
        }
        evkind::UNDO => {
            if let Some(sub) = ev.payload.get("undo_subject").and_then(|x| x.as_str()) {
                st.decisions.remove(sub);
                st.undone.insert(sub.to_string());
            }
        }
        evkind::RENAME => {
            if let (Some(id), Some(name)) = (
                ev.payload.get("gate_id").and_then(|x| x.as_str()),
                ev.payload.get("new_name").and_then(|x| x.as_str()),
            ) {
                st.names.insert(id.to_string(), name.to_string());
            }
        }
        evkind::REFRESH => {
            if let (Some(node), Some(hash)) = (
                ev.payload.get("node_id").and_then(|x| x.as_str()),
                ev.payload.get("cfg_hash").and_then(|x| x.as_str()),
            ) {
                st.refreshed
                    .insert(node.to_string(), RefreshRec { cfg_hash: hash.to_string(), seq: ev.seq });
            }
        }
        _ => {}
    }
}

impl State {
    pub fn replay(t: &Transaction, branch: &str, until: Option<i64>) -> rusqlite::Result<State> {
        let rows = crate::store_ops::list_events_until(t, branch, until)?;
        let mut st = State::default();
        for ev in rows {
            apply_event(&mut st, &ev);
        }
        Ok(st)
    }

    pub fn mat_version(&self, kind: &str, id: &str) -> Option<u32> {
        self.materials.get(&(kind.to_string(), id.to_string())).copied()
    }

    pub fn mat_ref(&self, kind: &str, id: &str) -> Option<MatRef> {
        self.mat_version(kind, id).map(|version| MatRef {
            kind: kind.to_string(),
            id: id.to_string(),
            version,
        })
    }
}

/// 规范化数值：无法转换返回 None（调用方保留原值并附诊断，绝不丢弃事件）。
pub fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|x| x.is_finite()),
        _ => None,
    }
}

/// 稳定身份：FNV-1a 64bit，随导出/导入保持不变（只依赖字符串内容）。
pub fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for b in p.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{}:{:016x}", prefix, h)
}

/// 点是否在多边形内（含边界；边界规则版本见 model::RULES_VERSION）。
pub fn point_in_polygon(pt: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let (x, y) = (pt[0], pt[1]);
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let scale = 1e-9 * (1.0 + a[0].abs() + a[1].abs() + b[0].abs() + b[1].abs());
        let cross = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
        let within = x <= a[0].max(b[0]) + 1e-12
            && x + 1e-12 >= a[0].min(b[0])
            && y <= a[1].max(b[1]) + 1e-12
            && y + 1e-12 >= a[1].min(b[1]);
        if cross.abs() <= scale && within {
            return true;
        }
        let intersects = ((a[1] > y) != (b[1] > y))
            && (x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0]);
        if intersects {
            inside = !inside;
        }
    }
    inside
}

#[allow(dead_code)]
fn _silence() {
    let _ = json!(null);
}
