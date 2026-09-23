//! SQLite 持久层。
//!
//! 设计要点：
//! - artifacts 内容寻址、不可变；分支在 branch_artifacts 中只保存版本指针。
//! - events 是只增日志（seq 每分支单调递增），任何裁定/修订都新增事件。
//! - results 保存评估结果；上游变化时旧行置 stale（status + stale_reason），不删除不覆盖。
//! - decisions 保存多候选的人工裁定分支。
//! - 所有写操作在事务中，失败整笔回滚，不留半份记录。

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::{
    stable_id, Artifact, ArtifactData, ArtifactKind, CompensationResult, GateResult,
};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug)]
pub struct StoreError(pub String);
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "store error: {}", self.0)
    }
}
impl std::error::Error for StoreError {}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError(e.to_string())
    }
}
impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError(e.to_string())
    }
}

pub fn err<T>(s: impl Into<String>) -> Result<T, StoreError> {
    Err(StoreError(s.into()))
}

pub struct Store {
    pub conn: Connection,
}

/// 迁移记录（状态迁移测试用：从 v0 起逐版升级）。
pub fn current_schema_version(conn: &Connection) -> Result<u32, StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    let v: Option<String> = conn
        .query_row("SELECT value FROM schema_meta WHERE key='version'", [], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

fn apply_v1(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        CREATE TABLE artifacts (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            stem TEXT NOT NULL,
            version INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            data_json TEXT NOT NULL,
            UNIQUE(kind, stem, version)
        );
        CREATE TABLE branches (
            name TEXT PRIMARY KEY,
            created_seq INTEGER NOT NULL DEFAULT 0,
            note TEXT NOT NULL DEFAULT ''
        );
        -- 分支 -> 当前可见的制品版本指针（as-of 查询走 events 日志）。
        CREATE TABLE branch_artifacts (
            branch TEXT NOT NULL,
            kind TEXT NOT NULL,
            stem TEXT NOT NULL,
            artifact_id TEXT NOT NULL,
            seq INTEGER NOT NULL,
            PRIMARY KEY (branch, kind, stem)
        );
        CREATE TABLE events (
            seq INTEGER NOT NULL,
            branch TEXT NOT NULL,
            kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (branch, seq)
        );
        CREATE TABLE results (
            id TEXT PRIMARY KEY,
            branch TEXT NOT NULL,
            scope TEXT NOT NULL,            -- 'comp' 或 gate stem
            seq INTEGER NOT NULL,
            status TEXT NOT NULL,
            inputs_json TEXT NOT NULL,      -- 输入依赖版本钉住
            result_json TEXT NOT NULL,
            stale_reason TEXT NOT NULL DEFAULT '',
            UNIQUE(branch, scope, seq)
        );
        CREATE TABLE decisions (
            id TEXT PRIMARY KEY,
            branch TEXT NOT NULL,
            seq INTEGER NOT NULL,
            result_id TEXT NOT NULL,
            candidate TEXT NOT NULL,       -- 'inverse'/'reject'/'pseudoinverse'/'boundary-inside' ...
            action TEXT NOT NULL,          -- accept / reject / adjust
            note TEXT NOT NULL DEFAULT '',
            UNIQUE(branch, seq)
        );
        CREATE TABLE provenance (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            branch TEXT NOT NULL,
            seq INTEGER NOT NULL,
            result_id TEXT NOT NULL,
            artifact_id TEXT NOT NULL,
            role TEXT NOT NULL
        );
        "#,
    )?;
    Ok(())
}

/// 迁移：幂等；从当前版本逐档升到最新。
pub fn migrate(conn: &mut Connection) -> Result<u32, StoreError> {
    let current = current_schema_version(conn)?;
    if current < 1 {
        let tx = conn.transaction()?;
        apply_v1(&tx)?;
        tx.execute(
            "INSERT OR REPLACE INTO schema_meta(key,value) VALUES ('version','1')",
            [],
        )?;
        tx.commit()?;
    }
    // 未来 v2/v3 在此 continue。
    current_schema_version(conn)
}

impl Store {
    pub fn open(path: &str) -> Result<Store, StoreError> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "on")?;
        // WAL 让两个窗口并发保存时的冲突检测确定。
        conn.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&mut conn)?;
        Ok(Store { conn })
    }

    /// 内存库（测试用）。
    pub fn open_in_memory() -> Result<Store, StoreError> {
        let mut conn = Connection::open_in_memory()?;
        migrate(&mut conn)?;
        Ok(Store { conn })
    }

    pub fn schema_version(&self) -> Result<u32, StoreError> {
        current_schema_version(&self.conn)
    }

    // ---------- 分支 ----------

    pub fn ensure_branch(&self, name: &str, note: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO branches(name,note) VALUES (?1,?2)",
            params![name, note],
        )?;
        Ok(())
    }

    pub fn list_branches(&self) -> Result<Vec<String>, StoreError> {
        let mut stmt = self.conn.prepare("SELECT name FROM branches ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ---------- 制品（内容寻址） ----------

    /// 写入一个新版本制品。若内容已存在则返回既有 id（哈希相同=同一版本身份）。
    pub fn put_artifact(
        &self,
        kind: ArtifactKind,
        stem: &str,
        version: u32,
        data: &ArtifactData,
    ) -> Result<Artifact, StoreError> {
        if kind.as_str() != data.kind() {
            return err("制品 kind 与 data 类型不一致");
        }
        let data_json = serde_json::to_string(data)?;
        let mut hasher = Sha256::new();
        hasher.update(kind.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(stem.as_bytes());
        hasher.update(b"\0");
        hasher.update(version.to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(data_json.as_bytes());
        let sha = format!("{:x}", hasher.finalize());
        let id = stable_id(kind.as_str(), stem, version);
        self.conn.execute(
            "INSERT OR IGNORE INTO artifacts(id,kind,stem,version,sha256,data_json)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, kind.as_str(), stem, version, sha, data_json],
        )?;
        Ok(Artifact {
            id,
            kind: kind.as_str().into(),
            stem: stem.into(),
            version,
            sha256: sha,
            data: data.clone(),
        })
    }

    pub fn get_artifact(&self, id: &str) -> Result<Option<Artifact>, StoreError> {
        self.conn
            .query_row(
                "SELECT id,kind,stem,version,sha256,data_json FROM artifacts WHERE id=?1",
                params![id],
                |r| {
                    let data_json: String = r.get(5)?;
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, u32>(3)?,
                        r.get::<_, String>(4)?,
                        data_json,
                    ))
                },
            )
            .optional()?
            .map(|(id, kind, stem, version, sha, dj)| {
                let data: ArtifactData = serde_json::from_str(&dj)?;
                Ok(Artifact {
                    id,
                    kind,
                    stem,
                    version,
                    sha256: sha,
                    data,
                })
            })
            .transpose()
    }

    pub fn all_artifacts(&self) -> Result<Vec<Artifact>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM artifacts ORDER BY kind, stem, version")?;
        let ids: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        ids.into_iter()
            .map(|id| self.get_artifact(&id).map(Option::unwrap))
            .collect()
    }

    /// 分支当前指向的某 stem 制品。
    pub fn branch_artifact(
        &self,
        branch: &str,
        kind: ArtifactKind,
        stem: &str,
    ) -> Result<Option<Artifact>, StoreError> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT artifact_id FROM branch_artifacts WHERE branch=?1 AND kind=?2 AND stem=?3",
                params![branch, kind.as_str(), stem],
                |r| r.get(0),
            )
            .optional()?;
        match id {
            Some(id) => self.get_artifact(&id),
            None => Ok(None),
        }
    }
}

// ---------- 事件日志载荷 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum EventPayload {
    /// 制品版本被引入分支（导入/修订/重解释）。
    ArtifactCommitted {
        artifact_id: String,
        kind: String,
        stem: String,
        version: u32,
        sha256: String,
        #[serde(default)]
        supersedes: Option<String>,
        #[serde(default)]
        reason: String,
    },
    /// 自动/手动评估结果落地。
    ResultRecorded {
        result_id: String,
        scope: String,
        status: String,
        inputs: serde_json::Value,
        #[serde(default)]
        stale_of: Option<String>,
    },
    /// 旧结果被标记过期（不覆盖）。
    MarkedStale {
        result_id: String,
        scope: String,
        reason: String,
    },
    /// 人工裁定（accept/reject/adjust），candidate 标识所选项。
    Decision {
        result_id: String,
        candidate: String,
        action: String,
        note: String,
    },
    /// 冲突的人工决定（两个分支互斥决定各自保留）。
    ConflictingDecision {
        other_branch: String,
        other_candidate: String,
        note: String,
    },
    /// 重放脚本中的固定场景步骤（反例档案）。
    ScenarioStep {
        code: String,
        title: String,
        accepted: bool,
        note: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct LoggedEvent {
    pub seq: u64,
    pub branch: String,
    pub payload: EventPayload,
    pub created_at: String,
}

// ---------- 记录结果 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultRow {
    pub id: String,
    pub branch: String,
    pub scope: String,
    pub seq: u64,
    pub status: String,
    pub inputs: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub comp: Option<CompensationResult>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub gate: Option<GateResult>,
    pub stale_reason: String,
}

fn result_id(branch: &str, scope: &str, seq: u64) -> String {
    format!("result:{branch}:{scope}:s{seq}")
}

impl Store {
    /// 在单个事务里：追加事件 + 更新分支指针 + 写制品（若携带）。
    /// 任一步失败整笔回滚，不留半份记录。
    pub fn commit_artifact(
        &mut self,
        branch: &str,
        kind: ArtifactKind,
        stem: &str,
        data: &ArtifactData,
        reason: &str,
        base_version: Option<u32>,
    ) -> Result<Artifact, StoreError> {
        self.ensure_branch(branch, "")?;
        let tx = self.conn.transaction()?;
        // 同版本并发修改检测：客户端必须基于当前最新版本提交。
        let current: Option<u32> = tx
            .query_row(
                "SELECT MAX(version) FROM artifacts WHERE kind=?1 AND stem=?2",
                params![kind.as_str(), stem],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let next = current.unwrap_or(0) + 1;
        if let Some(base) = base_version {
            if current != Some(base) {
                return Err(StoreError(format!(
                    "VERSION_CONFLICT: stem={stem} 当前版本 v{}，你基于 v{} 提交",
                    current.unwrap_or(0),
                    base
                )));
            }
        }
        let data_json = serde_json::to_string(data)?;
        let mut hasher = Sha256::new();
        hasher.update(kind.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(stem.as_bytes());
        hasher.update(b"\0");
        hasher.update(next.to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(data_json.as_bytes());
        let sha = format!("{:x}", hasher.finalize());
        let id = stable_id(kind.as_str(), stem, next);
        tx.execute(
            "INSERT OR IGNORE INTO artifacts(id,kind,stem,version,sha256,data_json)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![id, kind.as_str(), stem, next, sha, data_json],
        )?;
        let seq = next_event_seq(&tx, branch)?;
        // 同分支中旧指针版本（供 supersedes 记录）。
        let supersedes: Option<String> = tx
            .query_row(
                "SELECT artifact_id FROM branch_artifacts WHERE branch=?1 AND kind=?2 AND stem=?3",
                params![branch, kind.as_str(), stem],
                |r| r.get(0),
            )
            .optional()?;
        tx.execute(
            "INSERT INTO branch_artifacts(branch,kind,stem,artifact_id,seq)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(branch,kind,stem) DO UPDATE SET artifact_id=excluded.artifact_id, seq=excluded.seq",
            params![branch, kind.as_str(), stem, id, seq],
        )?;
        let payload = EventPayload::ArtifactCommitted {
            artifact_id: id.clone(),
            kind: kind.as_str().into(),
            stem: stem.into(),
            version: next,
            sha256: sha.clone(),
            supersedes,
            reason: reason.into(),
        };
        insert_event(&tx, branch, seq, &payload)?;
        tx.commit()?;
        Ok(Artifact {
            id,
            kind: kind.as_str().into(),
            stem: stem.into(),
            version: next,
            sha256: sha,
            data: data.clone(),
        })
    }

    pub fn next_seq(&self, branch: &str) -> Result<u64, StoreError> {
        Ok(next_event_seq(&self.conn, branch)?)
    }

    /// 追加任意事件（用于裁定、场景步骤等）。
    pub fn append_event(
        &mut self,
        branch: &str,
        payload: &EventPayload,
    ) -> Result<u64, StoreError> {
        self.ensure_branch(branch, "")?;
        let tx = self.conn.transaction()?;
        let seq = next_event_seq(&tx, branch)?;
        insert_event(&tx, branch, seq, payload)?;
        if let EventPayload::Decision { result_id, candidate, action, note } = payload {
            tx.execute(
                "INSERT INTO decisions(id,branch,seq,result_id,candidate,action,note)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    format!("decision:{branch}:s{seq}"),
                    branch,
                    seq,
                    result_id,
                    candidate,
                    action,
                    note
                ],
            )?;
        }
        tx.commit()?;
        Ok(seq)
    }

    /// 记录评估结果（comp 或 gate），附输入依赖版本与来源链。
    pub fn record_result(
        &mut self,
        branch: &str,
        scope: &str,
        status: &str,
        inputs: &serde_json::Value,
        comp: Option<&CompensationResult>,
        gate: Option<&GateResult>,
        provenance: &[(String, String)],
    ) -> Result<(String, u64), StoreError> {
        let tx = self.conn.transaction()?;
        let seq = next_event_seq(&tx, branch)?;
        let id = result_id(branch, scope, seq);
        let result_json = serde_json::to_string(&serde_json::json!({
            "comp": comp, "gate": gate,
        }))?;
        tx.execute(
            "INSERT INTO results(id,branch,scope,seq,status,inputs_json,result_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![id, branch, scope, seq, status, inputs.to_string(), result_json],
        )?;
        for (artifact_id, role) in provenance {
            tx.execute(
                "INSERT INTO provenance(branch,seq,result_id,artifact_id,role)
                 VALUES (?1,?2,?3,?4,?5)",
                params![branch, seq, id, artifact_id, role],
            )?;
        }
        let payload = EventPayload::ResultRecorded {
            result_id: id.clone(),
            scope: scope.into(),
            status: status.into(),
            inputs: inputs.clone(),
            stale_of: None,
        };
        insert_event(&tx, branch, seq, &payload)?;
        tx.commit()?;
        Ok((id, seq))
    }

    /// 上游修订：把同 scope 旧结果标记 stale（保留内容，不覆盖）。
    pub fn mark_stale(
        &mut self,
        branch: &str,
        scope: &str,
        before_seq: u64,
        reason: &str,
    ) -> Result<usize, StoreError> {
        let tx = self.conn.transaction()?;
        let affected = tx.execute(
            "UPDATE results SET status='stale', stale_reason=?1
             WHERE branch=?2 AND scope=?3 AND seq<?4 AND status!='stale'",
            params![reason, branch, scope, before_seq],
        )?;
        if affected > 0 {
            let seq = next_event_seq(&tx, branch)?;
            let ids = result_ids_before(&tx, branch, scope, before_seq)?;
            for rid in ids {
                let payload = EventPayload::MarkedStale {
                    result_id: rid,
                    scope: scope.into(),
                    reason: reason.into(),
                };
                insert_event(&tx, branch, seq, &payload)?;
            }
        }
        tx.commit()?;
        Ok(affected)
    }
}

fn next_event_seq(conn: &Connection, branch: &str) -> Result<u64, rusqlite::Error> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE branch=?1",
        params![branch],
        |r| r.get(0),
    )
}

fn insert_event(
    conn: &Connection,
    branch: &str,
    seq: u64,
    payload: &EventPayload,
) -> Result<(), rusqlite::Error> {
    let kind = match payload {
        EventPayload::ArtifactCommitted { .. } => "artifact-committed",
        EventPayload::ResultRecorded { .. } => "result-recorded",
        EventPayload::MarkedStale { .. } => "marked-stale",
        EventPayload::Decision { .. } => "decision",
        EventPayload::ConflictingDecision { .. } => "conflicting-decision",
        EventPayload::ScenarioStep { .. } => "scenario-step",
    };
    let json = serde_json::to_string(payload).expect("event payload serializable");
    conn.execute(
        "INSERT INTO events(seq,branch,kind,payload_json) VALUES (?1,?2,?3,?4)",
        params![seq, branch, kind, json],
    )?;
    Ok(())
}

fn result_ids_before(
    conn: &Connection,
    branch: &str,
    scope: &str,
    before_seq: u64,
) -> Result<Vec<String>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id FROM results WHERE branch=?1 AND scope=?2 AND seq<?3 AND stale_reason='' ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![branch, scope, before_seq], |r| r.get::<_, String>(0))?;
    rows.collect()
}

// ---------- 读取 / 时间旅行 ----------

impl Store {
    pub fn events(&self, branch: &str) -> Result<Vec<LoggedEvent>, StoreError> {
        self.events_asof(branch, u64::MAX)
    }

    /// 回到任意旧事件：只重放该分支 seq <= asof_seq 的事件，
    /// 重建当时的分支指针与可见结论，绝不拿最新结果覆盖历史。
    pub fn events_asof(
        &self,
        branch: &str,
        asof_seq: u64,
    ) -> Result<Vec<LoggedEvent>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT seq,branch,payload_json,created_at FROM events
             WHERE branch=?1 AND seq<=?2 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![branch, asof_seq], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, branch, pj, created_at) = row?;
            let payload: EventPayload = serde_json::from_str(&pj)?;
            out.push(LoggedEvent {
                seq,
                branch,
                payload,
                created_at,
            });
        }
        Ok(out)
    }

    /// 截至某事件时，分支可见的制品版本映射（kind:stem -> artifact_id）。
    pub fn view_asof(
        &self,
        branch: &str,
        asof_seq: u64,
    ) -> Result<std::collections::BTreeMap<String, String>, StoreError> {
        let mut view = std::collections::BTreeMap::new();
        for ev in self.events_asof(branch, asof_seq)? {
            if let EventPayload::ArtifactCommitted {
                kind,
                stem,
                artifact_id,
                ..
            } = &ev.payload
            {
                view.insert(format!("{kind}:{stem}"), artifact_id.clone());
            }
        }
        Ok(view)
    }

    /// 当前/历史某 stem 的制品 id。
    pub fn artifact_asof(
        &self,
        branch: &str,
        kind: ArtifactKind,
        stem: &str,
        asof_seq: u64,
    ) -> Result<Option<Artifact>, StoreError> {
        let view = self.view_asof(branch, asof_seq)?;
        match view.get(&format!("{}:{}", kind.as_str(), stem)) {
            Some(id) => self.get_artifact(id),
            None => Ok(None),
        }
    }

    pub fn results(&self, branch: &str) -> Result<Vec<ResultRow>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id,branch,scope,seq,status,inputs_json,result_json,stale_reason
             FROM results WHERE branch=?1 ORDER BY scope, seq",
        )?;
        let rows = stmt.query_map(params![branch], map_result_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn result(&self, id: &str) -> Result<Option<ResultRow>, StoreError> {
        match self
            .conn
            .query_row(
                "SELECT id,branch,scope,seq,status,inputs_json,result_json,stale_reason
                 FROM results WHERE id=?1",
                params![id],
                map_result_row,
            )
            .optional()?
        {
            Some(r) => Ok(Some(r)),
            None => Ok(None),
        }
    }

    /// 结果的来源链：哪些制品以什么角色参与。
    pub fn provenance(&self, result_id: &str) -> Result<Vec<(String, String)>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT artifact_id, role FROM provenance WHERE result_id=?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![result_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn decisions(&self, branch: &str) -> Result<Vec<DecisionRow>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT d.id,d.seq,d.result_id,d.candidate,d.action,d.note
             FROM decisions d WHERE d.branch=?1 ORDER BY d.seq",
        )?;
        let rows = stmt.query_map(params![branch], |r| {
            Ok(DecisionRow {
                id: r.get(0)?,
                seq: r.get(1)?,
                result_id: r.get(2)?,
                candidate: r.get(3)?,
                action: r.get(4)?,
                note: r.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionRow {
    pub id: String,
    pub seq: u64,
    pub result_id: String,
    pub candidate: String,
    pub action: String,
    pub note: String,
}

fn map_result_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ResultRow> {
    let result_json: String = r.get(6)?;
    let parsed: serde_json::Value =
        serde_json::from_str(&result_json).unwrap_or(serde_json::Value::Null);
    let comp = parsed
        .get("comp")
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value::<CompensationResult>(v.clone()).ok());
    let gate = parsed
        .get("gate")
        .filter(|v| !v.is_null())
        .and_then(|v| serde_json::from_value::<GateResult>(v.clone()).ok());
    let inputs: String = r.get(5)?;
    Ok(ResultRow {
        id: r.get(0)?,
        branch: r.get(1)?,
        scope: r.get(2)?,
        seq: r.get(3)?,
        status: r.get(4)?,
        inputs: serde_json::from_str(&inputs).unwrap_or(serde_json::Value::Null),
        comp,
        gate,
        stale_reason: r.get(7)?,
    })
}

// ---------- 导出 / 导入（身份、顺序、来源关系保持） ----------

#[derive(Debug, Serialize, Deserialize)]
pub struct Bundle {
    pub format: String,
    pub schema_version: u32,
    pub artifacts: Vec<ArtifactBundleRow>,
    pub branches: Vec<String>,
    pub branch_artifacts: Vec<BranchPointer>,
    pub events: Vec<EventBundleRow>,
    pub results: Vec<ResultBundleRow>,
    pub decisions: Vec<DecisionBundleRow>,
    pub provenance: Vec<ProvBundleRow>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ArtifactBundleRow {
    pub id: String,
    pub kind: String,
    pub stem: String,
    pub version: u32,
    pub sha256: String,
    pub data_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchPointer {
    pub branch: String,
    pub kind: String,
    pub stem: String,
    pub artifact_id: String,
    pub seq: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[derive(Clone)]
pub struct EventBundleRow {
    pub seq: u64,
    pub branch: String,
    pub kind: String,
    pub payload_json: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ResultBundleRow {
    pub id: String,
    pub branch: String,
    pub scope: String,
    pub seq: u64,
    pub status: String,
    pub inputs_json: String,
    pub result_json: String,
    pub stale_reason: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DecisionBundleRow {
    pub id: String,
    pub branch: String,
    pub seq: u64,
    pub result_id: String,
    pub candidate: String,
    pub action: String,
    pub note: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProvBundleRow {
    pub branch: String,
    pub seq: u64,
    pub result_id: String,
    pub artifact_id: String,
    pub role: String,
}

impl Store {
    pub fn export_bundle(&self) -> Result<Bundle, StoreError> {
        let artifacts = {
            let mut stmt = self.conn.prepare(
                "SELECT id,kind,stem,version,sha256,data_json FROM artifacts ORDER BY kind,stem,version",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ArtifactBundleRow {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    stem: r.get(2)?,
                    version: r.get(3)?,
                    sha256: r.get(4)?,
                    data_json: r.get(5)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let branches = self.list_branches()?;
        let branch_artifacts = {
            let mut stmt = self.conn.prepare(
                "SELECT branch,kind,stem,artifact_id,seq FROM branch_artifacts ORDER BY branch,kind,stem",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(BranchPointer {
                    branch: r.get(0)?,
                    kind: r.get(1)?,
                    stem: r.get(2)?,
                    artifact_id: r.get(3)?,
                    seq: r.get(4)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let events = {
            let mut stmt = self
                .conn
                .prepare("SELECT seq,branch,kind,payload_json,created_at FROM events ORDER BY branch,seq")?;
            let rows = stmt.query_map([], |r| {
                Ok(EventBundleRow {
                    seq: r.get(0)?,
                    branch: r.get(1)?,
                    kind: r.get(2)?,
                    payload_json: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let results = {
            let mut stmt = self.conn.prepare(
                "SELECT id,branch,scope,seq,status,inputs_json,result_json,stale_reason FROM results ORDER BY branch,scope,seq",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ResultBundleRow {
                    id: r.get(0)?,
                    branch: r.get(1)?,
                    scope: r.get(2)?,
                    seq: r.get(3)?,
                    status: r.get(4)?,
                    inputs_json: r.get(5)?,
                    result_json: r.get(6)?,
                    stale_reason: r.get(7)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let decisions = {
            let mut stmt = self.conn.prepare(
                "SELECT id,branch,seq,result_id,candidate,action,note FROM decisions ORDER BY branch,seq",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(DecisionBundleRow {
                    id: r.get(0)?,
                    branch: r.get(1)?,
                    seq: r.get(2)?,
                    result_id: r.get(3)?,
                    candidate: r.get(4)?,
                    action: r.get(5)?,
                    note: r.get(6)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let provenance = {
            let mut stmt = self.conn.prepare(
                "SELECT branch,seq,result_id,artifact_id,role FROM provenance ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ProvBundleRow {
                    branch: r.get(0)?,
                    seq: r.get(1)?,
                    result_id: r.get(2)?,
                    artifact_id: r.get(3)?,
                    role: r.get(4)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        Ok(Bundle {
            format: "flowline-bundle-v1".into(),
            schema_version: SCHEMA_VERSION,
            artifacts,
            branches,
            branch_artifacts,
            events,
            results,
            decisions,
            provenance,
        })
    }
}

/// 仅允许导入到空库（避免与既有身份冲突）；整体一个事务，失败回滚。
pub fn import_bundle(conn: &mut Connection, bundle: &Bundle) -> Result<(), StoreError> {
    if bundle.format != "flowline-bundle-v1" {
        return err(format!("不支持的导出格式 {}", bundle.format));
    }
    if bundle.schema_version > SCHEMA_VERSION {
        return err(format!(
            "导出来自更新的 schema v{}，本机 v{}",
            bundle.schema_version, SCHEMA_VERSION
        ));
    }
    migrate(conn)?;
    let tx = conn.transaction()?;
    let count: i64 = tx.query_row("SELECT COUNT(*) FROM artifacts", [], |r| r.get(0))?;
    if count != 0 {
        return err("目标数据库非空；导入只允许进入空库（删除本地数据库后再导入）");
    }
    // 校验：每个 data_json 可解析、id 与 kind:stem:v{n} 一致；sha256 可复算。
    for a in &bundle.artifacts {
        let expected_id = stable_id(&a.kind, &a.stem, a.version);
        if a.id != expected_id {
            return err(format!("制品身份不一致：{} != {}", a.id, expected_id));
        }
        let data: ArtifactData = serde_json::from_str(&a.data_json)
            .map_err(|e| StoreError(format!("制品 {} 数据损坏：{e}", a.id)))?;
        if data.kind() != a.kind {
            return err(format!("制品 {} 的 kind 与数据不符", a.id));
        }
        let mut hasher = Sha256::new();
        hasher.update(a.kind.as_bytes());
        hasher.update(b"\0");
        hasher.update(a.stem.as_bytes());
        hasher.update(b"\0");
        hasher.update(a.version.to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(a.data_json.as_bytes());
        let sha = format!("{:x}", hasher.finalize());
        if sha != a.sha256 {
            return err(format!("制品 {} 内容哈希不匹配，拒绝导入", a.id));
        }
        tx.execute(
            "INSERT INTO artifacts(id,kind,stem,version,sha256,data_json)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![a.id, a.kind, a.stem, a.version, a.sha256, a.data_json],
        )?;
    }
    for b in &bundle.branches {
        tx.execute(
            "INSERT OR IGNORE INTO branches(name,note) VALUES (?1,'')",
            params![b],
        )?;
    }
    for p in &bundle.branch_artifacts {
        tx.execute(
            "INSERT INTO branch_artifacts(branch,kind,stem,artifact_id,seq)
             VALUES (?1,?2,?3,?4,?5)",
            params![p.branch, p.kind, p.stem, p.artifact_id, p.seq],
        )?;
    }
    // 事件按 (branch, seq) 顺序恢复，保持顺序与时间线。
    let mut evs = bundle.events.clone();
    evs.sort_by(|a, b| a.branch.cmp(&b.branch).then(a.seq.cmp(&b.seq)));
    for e in &evs {
        // 校验 payload 可反序列化，坏档案整笔回滚。
        let _: EventPayload = serde_json::from_str(&e.payload_json)
            .map_err(|err| StoreError(format!("事件 {}/{} 损坏：{err}", e.branch, e.seq)))?;
        tx.execute(
            "INSERT INTO events(seq,branch,kind,payload_json,created_at)
             VALUES (?1,?2,?3,?4,?5)",
            params![e.seq, e.branch, e.kind, e.payload_json, e.created_at],
        )?;
    }
    for r in &bundle.results {
        tx.execute(
            "INSERT INTO results(id,branch,scope,seq,status,inputs_json,result_json,stale_reason)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                r.id, r.branch, r.scope, r.seq, r.status, r.inputs_json, r.result_json,
                r.stale_reason
            ],
        )?;
    }
    for d in &bundle.decisions {
        tx.execute(
            "INSERT INTO decisions(id,branch,seq,result_id,candidate,action,note)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                d.id, d.branch, d.seq, d.result_id, d.candidate, d.action, d.note
            ],
        )?;
    }
    for p in &bundle.provenance {
        tx.execute(
            "INSERT INTO provenance(branch,seq,result_id,artifact_id,role)
             VALUES (?1,?2,?3,?4,?5)",
            params![p.branch, p.seq, p.result_id, p.artifact_id, p.role],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// 按依赖关系失效：找出 provenance 引用了指定 kind:stem 任一版本制品的结果，
/// 标记 stale（不删除、不覆盖内容），并为每个结果写 MarkedStale 事件。
/// 返回受影响 (result_id, scope)。
pub fn invalidate_dependents(
    conn: &mut Connection,
    branch: &str,
    kind: &str,
    stem: &str,
    reason: &str,
) -> Result<Vec<(String, String)>, StoreError> {
    let tx = conn.transaction()?;
    let mut stmt = tx.prepare(
        "SELECT DISTINCT r.id, r.scope FROM results r
         JOIN provenance p ON p.result_id = r.id
         JOIN artifacts a ON a.id = p.artifact_id
         WHERE r.branch=?1 AND a.kind=?2 AND a.stem=?3 AND r.status!='stale'",
    )?;
    let affected: Vec<(String, String)> = stmt
        .query_map(params![branch, kind, stem], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for (id, _scope) in &affected {
        tx.execute(
            "UPDATE results SET status='stale', stale_reason=?1 WHERE id=?2",
            params![reason, id],
        )?;
    }
    if !affected.is_empty() {
        let seq = next_event_seq(&tx, branch)?;
        for (id, scope) in &affected {
            let payload = EventPayload::MarkedStale {
                result_id: id.clone(),
                scope: scope.clone(),
                reason: reason.into(),
            };
            insert_event(&tx, branch, seq, &payload)?;
        }
    }
    tx.commit()?;
    Ok(affected)
}

/// 直接把指定结果标记 stale（撤销裁定时恢复前一分支可见结论，不删除被撤销行）。
pub fn stale_result(
    conn: &mut Connection,
    branch: &str,
    result_id: &str,
    reason: &str,
) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    let scope: String = tx.query_row(
        "SELECT scope FROM results WHERE id=?1 AND branch=?2",
        params![result_id, branch],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE results SET status='stale', stale_reason=?1 WHERE id=?2",
        params![reason, result_id],
    )?;
    let seq = next_event_seq(&tx, branch)?;
    let payload = EventPayload::MarkedStale {
        result_id: result_id.into(),
        scope,
        reason: reason.into(),
    };
    insert_event(&tx, branch, seq, &payload)?;
    tx.commit()?;
    Ok(())
}
