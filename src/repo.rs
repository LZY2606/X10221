//! 命令式仓库层：所有写操作都是“追加事件 + 不动旧事实”，并在单事务内完成。

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::domain::{Basis, ContentObject, EventRecord, kind, rule};
use crate::storage::{now_ts, sha256_hex, Store};

#[derive(Debug)]
pub struct RepoError {
    pub status: u16,
    pub message: String,
}
impl RepoError {
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self { status: 409, message: msg.into() }
    }
    pub fn bad(msg: impl Into<String>) -> Self {
        Self { status: 400, message: msg.into() }
    }
}
pub type RepoResult<T> = Result<T, RepoError>;

fn e404(msg: impl Into<String>) -> RepoError {
    RepoError { status: 404, message: msg.into() }
}

impl Store {
    // ---------- 读取 ----------

    pub async fn list_cases(&self) -> RepoResult<Vec<serde_json::Value>> {
        let c = self.conn.lock().await;
        let mut stmt = c
            .prepare("SELECT case_id, title, created_seq FROM cases ORDER BY created_seq, case_id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok(json!({"case_id": r.get::<_, String>(0)?, "title": r.get::<_, String>(1)?}))
            })
            .unwrap();
        Ok(rows.map(|r| r.unwrap()).collect())
    }

    pub async fn head_seq(&self, case_id: &str, branch: &str) -> RepoResult<Option<u64>> {
        let c = self.conn.lock().await;
        c.query_row(
            "SELECT head_seq FROM branch_heads WHERE case_id=?1 AND branch=?2",
            params![case_id, branch],
            |r| r.get::<_, i64>(0),
        )
        .optional()
        .map_err(|e| e404(e.to_string()))
        .map(|v| v.map(|n| n as u64))
    }

    /// 分支祖先链（从自身向上），用于事件回放。
    fn branch_chain(conn: &Connection, case_id: &str, branch: &str) -> Vec<(String, Option<u64>)> {
        let mut chain = Vec::new();
        let mut current = branch.to_string();
        loop {
            let row: Option<(Option<String>, Option<i64>)> = conn
                .query_row(
                    "SELECT parent_branch, fork_after_seq FROM branches WHERE case_id=?1 AND branch=?2",
                    params![case_id, current],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .unwrap();
            let Some((parent, fork)) = row else { break };
            chain.push((current.clone(), fork.map(|v| v as u64)));
            match parent {
                Some(p) => current = p,
                None => break,
            }
        }
        // 最后加上主干根（main 自身，无 fork 限制）
        if !chain.iter().any(|(b, _)| b == "main") {
            chain.push(("main".to_string(), None));
        }
        chain.reverse();
        chain
    }

    /// 回放属于某分支视图的事件（按全局 seq 升序）。at_seq=Some(n) 时只回到第 n 个事件。
    pub async fn branch_events(
        &self,
        case_id: &str,
        branch: &str,
        at_seq: Option<u64>,
    ) -> RepoResult<Vec<EventRecord>> {
        let c = self.conn.lock().await;
        let chain = Self::branch_chain(&c, case_id, branch);
        let mut out = Vec::new();
        for (i, (b, fork_limit)) in chain.iter().enumerate() {
            let lo: i64 = match (i == 0, fork_limit) {
                (false, Some(f)) => *f as i64 + 1,
                _ => 0,
            };
            let mut hi = i64::MAX;
            if let Some(f) = fork_limit {
                hi = *f as i64;
            }
            if let Some(at) = at_seq {
                hi = hi.min(at as i64);
            }
            let mut stmt = c
                .prepare(
                    "SELECT id, seq, branch_seq, event_type, actor, summary, payload, depends_on, basis, created_at
                     FROM events WHERE case_id=?1 AND branch=?2 AND seq>=?3 AND seq<=?4
                     ORDER BY seq",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![case_id, b, lo, hi], row_to_event(case_id, b))
                .unwrap();
            for r in rows {
                out.push(r.unwrap());
            }
        }
        // at_seq 可能指向其他分支的全局序号：严格模式下只返回该分支可见的事件（上面已过滤）
        Ok(out)
    }

    pub async fn get_object(&self, id: &str) -> RepoResult<ContentObject> {
        let c = self.conn.lock().await;
        c.query_row(
            "SELECT id, kind, version, COALESCE(supersedes,''), payload FROM objects WHERE id=?1",
            params![id],
            |r| {
                let sup: String = r.get(3)?;
                Ok(ContentObject {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    version: r.get::<_, i64>(2)? as u32,
                    supersedes: if sup.is_empty() { None } else { Some(sup) },
                    payload: serde_json::from_str(&r.get::<_, String>(4)?).unwrap(),
                })
            },
        )
        .map_err(|_| e404(format!("对象 {id} 不存在")))
    }

    pub async fn latest_version(&self, case_id: &str, kind: &str) -> RepoResult<u32> {
        let c = self.conn.lock().await;
        c.query_row(
            "SELECT COALESCE(MAX(version),0) FROM objects WHERE case_id=?1 AND kind=?2",
            params![case_id, kind],
            |r| r.get::<_, i64>(0),
        )
        .map(|v| v as u32)
        .map_err(|e| e404(e.to_string()))
    }

    pub async fn source_bytes(&self, case_id: &str, role: &str) -> RepoResult<(String, Vec<u8>, String)> {
        let c = self.conn.lock().await;
        c.query_row(
            "SELECT filename, content, sha256 FROM sources WHERE case_id=?1 AND role=?2",
            params![case_id, role],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| e404(format!("来源 {role} 不存在")))
    }
}

fn row_to_event<'a>(
    case_id: &'a str,
    branch: &'a str,
) -> impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<EventRecord> + 'a {
    move |r| {
        let bs: Option<i64> = r.get(2)?;
        Ok(EventRecord {
            id: r.get(0)?,
            seq: r.get::<_, i64>(1)? as u64,
            branch_seq: bs.map(|v| v as u64),
            case_id: case_id.to_string(),
            branch: branch.to_string(),
            event_type: r.get(3)?,
            actor: r.get(4)?,
            summary: r.get(5)?,
            payload: serde_json::from_str(&r.get::<_, String>(6)?).unwrap(),
            depends_on: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
            basis: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
            created_at: r.get(9)?,
        })
    }
}

/// 派生结果读取行（供 state 投影使用）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct GateRunRow {
    pub id: String,
    pub seq: u64,
    pub gate_id: String,
    pub gate_ref: String,
    pub transform_ref: String,
    pub spillover_ref: String,
    pub comp_run_ref: Option<String>,
    pub parent_run_ref: Option<String>,
    pub status: String,
    pub chosen_candidate_id: Option<String>,
    pub candidates: Vec<GateCandidateRow>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GateCandidateRow {
    pub candidate_id: String,
    pub numerator: u64,
    pub denominator: u64,
    pub percent: f64,
    pub denominator_source: String,
    pub member_indices: Vec<u32>,
    pub boundary_indices: Vec<u32>,
    pub unconvertible_indices: Vec<u32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompRunRow {
    pub id: String,
    pub seq: u64,
    pub spillover_ref: String,
    pub status: String,
    pub rcond: Option<f64>,
    pub chosen_candidate_id: Option<String>,
    pub candidates: Vec<CompCandidateRow>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CompCandidateRow {
    pub id: String,
    pub kind: String,
    pub matrix: Vec<Vec<f64>>,
    pub rationale: String,
}

impl Store {
    pub async fn gate_runs(&self, case_id: &str, branch: &str) -> RepoResult<Vec<GateRunRow>> {
        let c = self.conn.lock().await;
        let mut run_stmt = c
            .prepare(
                "SELECT id, seq, gate_id, gate_ref, transform_ref, spillover_ref, comp_run_ref,
                        parent_run_ref, status, chosen_candidate_id
                 FROM gate_runs WHERE case_id=?1 AND branch=?2 ORDER BY seq",
            )
            .unwrap();
        let mut runs: Vec<GateRunRow> = run_stmt
            .query_map(params![case_id, branch], |r| {
                Ok(GateRunRow {
                    id: r.get(0)?,
                    seq: r.get::<_, i64>(1)? as u64,
                    gate_id: r.get(2)?,
                    gate_ref: r.get(3)?,
                    transform_ref: r.get(4)?,
                    spillover_ref: r.get(5)?,
                    comp_run_ref: r.get(6)?,
                    parent_run_ref: r.get(7)?,
                    status: r.get(8)?,
                    chosen_candidate_id: r.get(9)?,
                    candidates: Vec::new(),
                })
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for run in runs.iter_mut() {
            let mut stmt = c
                .prepare(
                    "SELECT candidate_id, member_indices, numerator, denominator, percent,
                            denominator_source, boundary_indices, unconvertible_indices
                     FROM gate_run_candidates WHERE case_id=?1 AND run_id=?2 ORDER BY id",
                )
                .unwrap();
            run.candidates = stmt
                .query_map(params![case_id, run.id], |r| {
                    Ok(GateCandidateRow {
                        candidate_id: r.get(0)?,
                        member_indices: serde_json::from_str(&r.get::<_, String>(1)?).unwrap(),
                        numerator: r.get::<_, i64>(2)? as u64,
                        denominator: r.get::<_, i64>(3)? as u64,
                        percent: r.get(4)?,
                        denominator_source: r.get(5)?,
                        boundary_indices: serde_json::from_str(&r.get::<_, String>(6)?).unwrap(),
                        unconvertible_indices: serde_json::from_str(&r.get::<_, String>(7)?).unwrap(),
                    })
                })
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
        }
        Ok(runs)
    }

    pub async fn comp_runs(&self, case_id: &str, branch: &str) -> RepoResult<Vec<CompRunRow>> {
        let c = self.conn.lock().await;
        let mut run_stmt = c
            .prepare(
                "SELECT id, seq, spillover_ref, status, rcond, chosen_candidate_id
                 FROM comp_runs WHERE case_id=?1 AND branch=?2 ORDER BY seq",
            )
            .unwrap();
        let mut runs: Vec<CompRunRow> = run_stmt
            .query_map(params![case_id, branch], |r| {
                Ok(CompRunRow {
                    id: r.get(0)?,
                    seq: r.get::<_, i64>(1)? as u64,
                    spillover_ref: r.get(2)?,
                    status: r.get(3)?,
                    rcond: r.get(4)?,
                    chosen_candidate_id: r.get(5)?,
                    candidates: Vec::new(),
                })
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        for run in runs.iter_mut() {
            let mut stmt = c
                .prepare(
                    "SELECT id, kind, matrix, rationale FROM comp_candidates
                     WHERE case_id=?1 AND run_id=?2 ORDER BY id",
                )
                .unwrap();
            run.candidates = stmt
                .query_map(params![case_id, run.id], |r| {
                    Ok(CompCandidateRow {
                        id: r.get(0)?,
                        kind: r.get(1)?,
                        matrix: serde_json::from_str(&r.get::<_, String>(2)?).unwrap(),
                        rationale: r.get(3)?,
                    })
                })
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
        }
        Ok(runs)
    }
}
