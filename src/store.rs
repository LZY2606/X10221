//! 谱系引擎：材料→补偿→变换→门→群体，append-only 事件日志 + 失效传播。
use crate::db::Db;
use crate::domain::{
    CompensationMatrix, FlowEvent, GatePolygon, GateTree, MaterialKind, TransformParams,
};
use crate::error::{AppError, AppResult};
use crate::gates;
use crate::hash::sha256_hex;
use rusqlite::{params, Connection};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub const BASELINE_BRANCH: &str = "br-baseline";
pub const REVIEW_BRANCH: &str = "br-review";
pub const ROOT_RESULT: &str = "root:universe:1";

#[derive(Clone)]
pub struct Store {
    pub db: std::sync::Arc<Db>,
}

impl Store {
    pub fn new(db: std::sync::Arc<Db>) -> Self {
        Store { db }
    }

    pub fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.conn.lock().unwrap()
    }

    pub fn meta(&self, key: &str) -> Option<String> {
        self.conn()
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get::<_, String>(0))
            .ok()
    }
    pub fn set_meta(&self, key: &str, value: &str) {
        self.conn()
            .execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .unwrap();
    }

    // ---------- 事件日志 ----------

    pub fn append_event(
        &self,
        c: &Connection,
        branch: &str,
        kind: &str,
        payload: &serde_json::Value,
        base_seq: Option<i64>,
    ) -> AppResult<i64> {
        let now = self.clock_ms(c);
        c.execute(
            "INSERT INTO events(branch_id,kind,payload_json,base_seq,created_at)
             VALUES(?1,?2,?3,?4,?5)",
            params![branch, kind, payload.to_string(), base_seq, now],
        )?;
        let seq = c.last_insert_rowid();
        c.execute("UPDATE branches SET head_event_seq=?1 WHERE id=?2", params![seq, branch])?;
        Ok(seq)
    }

    fn clock_ms(&self, c: &Connection) -> i64 {
        c.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM events", [], |r| r.get(0))
            .unwrap_or(1)
    }

    pub fn create_branch(&self, id: &str, label: &str, parent: Option<&str>) -> AppResult<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM branches WHERE id=?1",
            [id],
            |r| r.get(0),
        )?;
        if exists == 0 {
            tx.execute(
                "INSERT INTO branches(id,label,parent_branch,head_event_seq,created_at)
                 VALUES(?1,?2,?3,NULL,?4)",
                params![id, label, parent, 0],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn log_decision(
        &self,
        c: &Connection,
        seq: i64,
        branch: &str,
        verdict: &str,
        target: &str,
        candidate: Option<&str>,
        note: &str,
    ) -> AppResult<()> {
        c.execute(
            "INSERT INTO decisions(event_seq,branch_id,verdict,target_result_ref,chosen_candidate,note,reversed_at)
             VALUES(?1,?2,?3,?4,?5,?6,NULL)",
            params![seq, branch, verdict, target, candidate, note],
        )?;
        Ok(())
    }

    // ---------- 材料（原始字节）与制品（解释/派生） ----------

    /// 导入材料：事务内完成；任何失败整体回滚，不留半份记录。
    pub fn import_material_tx(
        &self,
        tx: &Connection,
        id: &str,
        kind: MaterialKind,
        source_uri: &str,
        raw: &[u8],
    ) -> AppResult<i64> {
        let version = self.next_material_version(tx, id)?;
        let sha = sha256_hex(raw);
        tx.execute(
            "INSERT INTO materials(id,version,kind,source_uri,source_sha256,raw_repr,created_at,superseded_by)
             VALUES(?1,?2,?3,?4,?5,?6,?7,NULL)",
            params![
                id,
                version,
                kind.to_string(),
                source_uri,
                sha,
                BASE64.encode(raw),
                version * 0 + version,
            ],
        )?;
        if version > 1 {
            tx.execute(
                "UPDATE materials SET superseded_by=?1 WHERE id=?2 AND version=?3",
                params![version, id, version - 1],
            )?;
        }
        Ok(version)
    }

    fn next_material_version(&self, c: &Connection, id: &str) -> AppResult<i64> {
        Ok(c.query_row(
            "SELECT COALESCE(MAX(version),0)+1 FROM materials WHERE id=?1",
            [id],
            |r| r.get(0),
        )?)
    }

    pub fn put_artifact(
        &self,
        tx: &Connection,
        id: &str,
        kind: &str,
        branch: &str,
        material_ref: &str,
        content: &serde_json::Value,
        source_event_seq: i64,
    ) -> AppResult<i64> {
        let version: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version),0)+1 FROM artifacts WHERE id=?1 AND branch_id=?2",
            params![id, branch],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO artifacts(id,version,kind,branch_id,material_ref,content_json,source_event_seq,superseded_by)
             VALUES(?1,?2,?3,?4,?5,?6,?7,NULL)",
            params![id, version, kind, branch, material_ref, content.to_string(), source_event_seq],
        )?;
        if version > 1 {
            tx.execute(
                "UPDATE artifacts SET superseded_by=?1 WHERE id=?2 AND branch_id=?3 AND version=?4",
                params![version, id, branch, version - 1],
            )?;
        }
        Ok(version)
    }

    pub fn latest_artifact(&self, branch: &str, id: &str) -> Option<(i64, serde_json::Value)> {
        self.conn()
            .query_row(
                "SELECT version,content_json FROM artifacts
                 WHERE branch_id=?1 AND id=?2 ORDER BY version DESC LIMIT 1",
                params![branch, id],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .ok()
            .map(|(v, s)| (v, serde_json::from_str(&s).unwrap_or(json!({}))))
    }

    pub fn material_raw(&self, id: &str, version: i64) -> AppResult<Vec<u8>> {
        let s: String = self.conn().query_row(
            "SELECT raw_repr FROM materials WHERE id=?1 AND version=?2",
            params![id, version],
            |r| r.get(0),
        )?;
        Ok(BASE64.decode(s.as_bytes()).map_err(|e| AppError::Case(e.to_string()))?)
    }
}

// ---------- base64（标准库手写，零依赖） ----------

struct B64;
impl B64 {
    const T: &'static [u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    fn encode(&self, data: &[u8]) -> String {
        let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
        for ch in data.chunks(3) {
            let b = [
                ch.first().copied().unwrap_or(0),
                ch.get(1).copied().unwrap_or(0),
                ch.get(2).copied().unwrap_or(0),
            ];
            let n = ch.len();
            let v = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(Self::T[((v >> 18) & 63) as usize] as char);
            out.push(Self::T[((v >> 12) & 63) as usize] as char);
            if n >= 2 {
                out.push(Self::T[((v >> 6) & 63) as usize] as char);
            } else {
                out.push('=');
            }
            if n >= 3 {
                out.push(Self::T[(v & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
        out
    }
    fn decode(&self, s: &[u8]) -> Result<Vec<u8>, String> {
        let val = |c: u8| -> Result<u32, String> {
            match c {
                b'A'..=b'Z' => Ok((c - b'A') as u32),
                b'a'..=b'z' => Ok((c - b'a' + 26) as u32),
                b'0'..=b'9' => Ok((c - b'0' + 52) as u32),
                b'+' => Ok(62),
                b'/' => Ok(63),
                _ => Err(format!("非法 base64 字符 {c}")),
            }
        };
        let mut out = Vec::new();
        let mut chunk: Vec<u32> = Vec::new();
        let mut pad = 0;
        for &c in s.iter().filter(|c| !c.is_ascii_whitespace()) {
            if c == b'=' {
                pad += 1;
                chunk.push(0);
            } else {
                chunk.push(val(c)?);
            }
            if chunk.len() == 4 {
                let v = (chunk[0] << 18) | (chunk[1] << 12) | (chunk[2] << 6) | chunk[3];
                out.push((v >> 16) as u8);
                if pad < 2 {
                    out.push((v >> 8) as u8);
                }
                if pad < 1 {
                    out.push(v as u8);
                }
                chunk.clear();
            }
        }
        Ok(out)
    }
}
const BASE64: B64 = B64;

// ---------- 谱系计算 ----------

#[derive(Debug, Clone)]
pub struct Pipeline {
    pub events: Vec<FlowEvent>,
    pub comp: Option<CompensationMatrix>,
    pub comp_version: i64,
    pub comp_inv: Option<Vec<Vec<f64>>>,
    pub comp_diagnostic: Option<String>,
    pub xform: TransformParams,
    pub xform_version: i64,
    pub tree: GateTree,
    pub tree_version: i64,
}

impl Store {
    /// 载入某分支最新解释链。近奇异补偿矩阵：保留诊断，绝不静默求逆。
    pub fn load_pipeline(&self, branch: &str) -> AppResult<Pipeline> {
        let events: Vec<FlowEvent> = match self.latest_artifact(branch, "events") {
            Some((_, content)) => serde_json::from_value(content["events"].clone())
                .map_err(|e| AppError::Case(format!("events artifact 解析失败: {e}")))?,
            None => Vec::new(),
        };
        let (comp, comp_version) = match self.latest_artifact(branch, "comp") {
            Some((v, c)) => (serde_json::from_value::<CompensationMatrix>(c["matrix"].clone()).ok(), v),
            None => (None, 0),
        };
        let (comp_inv, comp_diagnostic) = match &comp {
            Some(m) => match m.inverse() {
                Ok(inv) => (Some(inv), None),
                Err(reason) => (None, Some(reason)),
            },
            None => (None, Some("缺少补偿矩阵".into())),
        };
        let (xform, xform_version) = match self.latest_artifact(branch, "xform") {
            Some((v, c)) => (
                serde_json::from_value::<TransformParams>(c["params"].clone())
                    .map_err(|e| AppError::Case(format!("xform artifact 解析失败: {e}")))?,
                v,
            ),
            None => (
                TransformParams {
                    cofactors: BTreeMap::new(),
                    mode: BTreeMap::new(),
                    coordinate_basis: "none".into(),
                },
                0,
            ),
        };
        let (tree, tree_version) = match self.latest_artifact(branch, "gates") {
            Some((v, c)) => (
                serde_json::from_value::<GateTree>(c["tree"].clone())
                    .map_err(|e| AppError::Case(format!("gates artifact 解析失败: {e}")))?,
                v,
            ),
            None => (GateTree { gates: vec![] }, 0),
        };
        Ok(Pipeline {
            events,
            comp,
            comp_version,
            comp_inv,
            comp_diagnostic,
            xform,
            xform_version,
            tree,
            tree_version,
        })
    }

    /// 事件在指定 transform 下的坐标（荧光通道先补偿再变换）。
    pub fn event_coordinates(
        pipe: &Pipeline,
        ev: &FlowEvent,
    ) -> BTreeMap<String, (f64, crate::domain::ValueStatus)> {
        let mut out = BTreeMap::new();
        let comp_names: Vec<String> = pipe
            .comp
            .as_ref()
            .map(|c| c.channel_names.clone())
            .unwrap_or_default();
        let compensated: BTreeMap<String, f64> = match (&pipe.comp, &pipe.comp_inv) {
            (Some(m), Some(inv)) => {
                let measured: Vec<f64> = comp_names
                    .iter()
                    .map(|n| ev.channels.get(n).map(|v| v.raw).unwrap_or(f64::NAN))
                    .collect();
                let c = gates::apply_compensation(inv, &measured);
                m.channel_names
                    .iter()
                    .cloned()
                    .zip(c.into_iter())
                    .collect()
            }
            _ => comp_names.iter().map(|n| (n.clone(), f64::NAN)).collect(),
        };
        for (name, cv) in &ev.channels {
            let raw = if comp_names.contains(name) {
                compensated.get(name).copied().unwrap_or(f64::NAN)
            } else {
                cv.raw
            };
            let mode = pipe.xform.mode.get(name).map(String::as_str).unwrap_or("asinh");
            let val = if raw.is_nan() {
                f64::NAN
            } else if mode == "linear" {
                raw
            } else {
                let co = pipe.xform.cofactors.get(name).copied().unwrap_or(150.0);
                gates::transform_value(raw, co)
            };
            out.insert(name.clone(), (val, cv.status));
        }
        out
    }

    /// 重算一个分支的所有门结果。返回 (写入的结果行数, 受影响 stale 标记数)。
    /// 边界点：保留全部合法候选行（inclusive/exclusive/boundary-only）。
    pub fn recompute_branch(
        &self,
        tx: &Connection,
        branch: &str,
        source_event_seq: i64,
    ) -> AppResult<(usize, usize)> {
        let pipe = self.load_pipeline(branch)?;
        let mut written = 0usize;
        let mut staled = 0usize;

        // 1) 失效传播：把引用旧版依赖的 current 结果标记 stale（不覆盖数值）
        staled += self.mark_stale(tx, branch, &pipe)?;

        // 2) 计算每个事件的坐标
        let coords: Vec<(String, BTreeMap<String, (f64, crate::domain::ValueStatus)>)> = pipe
            .events
            .iter()
            .map(|ev| (ev.id.0.clone(), Self::event_coordinates(&pipe, ev)))
            .collect();

        // 3) 按拓扑序（树按父在前存储）计算每个门
        let mut gate_result: BTreeMap<String, (String, usize, usize)> = BTreeMap::new();
        // gate_id -> (result_id, count, denominator)
        let mut parent_members: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        parent_members.insert(
            "root".into(),
            coords
                .iter()
                .map(|(id, _)| id.clone())
                .collect::<BTreeSet<String>>(),
        );

        for gate in &pipe.tree.gates {
            let parent_key = gate.parent.clone();
            let parent_set = parent_members
                .get(&parent_key)
                .cloned()
                .unwrap_or_else(|| coords.iter().map(|(id, _)| id.clone()).collect());
            let (denominator, parent_ref) = if gate.parent == "root" {
                (parent_set.len(), ROOT_RESULT.to_string())
            } else {
                let (_, count, _) = gate_result.get(&gate.parent).cloned().ok_or_else(|| {
                    AppError::Case(format!("门 {} 的父 {} 尚未计算", gate.stable_id, gate.parent))
                })?;
                (count, gate_result.get(&gate.parent).unwrap().0.clone())
            };

            // 收集候选事件坐标
            let scoped: Vec<(String, f64, f64)> = coords
                .iter()
                .filter(|(id, _)| parent_set.contains(id))
                .map(|(id, c)| {
                    let (x, _) = c.get(&gate.x_channel).copied().unwrap_or((f64::NAN, crate::domain::ValueStatus::Invalid));
                    let (y, _) = c.get(&gate.y_channel).copied().unwrap_or((f64::NAN, crate::domain::ValueStatus::Invalid));
                    (id.clone(), x, y)
                })
                .collect();
            let cand = gates::classify_events(gate, &scoped);

            let candidates: Vec<(&str, Vec<String>)> = vec![
                ("inclusive", cand.inclusive.clone()),
                ("exclusive", cand.exclusive.clone()),
            ];
            if cand.boundary {
                // 边界点：保留无法唯一决定时的全部合法候选
                let boundary = cand.boundary_only.clone();
                let mut rows = candidates;
                rows.push(("boundary-only", boundary));
                for (label, members) in rows {
                    written += self.insert_result(
                        tx, branch, gate, label, &members, denominator, &parent_ref,
                        &pipe, source_event_seq,
                    )?;
                }
                // 谱系默认走 exclusive（未裁定前的保守选择），但所有候选行都已保存
                parent_members.insert(
                    gate.stable_id.clone(),
                    cand.exclusive.iter().cloned().collect(),
                );
                gate_result.insert(
                    gate.stable_id.clone(),
                    (
                        format!("res:{}:{}:{}", branch, gate.stable_id, "exclusive"),
                        cand.exclusive.len(),
                        denominator,
                    ),
                );
            } else {
                for (label, members) in candidates {
                    written += self.insert_result(
                        tx, branch, gate, label, &members, denominator, &parent_ref,
                        &pipe, source_event_seq,
                    )?;
                }
                parent_members.insert(
                    gate.stable_id.clone(),
                    cand.exclusive.iter().cloned().collect(),
                );
                gate_result.insert(
                    gate.stable_id.clone(),
                    (
                        format!("res:{}:{}:{}", branch, gate.stable_id, "exclusive"),
                        cand.exclusive.len(),
                        denominator,
                    ),
                );
            }
        }
        Ok((written, staled))
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_result(
        &self,
        tx: &Connection,
        branch: &str,
        gate: &GatePolygon,
        label: &str,
        members: &[String],
        denominator: usize,
        parent_ref: &str,
        pipe: &Pipeline,
        source_event_seq: i64,
    ) -> AppResult<usize> {
        let result_id = format!("res:{}:{}:{}", branch, gate.stable_id, label);
        let version: i64 = tx.query_row(
            "SELECT COALESCE(MAX(version),0)+1 FROM results WHERE id=?1 AND branch_id=?2",
            params![result_id, branch],
            |r| r.get(0),
        )?;
        let count = members.len();
        let percent = if denominator > 0 {
            Some(count as f64 / denominator as f64 * 100.0)
        } else {
            None
        };
        let deps = json!({
            "events": "events@latest",
            "comp": format!("comp@{}", pipe.comp_version),
            "xform": format!("xform@{}", pipe.xform_version),
            "gates": format!("gates@{}", pipe.tree_version),
            "gate_stable_id": gate.stable_id,
            "gate_geometry": gate.vertices,
            "parent_result": parent_ref,
            "denominator_source": parent_ref,
            "denominator_pinned": denominator,
        });
        tx.execute(
            "INSERT INTO results(id,version,branch_id,gate_stable_id,candidate_label,parent_result_ref,
              numerator_count,denominator_count,percent,member_ids_json,deps_json,source_event_seq,status,stale_reason)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'current',NULL)",
            params![
                result_id,
                version,
                branch,
                gate.stable_id,
                label,
                parent_ref,
                count as i64,
                denominator as i64,
                percent,
                serde_json::to_string(members).unwrap(),
                deps.to_string(),
                source_event_seq,
            ],
        )?;
        Ok(1)
    }

    /// 失效传播：把依赖旧版制品的 current 结果标记 stale。
    /// 门的百分比分母钉住父群版本；上游修订只标记过期，不覆盖。
    fn mark_stale(&self, tx: &Connection, branch: &str, pipe: &Pipeline) -> AppResult<usize> {
        let mut stmt = tx.prepare(
            "SELECT rowid, deps_json FROM results WHERE branch_id=?1 AND status='current'",
        )?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([branch], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        let mut staled = 0usize;
        for (rowid, deps_json) in rows {
            let deps: serde_json::Value = serde_json::from_str(&deps_json).unwrap_or(json!({}));
            let mut reason: Option<String> = None;
            for (key, current) in [
                ("comp", pipe.comp_version),
                ("xform", pipe.xform_version),
                ("gates", pipe.tree_version),
            ] {
                let dep = deps
                    .get(key)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let used: i64 = dep.rsplit('@').next().and_then(|s| s.parse().ok()).unwrap_or(0);
                if used > 0 && used != current {
                    reason = Some(format!(
                        "上游 {} 版本变化：结果基于 {}，当前 {}@{}",
                        key, dep, key, current
                    ));
                }
            }
            if let Some(r) = reason {
                tx.execute(
                    "UPDATE 'x' WHERE 0",
                    [],
                )
                .ok();
                tx.execute(
                    "UPDATE results SET status='stale', stale_reason=?1 WHERE rowid=?2",
                    params![r, rowid],
                )?;
                staled += 1;
            }
        }
        Ok(staled)
    }
}
