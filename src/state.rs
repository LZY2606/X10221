//! 世界状态：由追加式事件日志折叠得到；支持 as_of 时间旅行。
use crate::store::{AdjRow, BranchRow, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorldState {
    pub branches: BTreeMap<String, BranchRow>,
    /// 被标记过期的派生集合 (derived_id, version)。
    pub stale_derived: BTreeSet<(String, i64)>,
    /// 被标记过期的群体统计 (stat_id, version)。
    pub stale_popstats: BTreeSet<(String, i64)>,
    /// 已撤销的裁定。
    pub superseded_adjs: BTreeSet<String>,
}

pub fn fold_events(events: &[crate::domain::EventRow]) -> WorldState {
    let mut ws = WorldState::default();
    for ev in events {
        let p = &ev.payload;
        match ev.kind.as_str() {
            "branch.created" => {
                let id = p["branch_id"].as_str().unwrap_or("").to_string();
                ws.branches.insert(
                    id.clone(),
                    BranchRow {
                        branch_id: id,
                        parent: p["parent"].as_str().map(|s| s.to_string()),
                        base_event: p["base_event"].as_i64().unwrap_or(0),
                        head_event: ev.event_id,
                        status: "active".into(),
                    },
                );
            }
            "branch.head" => {
                let id = p["branch_id"].as_str().unwrap_or("");
                if let Some(b) = ws.branches.get_mut(id) {
                    b.head_event = p["head_event"].as_i64().unwrap_or(ev.event_id);
                }
            }
            "derived.stale" => {
                ws.stale_derived.insert((
                    p["derived_id"].as_str().unwrap_or("").to_string(),
                    p["version"].as_i64().unwrap_or(0),
                ));
            }
            "popstat.stale" => {
                ws.stale_popstats.insert((
                    p["stat_id"].as_str().unwrap_or("").to_string(),
                    p["version"].as_i64().unwrap_or(0),
                ));
            }
            "adjudication.reverted" => {
                ws.superseded_adjs.insert(p["adj_id"].as_str().unwrap_or("").to_string());
            }
            _ => {}
        }
    }
    ws
}

impl WorldState {
    /// 分支上针对某 subject 的当前生效裁定（最后一条未撤销的）。
    pub fn active_adjudication<'a>(
        &self,
        store: &Store,
        branch_id: &str,
        subject_id: &str,
    ) -> anyhow::Result<Option<AdjRow>> {
        let all = store.adjudications()?;
        Ok(all
            .into_iter()
            .filter(|a| a.branch_id == branch_id && a.subject_id == subject_id)
            .filter(|a| !self.superseded_adjs.contains(&a.adj_id))
            .max_by_key(|a| a.event))
    }

    pub fn branch_adjudications(&self, store: &Store, branch_id: &str) -> anyhow::Result<Vec<AdjRow>> {
        let all = store.adjudications()?;
        Ok(all
            .into_iter()
            .filter(|a| a.branch_id == branch_id && !self.superseded_adjs.contains(&a.adj_id))
            .collect())
    }
}

/// 载入截至 as_of 的世界状态。
pub fn load_state(store: &Store, as_of: Option<i64>) -> anyhow::Result<WorldState> {
    let events = match as_of {
        Some(e) => store.events_before(e)?,
        None => store.all_events()?,
    };
    Ok(fold_events(&events))
}
