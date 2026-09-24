//! SQLite 谱系存储：来源（原始材料）与解释（派生结果、人工裁定）分开、全部版本化。
//! 所有写操作在事务内完成；导入失败整体回滚，不留半份记录。

use crate::domain;
use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MIGRATIONS: &[&str] = &[MIGRATION_1, MIGRATION_2];

const MIGRATION_1: &str = r#"
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE cases (
  case_id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  description TEXT NOT NULL
);
CREATE TABLE branches (
  id TEXT PRIMARY KEY,
  case_id TEXT NOT NULL REFERENCES cases(case_id),
  branch_id TEXT NOT NULL,
  label TEXT NOT NULL,
  parent_branch TEXT,
  created_event_id TEXT,
  UNIQUE(case_id, branch_id)
);
CREATE TABLE materials (
  id TEXT PRIMARY KEY,
  case_id TEXT NOT NULL,
  lineage_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  version_no INTEGER NOT NULL,
  source_ref TEXT NOT NULL,
  content_json TEXT NOT NULL,
  bytes_sha TEXT NOT NULL,
  rename_only INTEGER NOT NULL DEFAULT 0,
  event_id TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  UNIQUE(case_id, lineage_id, version_no)
);
CREATE TABLE events (
  id TEXT PRIMARY KEY,
  case_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,
  summary TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  recorded_at INTEGER NOT NULL,
  UNIQUE(case_id, branch_id, seq)
);
CREATE TABLE results (
  id TEXT PRIMARY KEY,
  node_id TEXT NOT NULL,
  lineage_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  input_hash TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_event_id TEXT NOT NULL,
  UNIQUE(node_id, input_hash)
);
CREATE TABLE result_candidates (
  result_id TEXT NOT NULL REFERENCES results(id),
  candidate_id TEXT NOT NULL,
  label TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  PRIMARY KEY(result_id, candidate_id)
);
CREATE TABLE marks (
  branch_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  lineage_id TEXT NOT NULL,
  result_id TEXT REFERENCES results(id),
  state TEXT NOT NULL,
  updated_event_id TEXT NOT NULL,
  stale_reason TEXT,
  PRIMARY KEY(branch_id, node_id)
);
CREATE TABLE decisions (
  id TEXT PRIMARY KEY,
  branch_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  action TEXT NOT NULL,
  target_result_id TEXT,
  target_candidate_id TEXT,
  created_event_id TEXT NOT NULL,
  undone_event_id TEXT
);
CREATE TABLE dependencies (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  case_id TEXT NOT NULL,
  from_lineage TEXT NOT NULL,
  to_lineage TEXT NOT NULL,
  rel TEXT NOT NULL,
  UNIQUE(case_id, from_lineage, to_lineage, rel)
);
"#;

const MIGRATION_2: &str = r#"
CREATE TABLE snapshots (
  event_id TEXT PRIMARY KEY REFERENCES events(id),
  heads_json TEXT NOT NULL,
  marks_json TEXT NOT NULL,
  decisions_json TEXT NOT NULL
);
CREATE TABLE findings (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL REFERENCES events(id),
  code TEXT NOT NULL,
  subject TEXT NOT NULL,
  severity TEXT NOT NULL,
  basis_json TEXT NOT NULL
);
CREATE TABLE conflicts (
  id TEXT PRIMARY KEY,
  case_id TEXT NOT NULL,
  branch_id TEXT NOT NULL,
  node_ref TEXT NOT NULL,
  expected_version INTEGER,
  actual_version INTEGER NOT NULL,
  diff_json TEXT NOT NULL,
  event_id TEXT NOT NULL,
  recorded_at INTEGER NOT NULL
);
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictDiff {
    pub node_ref: String,
    pub expected_version: Option<i64>,
    pub actual_version: i64,
    pub expected_content: Value,
    pub attempted_content: Value,
}

pub struct LineageStore {
    pub db: Connection,
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn fnv64(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

impl LineageStore {
    pub fn open(path: &str) -> Result<Self> {
        let mut db = Connection::open(path)?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        let mut store = LineageStore { db };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self> {
        let mut db = Connection::open_in_memory()?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        let mut store = LineageStore { db };
        store.migrate()?;
        Ok(store)
    }

    pub fn user_version(&self) -> Result<i64> {
        Ok(self.db.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    pub fn migrate(&mut self) -> Result<()> {
        let current = self.user_version()?;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let v = i as i64 + 1;
            if current < v {
                let tx = self.db.transaction()?;
                tx.execute_batch(sql)?;
                tx.execute_batch(&format!("PRAGMA user_version = {};", v))?;
                tx.commit()?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EventRow {
    pub id: String,
    pub case_id: String,
    pub branch_id: String,
    pub seq: i64,
    pub kind: String,
    pub summary: String,
    pub payload: Value,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub code: String,
    pub subject: String,
    pub severity: String,
    pub basis: Value,
}

#[derive(Debug, Clone)]
pub struct CommitPlan {
    pub case_id: String,
    pub branch_id: String,
    pub lineage_id: String,
    pub kind: String,
    pub source_ref: String,
    pub content: Value,
    pub bytes_sha: String,
    /// 乐观并发：基于调用方读到的版本
    pub base_version: Option<i64>,
    pub rename_only: bool,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitReceipt {
    pub event_id: String,
    pub material_id: String,
    pub version_no: i64,
    pub recomputed: Vec<String>,
    pub marked_stale: Vec<String>,
    pub conflict: Option<ConflictDiff>,
}

impl LineageStore {
    pub fn next_seq(&self, case_id: &str, branch_id: &str) -> Result<i64> {
        Ok(self.db.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE case_id=?1 AND branch_id=?2",
            params![case_id, branch_id],
            |r| r.get(0),
        )?)
    }

    pub fn create_case(&mut self, case_id: &str, title: &str, description: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO cases(case_id,title,description) VALUES(?1,?2,?3)",
            params![case_id, title, description],
        )?;
        Ok(())
    }

    pub fn create_branch(
        &mut self,
        case_id: &str,
        branch_id: &str,
        label: &str,
        parent_branch: Option<&str>,
    ) -> Result<String> {
        let exists: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM branches WHERE case_id=?1 AND branch_id=?2",
            params![case_id, branch_id],
            |r| r.get(0),
        )?;
        if exists > 0 {
            return Ok(self
                .db
                .query_row(
                    "SELECT id FROM branches WHERE case_id=?1 AND branch_id=?2",
                    params![case_id, branch_id],
                    |r| r.get::<_, String>(0),
                )
                .unwrap());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let seq = self.next_seq(case_id, branch_id)?;
        let eid = format!("evt-{}-{}-{}", case_id, branch_id, seq);
        let payload = serde_json::json!({
            "type": "branch_created", "branch_id": branch_id,
            "label": label, "parent_branch": parent_branch,
        });
        self.db.execute(
            "INSERT INTO events(id,case_id,branch_id,seq,kind,summary,payload_json,recorded_at)
             VALUES(?1,?2,?3,?4,'branch',?5,?6,?7)",
            params![
                id_key(&eid, seq),
                case_id,
                branch_id,
                seq,
                format!("创建分支 {}", label),
                payload.to_string(),
                now_ms()
            ],
        )?;
        self.db.execute(
            "INSERT INTO branches(id,case_id,branch_id,label,parent_branch,created_event_id)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![id, case_id, branch_id, label, parent_branch, eid],
        )?;
        // 从父分支复制 heads/marks（引用同一不可变结果/材料）
        if let Some(parent) = parent_branch {
            self.clone_heads_from(case_id, parent, branch_id)?;
        }
        self.write_snapshot(&eid, case_id, branch_id)?;
        Ok(eid)
    }

    fn clone_heads_from(
        &mut self,
        case_id: &str,
        src_branch: &str,
        dst_branch: &str,
    ) -> Result<()> {
        let event_id = self.db.query_row(
            "SELECT id FROM events WHERE case_id=?1 AND branch_id=?2 ORDER BY seq DESC LIMIT 1",
            params![case_id, dst_branch],
            |r| r.get::<_, String>(0),
        )?;
        let rows: Vec<(String, String, Option<String>, String, Option<String>)> = {
            let mut stmt = self.db.prepare(
                "SELECT node_id,lineage_id,result_id,state,stale_reason FROM marks
                 WHERE branch_id=?1",
            )?;
            let mapped = stmt
                .query_map(params![branch_pk(case_id, src_branch)], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            mapped
        };
        for (node, lineage, result, state, reason) in rows {
            self.db.execute(
                "INSERT INTO marks(branch_id,node_id,lineage_id,result_id,state,updated_event_id,stale_reason)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(branch_id,node_id) DO UPDATE SET
                   lineage_id=excluded.lineage_id, result_id=excluded.result_id,
                   state=excluded.state, updated_event_id=excluded.updated_event_id,
                   stale_reason=excluded.stale_reason",
                params![
                    branch_pk(case_id, dst_branch),
                    node,
                    lineage,
                    result,
                    state,
                    event_id,
                    reason
                ],
            )?;
        }
        Ok(())
    }
}

fn id_key(base: &str, _seq: i64) -> String {
    base.to_string()
}

fn branch_pk(case_id: &str, branch_id: &str) -> String {
    format!("{}::{}", case_id, branch_id)
}

impl LineageStore {
    pub fn current_material_version(
        &self,
        case_id: &str,
        lineage_id: &str,
    ) -> Result<Option<(String, i64, Value)>> {
        let mut stmt = self.db.prepare(
            "SELECT id,version_no,content_json FROM materials
             WHERE case_id=?1 AND lineage_id=?2 ORDER BY version_no DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![case_id, lineage_id])?;
        if let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let v: i64 = r.get(1)?;
            let content: Value = serde_json::from_str(&r.get::<_, String>(2)?)?;
            Ok(Some((id, v, content)))
        } else {
            Ok(None)
        }
    }

    /// 记录依赖（from 依赖 to）。
    pub fn add_dependency(
        &mut self,
        case_id: &str,
        from_lineage: &str,
        to_lineage: &str,
        rel: &str,
    ) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO dependencies(case_id,from_lineage,to_lineage,rel)
             VALUES(?1,?2,?3,?4)",
            params![case_id, from_lineage, to_lineage, rel],
        )?;
        Ok(())
    }

    fn dependents(&self, case_id: &str, lineage: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut stack = vec![lineage.to_string()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(cur) = stack.pop() {
            let mut stmt = self.db.prepare(
                "SELECT DISTINCT from_lineage FROM dependencies WHERE case_id=?1 AND to_lineage=?2",
            )?;
            let rows = stmt
                .query_map(params![case_id, cur], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for r in rows {
                if seen.insert(r.clone()) {
                    out.push(r.clone());
                    stack.push(r);
                }
            }
        }
        Ok(out)
    }

    /// 提交新材料版本。失败整体回滚。返回 409 语义时调用方映射为冲突响应。
    pub fn commit_material(&mut self, plan: CommitPlan) -> Result<CommitReceipt> {
        let tx = self.db.transaction()?;
        let current: Option<(String, i64, Value)> = {
            let mut stmt = tx.prepare(
                "SELECT id,version_no,content_json FROM materials
                 WHERE case_id=?1 AND lineage_id=?2 ORDER BY version_no DESC LIMIT 1",
            )?;
            let mut rows = stmt.query(params![plan.case_id, plan.lineage_id])?;
            if let Some(r) = rows.next()? {
                Some((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    serde_json::from_str(&r.get::<_, String>(2)?)?,
                ))
            } else {
                None
            }
        };
        let new_version = current.as_ref().map(|c| c.1 + 1).unwrap_or(1);
        if let Some(base) = plan.base_version {
            if let Some((_, actual, existing)) = &current {
                if *actual != base {
                    let diff = ConflictDiff {
                        node_ref: plan.lineage_id.clone(),
                        expected_version: Some(base),
                        actual_version: *actual,
                        expected_content: existing.clone(),
                        attempted_content: plan.content.clone(),
                    };
                    return Ok(CommitReceipt {
                        event_id: String::new(),
                        material_id: String::new(),
                        version_no: *actual,
                        recomputed: vec![],
                        marked_stale: vec![],
                        conflict: Some(diff),
                    });
                }
            } else if base != 0 {
                bail!("乐观版本基于 {} 但材料不存在", base);
            }
        }

        let seq = tx.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE case_id=?1 AND branch_id=?2",
            params![plan.case_id, plan.branch_id],
            |r| r.get(0),
        )?;
        let event_id = format!(
            "evt-{}-{}-{}",
            plan.case_id, plan.branch_id, seq
        );
        let material_id = format!(
            "mat-{}-{}-v{}",
            plan.case_id, plan.lineage_id, new_version
        );
        tx.execute(
            "INSERT INTO events(id,case_id,branch_id,seq,kind,summary,payload_json,recorded_at)
             VALUES(?1,?2,?3,?4,'material',?5,?6,?7)",
            params![
                event_id,
                plan.case_id,
                plan.branch_id,
                seq,
                plan.summary,
                serde_json::json!({
                    "type": "material_committed",
                    "lineage_id": plan.lineage_id,
                    "kind": plan.kind,
                    "version_no": new_version,
                    "source_ref": plan.source_ref,
                    "bytes_sha": plan.bytes_sha,
                    "rename_only": plan.rename_only,
                })
                .to_string(),
                now_ms()
            ],
        )?;
        tx.execute(
            "INSERT INTO materials(id,case_id,lineage_id,kind,version_no,source_ref,
                content_json,bytes_sha,rename_only,event_id,created_seq)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                material_id,
                plan.case_id,
                plan.lineage_id,
                plan.kind,
                new_version,
                plan.source_ref,
                plan.content.to_string(),
                plan.bytes_sha,
                plan.rename_only as i64,
                event_id,
                seq
            ],
        )?;
        tx.commit()?;
        let receipt = self.after_material_change(
            &plan.case_id,
            &plan.branch_id,
            &plan.lineage_id,
            &event_id,
            &material_id,
            new_version,
            &plan.kind,
            plan.rename_only,
        )?;
        Ok(receipt)
    }

    /// 失效传播与重算入口。重算只写入新结果，旧标记保留 stale 结论，绝不覆盖。
    fn after_material_change(
        &mut self,
        case_id: &str,
        branch_id: &str,
        lineage_id: &str,
        event_id: &str,
        material_id: &str,
        version_no: i64,
        kind: &str,
        rename_only: bool,
    ) -> Result<CommitReceipt> {
        let bp = branch_pk(case_id, branch_id);
        self.db.execute(
            "INSERT INTO marks(branch_id,node_id,lineage_id,result_id,state,updated_event_id,stale_reason)
             VALUES(?1,?2,?3,NULL,'source',?4,NULL)
             ON CONFLICT(branch_id,node_id) DO UPDATE SET state='source',
               updated_event_id=excluded.updated_event_id, stale_reason=NULL",
            params![bp, lineage_id, lineage_id, event_id],
        )?;

        let mut marked_stale = Vec::new();
        let mut recomputed = Vec::new();

        if !rename_only {
            let dependents = self.dependents(case_id, lineage_id)?;
            for dep in &dependents {
                self.db.execute(
                    "UPDATE marks SET state='stale', stale_reason=?1, updated_event_id=?2
                     WHERE branch_id=?3 AND node_id=?4 AND state!='stale' AND result_id IS NOT NULL",
                    params![format!("上游 {} 有新版本", lineage_id), event_id, bp, dep],
                )?;
                if self.db.changes() > 0 {
                    marked_stale.push(dep.clone());
                }
            }
            self.record_finding(
                event_id,
                "invalidation_scope",
                lineage_id,
                "info",
                serde_json::json!({
                    "changed_lineage": lineage_id,
                    "marked_stale": marked_stale,
                    "rule": "依赖闭包失效；旧结果保留且不覆盖",
                }),
            )?;
        } else {
            self.record_finding(
                event_id,
                "gate_renamed",
                lineage_id,
                "info",
                serde_json::json!({
                    "lineage_id": lineage_id,
                    "rule": "仅改名：lineage 身份与钉住的分母版本不变，不触发数值失效",
                }),
            )?;
        }

        // v1 导入或直接来源（事件/矩阵）改动：尝试重算其直接计算节点。
        if version_no == 1 || kind == "events" || kind == "matrix" {
            if let Some(node) = self.try_recompute(case_id, branch_id, kind, lineage_id, event_id)? {
                recomputed.push(node);
            }
        }

        self.write_snapshot(event_id, case_id, branch_id)?;
        Ok(CommitReceipt {
            event_id: event_id.to_string(),
            material_id: material_id.to_string(),
            version_no,
            recomputed,
            marked_stale,
            conflict: None,
        })
    }

}

fn id_key(base: &str, _seq: i64) -> String {
    base.to_string()
}

fn branch_pk(case_id: &str, branch_id: &str) -> String {
    format!("{}::{}", case_id, branch_id)
}

impl LineageStore {
    pub fn current_material_version(
        &self,
        case_id: &str,
        lineage_id: &str,
    ) -> Result<Option<(String, i64, Value)>> {
        let mut stmt = self.db.prepare(
            "SELECT id,version_no,content_json FROM materials
             WHERE case_id=?1 AND lineage_id=?2 ORDER BY version_no DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![case_id, lineage_id])?;
        if let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let v: i64 = r.get(1)?;
            let content: Value = serde_json::from_str(&r.get::<_, String>(2)?)?;
            Ok(Some((id, v, content)))
        } else {
            Ok(None)
        }
    }

    /// 记录依赖（from 依赖 to）。
    pub fn add_dependency(
        &mut self,
        case_id: &str,
        from_lineage: &str,
        to_lineage: &str,
        rel: &str,
    ) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO dependencies(case_id,from_lineage,to_lineage,rel)
             VALUES(?1,?2,?3,?4)",
            params![case_id, from_lineage, to_lineage, rel],
        )?;
        Ok(())
    }

    fn dependents(&self, case_id: &str, lineage: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut stack = vec![lineage.to_string()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(cur) = stack.pop() {
            let mut stmt = self.db.prepare(
                "SELECT DISTINCT from_lineage FROM dependencies WHERE case_id=?1 AND to_lineage=?2",
            )?;
            let rows = stmt
                .query_map(params![case_id, cur], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for r in rows {
                if seen.insert(r.clone()) {
                    out.push(r.clone());
                    stack.push(r);
                }
            }
        }
        Ok(out)
    }

    /// 提交新材料版本。失败整体回滚。返回 409 语义时调用方映射为冲突响应。
    pub fn commit_material(&mut self, plan: CommitPlan) -> Result<CommitReceipt> {
        let tx = self.db.transaction()?;
        let current: Option<(String, i64, Value)> = {
            let mut stmt = tx.prepare(
                "SELECT id,version_no,content_json FROM materials
                 WHERE case_id=?1 AND lineage_id=?2 ORDER BY version_no DESC LIMIT 1",
            )?;
            let mut rows = stmt.query(params![plan.case_id, plan.lineage_id])?;
            if let Some(r) = rows.next()? {
                Some((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    serde_json::from_str(&r.get::<_, String>(2)?)?,
                ))
            } else {
                None
            }
        };
        let new_version = current.as_ref().map(|c| c.1 + 1).unwrap_or(1);
        if let Some(base) = plan.base_version {
            if let Some((_, actual, existing)) = &current {
                if *actual != base {
                    let diff = ConflictDiff {
                        node_ref: plan.lineage_id.clone(),
                        expected_version: Some(base),
                        actual_version: *actual,
                        expected_content: existing.clone(),
                        attempted_content: plan.content.clone(),
                    };
                    return Ok(CommitReceipt {
                        event_id: String::new(),
                        material_id: String::new(),
                        version_no: *actual,
                        recomputed: vec![],
                        marked_stale: vec![],
                        conflict: Some(diff),
                    });
                }
            } else if base != 0 {
                bail!("乐观版本基于 {} 但材料不存在", base);
            }
        }

        let seq = tx.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE case_id=?1 AND branch_id=?2",
            params![plan.case_id, plan.branch_id],
            |r| r.get(0),
        )?;
        let event_id = format!(
            "evt-{}-{}-{}",
            plan.case_id, plan.branch_id, seq
        );
        let material_id = format!(
            "mat-{}-{}-v{}",
            plan.case_id, plan.lineage_id, new_version
        );
        tx.execute(
            "INSERT INTO events(id,case_id,branch_id,seq,kind,summary,payload_json,recorded_at)
             VALUES(?1,?2,?3,?4,'material',?5,?6,?7)",
            params![
                event_id,
                plan.case_id,
                plan.branch_id,
                seq,
                plan.summary,
                serde_json::json!({
                    "type": "material_committed",
                    "lineage_id": plan.lineage_id,
                    "kind": plan.kind,
                    "version_no": new_version,
                    "source_ref": plan.source_ref,
                    "bytes_sha": plan.bytes_sha,
                    "rename_only": plan.rename_only,
                })
                .to_string(),
                now_ms()
            ],
        )?;
        tx.execute(
            "INSERT INTO materials(id,case_id,lineage_id,kind,version_no,source_ref,
                content_json,bytes_sha,rename_only,event_id,created_seq)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                material_id,
                plan.case_id,
                plan.lineage_id,
                plan.kind,
                new_version,
                plan.source_ref,
                plan.content.to_string(),
                plan.bytes_sha,
                plan.rename_only as i64,
                event_id,
                seq
            ],
        )?;
        tx.commit()?;
        let receipt = self.after_material_change(
            &plan.case_id,
            &plan.branch_id,
            &plan.lineage_id,
            &event_id,
            &material_id,
            new_version,
            &plan.kind,
            plan.rename_only,
        )?;
        Ok(receipt)
    }

    /// 失效传播与重算入口。重算只写入新结果，旧标记保留 stale 结论，绝不覆盖。
    fn after_material_change(
        &mut self,
        case_id: &str,
        branch_id: &str,
        lineage_id: &str,
        event_id: &str,
        material_id: &str,
        version_no: i64,
        kind: &str,
        rename_only: bool,
    ) -> Result<CommitReceipt> {
        // 注册来源材料对应的图节点
        self.register_material_node(case_id, lineage_id, kind, event_id)?;

        let mut marked_stale = Vec::new();
        let mut recomputed = Vec::new();

        if !rename_only {
            // 下游标记 stale（含传递闭包），保留旧 result_id 与旧结论
            let dependents = self.dependents(case_id, lineage_id)?;
            for dep in &dependents {
                let node = dep;
                let existed: i64 = self.db.query_row(
                    "SELECT COUNT(*) FROM marks WHERE branch_id=?1 AND node_id=?2",
                    params![branch_pk(case_id, branch_id), node],
                    |r| r.get(0),
                )?;
                if existed > 0 {
                    self.db.execute(
                        "UPDATE marks SET state='stale', stale_reason=?1, updated_event_id=?2
                         WHERE branch_id=?3 AND node_id=?4 AND state!='stale'",
                        params![
                            format!("上游 {} 有新版本", lineage_id),
                            event_id,
                            branch_pk(case_id, branch_id),
                            node
                        ],
                    )?;
                    let changed = self.db.changes();
                    if changed > 0 {
                        marked_stale.push(node.clone());
                    }
                }
            }
            self.record_finding(
                event_id,
                "invalidation_scope",
                lineage_id,
                "info",
                serde_json::json!({
                    "changed_lineage": lineage_id,
                    "marked_stale": marked_stale,
                    "rule": "依赖闭包失效；旧结果保留且不覆盖",
                }),
            )?;
        } else {
            self.record_finding(
                event_id,
                "gate_renamed",
                lineage_id,
                "info",
                serde_json::json!({
                    "lineage_id": lineage_id,
                    "rule": "仅改名：lineage 身份与钉住的分母版本不变，不触发数值失效",
                }),
            )?;
        }

        // 首次导入（v1）：自底向上重算全部；后续版本仅直接节点可被显式 recompute。
        if version_no == 1 || kind == "events" || kind == "matrix" || kind == "spillover" {
            if let Some(node) = self.try_recompute(case_id, branch_id, kind, lineage_id, event_id)? {
                recomputed.push(node);
            }
        }

        self.write_snapshot(event_id, case_id, branch_id)?;
        Ok(CommitReceipt {
            event_id: event_id.to_string(),
            material_id: material_id.to_string(),
            version_no,
            recomputed,
            marked_stale,
            conflict: None,
        })
    }

}

fn id_key(base: &str, _seq: i64) -> String {
    base.to_string()
}

fn branch_pk(case_id: &str, branch_id: &str) -> String {
    format!("{}::{}", case_id, branch_id)
}

impl LineageStore {
    pub fn current_material_version(
        &self,
        case_id: &str,
        lineage_id: &str,
    ) -> Result<Option<(String, i64, Value)>> {
        let mut stmt = self.db.prepare(
            "SELECT id,version_no,content_json FROM materials
             WHERE case_id=?1 AND lineage_id=?2 ORDER BY version_no DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![case_id, lineage_id])?;
        if let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let v: i64 = r.get(1)?;
            let content: Value = serde_json::from_str(&r.get::<_, String>(2)?)?;
            Ok(Some((id, v, content)))
        } else {
            Ok(None)
        }
    }

    /// 记录依赖（from 依赖 to）。
    pub fn add_dependency(
        &mut self,
        case_id: &str,
        from_lineage: &str,
        to_lineage: &str,
        rel: &str,
    ) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO dependencies(case_id,from_lineage,to_lineage,rel)
             VALUES(?1,?2,?3,?4)",
            params![case_id, from_lineage, to_lineage, rel],
        )?;
        Ok(())
    }

    fn dependents(&self, case_id: &str, lineage: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut stack = vec![lineage.to_string()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(cur) = stack.pop() {
            let mut stmt = self.db.prepare(
                "SELECT DISTINCT from_lineage FROM dependencies WHERE case_id=?1 AND to_lineage=?2",
            )?;
            let rows = stmt
                .query_map(params![case_id, cur], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for r in rows {
                if seen.insert(r.clone()) {
                    out.push(r.clone());
                    stack.push(r);
                }
            }
        }
        Ok(out)
    }

    /// 提交新材料版本。失败整体回滚。返回 409 语义时调用方映射为冲突响应。
    pub fn commit_material(&mut self, plan: CommitPlan) -> Result<CommitReceipt> {
        let tx = self.db.transaction()?;
        let current: Option<(String, i64, Value)> = {
            let mut stmt = tx.prepare(
                "SELECT id,version_no,content_json FROM materials
                 WHERE case_id=?1 AND lineage_id=?2 ORDER BY version_no DESC LIMIT 1",
            )?;
            let mut rows = stmt.query(params![plan.case_id, plan.lineage_id])?;
            if let Some(r) = rows.next()? {
                Some((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    serde_json::from_str(&r.get::<_, String>(2)?)?,
                ))
            } else {
                None
            }
        };
        let new_version = current.as_ref().map(|c| c.1 + 1).unwrap_or(1);
        if let Some(base) = plan.base_version {
            if let Some((_, actual, existing)) = &current {
                if *actual != base {
                    let diff = ConflictDiff {
                        node_ref: plan.lineage_id.clone(),
                        expected_version: Some(base),
                        actual_version: *actual,
                        expected_content: existing.clone(),
                        attempted_content: plan.content.clone(),
                    };
                    return Ok(CommitReceipt {
                        event_id: String::new(),
                        material_id: String::new(),
                        version_no: *actual,
                        recomputed: vec![],
                        marked_stale: vec![],
                        conflict: Some(diff),
                    });
                }
            } else if base != 0 {
                bail!("乐观版本基于 {} 但材料不存在", base);
            }
        }

        let seq = tx.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE case_id=?1 AND branch_id=?2",
            params![plan.case_id, plan.branch_id],
            |r| r.get(0),
        )?;
        let event_id = format!(
            "evt-{}-{}-{}",
            plan.case_id, plan.branch_id, seq
        );
        let material_id = format!(
            "mat-{}-{}-v{}",
            plan.case_id, plan.lineage_id, new_version
        );
        tx.execute(
            "INSERT INTO events(id,case_id,branch_id,seq,kind,summary,payload_json,recorded_at)
             VALUES(?1,?2,?3,?4,'material',?5,?6,?7)",
            params![
                event_id,
                plan.case_id,
                plan.branch_id,
                seq,
                plan.summary,
                serde_json::json!({
                    "type": "material_committed",
                    "lineage_id": plan.lineage_id,
                    "kind": plan.kind,
                    "version_no": new_version,
                    "source_ref": plan.source_ref,
                    "bytes_sha": plan.bytes_sha,
                    "rename_only": plan.rename_only,
                })
                .to_string(),
                now_ms()
            ],
        )?;
        tx.execute(
            "INSERT INTO materials(id,case_id,lineage_id,kind,version_no,source_ref,
                content_json,bytes_sha,rename_only,event_id,created_seq)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                material_id,
                plan.case_id,
                plan.lineage_id,
                plan.kind,
                new_version,
                plan.source_ref,
                plan.content.to_string(),
                plan.bytes_sha,
                plan.rename_only as i64,
                event_id,
                seq
            ],
        )?;
        tx.commit()?;
        let receipt = self.after_material_change(
            &plan.case_id,
            &plan.branch_id,
            &plan.lineage_id,
            &event_id,
            &material_id,
            new_version,
            &plan.kind,
            plan.rename_only,
        )?;
        Ok(receipt)
    }

    /// 失效传播与重算入口。重算只写入新结果，旧标记保留 stale 结论，绝不覆盖。
    fn after_material_change(
        &mut self,
        case_id: &str,
        branch_id: &str,
        lineage_id: &str,
        event_id: &str,
        material_id: &str,
        version_no: i64,
        kind: &str,
        rename_only: bool,
    ) -> Result<CommitReceipt> {
        let bp = branch_pk(case_id, branch_id);
        self.db.execute(
            "INSERT INTO marks(branch_id,node_id,lineage_id,result_id,state,updated_event_id,stale_reason)
             VALUES(?1,?2,?3,NULL,'source',?4,NULL)
             ON CONFLICT(branch_id,node_id) DO UPDATE SET state='source',
               updated_event_id=excluded.updated_event_id, stale_reason=NULL",
            params![bp, lineage_id, lineage_id, event_id],
        )?;

        let mut marked_stale = Vec::new();
        let mut recomputed = Vec::new();

        if !rename_only {
            let dependents = self.dependents(case_id, lineage_id)?;
            for dep in &dependents {
                self.db.execute(
                    "UPDATE marks SET state='stale', stale_reason=?1, updated_event_id=?2
                     WHERE branch_id=?3 AND node_id=?4 AND state!='stale' AND result_id IS NOT NULL",
                    params![format!("上游 {} 有新版本", lineage_id), event_id, bp, dep],
                )?;
                if self.db.changes() > 0 {
                    marked_stale.push(dep.clone());
                }
            }
            self.record_finding(
                event_id,
                "invalidation_scope",
                lineage_id,
                "info",
                serde_json::json!({
                    "changed_lineage": lineage_id,
                    "marked_stale": marked_stale,
                    "rule": "依赖闭包失效；旧结果保留且不覆盖",
                }),
            )?;
        } else {
            self.record_finding(
                event_id,
                "gate_renamed",
                lineage_id,
                "info",
                serde_json::json!({
                    "lineage_id": lineage_id,
                    "rule": "仅改名：lineage 身份与钉住的分母版本不变，不触发数值失效",
                }),
            )?;
        }

        // v1 导入或直接来源（事件/矩阵）改动：尝试重算其直接计算节点。
        if version_no == 1 || kind == "events" || kind == "matrix" {
            if let Some(node) = self.try_recompute(case_id, branch_id, kind, lineage_id, event_id)? {
                recomputed.push(node);
            }
        }

        self.write_snapshot(event_id, case_id, branch_id)?;
        Ok(CommitReceipt {
            event_id: event_id.to_string(),
            material_id: material_id.to_string(),
            version_no,
            recomputed,
            marked_stale,
            conflict: None,
        })
    }

}

fn id_key(base: &str, _seq: i64) -> String {
    base.to_string()
}

fn branch_pk(case_id: &str, branch_id: &str) -> String {
    format!("{}::{}", case_id, branch_id)
}

impl LineageStore {
    pub fn current_material_version(
        &self,
        case_id: &str,
        lineage_id: &str,
    ) -> Result<Option<(String, i64, Value)>> {
        let mut stmt = self.db.prepare(
            "SELECT id,version_no,content_json FROM materials
             WHERE case_id=?1 AND lineage_id=?2 ORDER BY version_no DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![case_id, lineage_id])?;
        if let Some(r) = rows.next()? {
            let id: String = r.get(0)?;
            let v: i64 = r.get(1)?;
            let content: Value = serde_json::from_str(&r.get::<_, String>(2)?)?;
            Ok(Some((id, v, content)))
        } else {
            Ok(None)
        }
    }

    /// 记录依赖（from 依赖 to）。
    pub fn add_dependency(
        &mut self,
        case_id: &str,
        from_lineage: &str,
        to_lineage: &str,
        rel: &str,
    ) -> Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO dependencies(case_id,from_lineage,to_lineage,rel)
             VALUES(?1,?2,?3,?4)",
            params![case_id, from_lineage, to_lineage, rel],
        )?;
        Ok(())
    }

    fn dependents(&self, case_id: &str, lineage: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let mut stack = vec![lineage.to_string()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(cur) = stack.pop() {
            let mut stmt = self.db.prepare(
                "SELECT DISTINCT from_lineage FROM dependencies WHERE case_id=?1 AND to_lineage=?2",
            )?;
            let rows = stmt
                .query_map(params![case_id, cur], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for r in rows {
                if seen.insert(r.clone()) {
                    out.push(r.clone());
                    stack.push(r);
                }
            }
        }
        Ok(out)
    }

    /// 提交新材料版本。失败整体回滚。返回 409 语义时调用方映射为冲突响应。
    pub fn commit_material(&mut self, plan: CommitPlan) -> Result<CommitReceipt> {
        let tx = self.db.transaction()?;
        let current: Option<(String, i64, Value)> = {
            let mut stmt = tx.prepare(
                "SELECT id,version_no,content_json FROM materials
                 WHERE case_id=?1 AND lineage_id=?2 ORDER BY version_no DESC LIMIT 1",
            )?;
            let mut rows = stmt.query(params![plan.case_id, plan.lineage_id])?;
            if let Some(r) = rows.next()? {
                Some((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    serde_json::from_str(&r.get::<_, String>(2)?)?,
                ))
            } else {
                None
            }
        };
        let new_version = current.as_ref().map(|c| c.1 + 1).unwrap_or(1);
        if let Some(base) = plan.base_version {
            if let Some((_, actual, existing)) = &current {
                if *actual != base {
                    let diff = ConflictDiff {
                        node_ref: plan.lineage_id.clone(),
                        expected_version: Some(base),
                        actual_version: *actual,
                        expected_content: existing.clone(),
                        attempted_content: plan.content.clone(),
                    };
                    return Ok(CommitReceipt {
                        event_id: String::new(),
                        material_id: String::new(),
                        version_no: *actual,
                        recomputed: vec![],
                        marked_stale: vec![],
                        conflict: Some(diff),
                    });
                }
            } else if base != 0 {
                bail!("乐观版本基于 {} 但材料不存在", base);
            }
        }

        let seq = tx.query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE case_id=?1 AND branch_id=?2",
            params![plan.case_id, plan.branch_id],
            |r| r.get(0),
        )?;
        let event_id = format!(
            "evt-{}-{}-{}",
            plan.case_id, plan.branch_id, seq
        );
        let material_id = format!(
            "mat-{}-{}-v{}",
            plan.case_id, plan.lineage_id, new_version
        );
        tx.execute(
            "INSERT INTO events(id,case_id,branch_id,seq,kind,summary,payload_json,recorded_at)
             VALUES(?1,?2,?3,?4,'material',?5,?6,?7)",
            params![
                event_id,
                plan.case_id,
                plan.branch_id,
                seq,
                plan.summary,
                serde_json::json!({
                    "type": "material_committed",
                    "lineage_id": plan.lineage_id,
                    "kind": plan.kind,
                    "version_no": new_version,
                    "source_ref": plan.source_ref,
                    "bytes_sha": plan.bytes_sha,
                    "rename_only": plan.rename_only,
                })
                .to_string(),
                now_ms()
            ],
        )?;
        tx.execute(
            "INSERT INTO materials(id,case_id,lineage_id,kind,version_no,source_ref,
                content_json,bytes_sha,rename_only,event_id,created_seq)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                material_id,
                plan.case_id,
                plan.lineage_id,
                plan.kind,
                new_version,
                plan.source_ref,
                plan.content.to_string(),
                plan.bytes_sha,
                plan.rename_only as i64,
                event_id,
                seq
            ],
        )?;
        tx.commit()?;
        let receipt = self.after_material_change(
            &plan.case_id,
            &plan.branch_id,
            &plan.lineage_id,
            &event_id,
            &material_id,
            new_version,
            &plan.kind,
            plan.rename_only,
        )?;
        Ok(receipt)
    }

    /// 失效传播与重算入口。重算只写入新结果，旧标记保留 stale 结论，绝不覆盖。
    fn after_material_change(
        &mut self,
        case_id: &str,
        branch_id: &str,
        lineage_id: &str,
        event_id: &str,
        material_id: &str,
        version_no: i64,
        kind: &str,
        rename_only: bool,
    ) -> Result<CommitReceipt> {
        // 注册来源材料对应的图节点
        self.register_material_node(case_id, lineage_id, kind, event_id)?;

        let mut marked_stale = Vec::new();
        let mut recomputed = Vec::new();

        if !rename_only {
            // 下游标记 stale（含传递闭包），保留旧 result_id 与旧结论
            let dependents = self.dependents(case_id, lineage_id)?;
            for dep in &dependents {
                let node = dep;
                let existed: i64 = self.db.query_row(
                    "SELECT COUNT(*) FROM marks WHERE branch_id=?1 AND node_id=?2",
                    params![branch_pk(case_id, branch_id), node],
                    |r| r.get(0),
                )?;
                if existed > 0 {
                    self.db.execute(
                        "UPDATE marks SET state='stale', stale_reason=?1, updated_event_id=?2
                         WHERE branch_id=?3 AND node_id=?4 AND state!='stale'",
                        params![
                            format!("上游 {} 有新版本", lineage_id),
                            event_id,
                            branch_pk(case_id, branch_id),
                            node
                        ],
                    )?;
                    let changed = self.db.changes();
                    if changed > 0 {
                        marked_stale.push(node.clone());
                    }
                }
            }
            self.record_finding(
                event_id,
                "invalidation_scope",
                lineage_id,
                "info",
                serde_json::json!({
                    "changed_lineage": lineage_id,
                    "marked_stale": marked_stale,
                    "rule": "依赖闭包失效；旧结果保留且不覆盖",
                }),
            )?;
        } else {
            self.record_finding(
                event_id,
                "gate_renamed",
                lineage_id,
                "info",
                serde_json::json!({
                    "lineage_id": lineage_id,
                    "rule": "仅改名：lineage 身份与钉住的分母版本不变，不触发数值失效",
                }),
            )?;
        }

        // 首次导入（v1）：自底向上重算全部；后续版本仅直接节点可被显式 recompute。
        if version_no == 1 || kind == "events" || kind == "matrix" || kind == "spillover" {
            if let Some(node) = self.try_recompute(case_id, branch_id, kind, lineage_id, event_id)? {
                recomputed.push(node);
            }
        }

        self.write_snapshot(event_id, case_id, branch_id)?;
        Ok(CommitReceipt {
            event_id: event_id.to_string(),
            material_id: material_id.to_string(),
            version_no,
            recomputed,
            marked_stale,
            conflict: None,
        })
    }

    fn register_material_node(
        &mut self,
        case_id: &str,
        lineage_id: &str,
        kind: &str,
        event_id: &str,
    ) -> Result<()> {
        // 在 marks 中占位，来源材料本身状态为 source
        self.db.execute(
            "INSERT INTO marks(branch_id,node_id,lineage_id,result_id,state,updated_event_id,stale_reason)
             VALUES(?1,?2,?3,NULL,'source',?4,NULL)
             ON CONFLICT(branch_id,node_id) DO UPDATE SET state='source',
               updated_event_id=excluded.updated_event_id, stale_reason=NULL",
            params![branch_pk(case_id, lineage_id.split("::").next().unwrap_or(lineage_id)), lineage_id, lineage_id, event_id],
        )?;
        let _ = kind;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct HeadMaterial {
    material_id: String,
    version_no: i64,
    content: Value,
    kind: String,
    bytes_sha: String,
}

impl LineageStore {
    fn head_material(
        &self,
        case_id: &str,
        lineage_id: &str,
    ) -> Result<Option<HeadMaterial>> {
        let mut stmt = self.db.prepare(
            "SELECT m.id,m.version_no,m.content_json,m.kind,m.bytes_sha
             FROM materials m
             JOIN (SELECT lineage_id, MAX(version_no) mx FROM materials
                   WHERE case_id=?1 GROUP BY lineage_id) t
               ON t.lineage_id=m.lineage_id AND t.mx=m.version_no
             WHERE m.case_id=?1 AND m.lineage_id=?2",
        )?;
        let mut rows = stmt.query(params![case_id, lineage_id])?;
        if let Some(r) = rows.next()? {
            Ok(Some(HeadMaterial {
                material_id: r.get(0)?,
                version_no: r.get(1)?,
                content: serde_json::from_str(&r.get::<_, String>(2)?)?,
                kind: r.get(3)?,
                bytes_sha: r.get(4)?,
            }))
        } else {
            Ok(None)
        }
    }

    fn list_heads(&self, case_id: &str) -> Result<Vec<HeadMaterial>> {
        let mut stmt = self.db.prepare(
            "SELECT m.id,m.version_no,m.content_json,m.kind,m.bytes_sha,m.lineage_id
             FROM materials m
             JOIN (SELECT lineage_id, MAX(version_no) mx FROM materials
                   WHERE case_id=?1 GROUP BY lineage_id) t
               ON t.lineage_id=m.lineage_id AND t.mx=m.version_no
             WHERE m.case_id=?1
             ORDER BY m.lineage_id",
        )?;
        let rows = stmt
            .query_map(params![case_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(id, v, cj, kind, sha)| HeadMaterial {
                material_id: id,
                version_no: v,
                content: serde_json::from_str(&cj).unwrap_or(Value::Null),
                kind,
                bytes_sha: sha,
            })
            .collect())
    }

    fn record_finding(
        &mut self,
        event_id: &str,
        code: &str,
        subject: &str,
        severity: &str,
        basis: Value,
    ) -> Result<()> {
        self.db.execute(
            "INSERT INTO findings(event_id,code,subject,severity,basis_json)
             VALUES(?1,?2,?3,?4,?5)",
            params![event_id, code, subject, severity, basis.to_string()],
        )?;
        Ok(())
    }

    fn write_snapshot(
        &mut self,
        event_id: &str,
        case_id: &str,
        branch_id: &str,
    ) -> Result<()> {
        let bp = branch_pk(case_id, branch_id);
        let heads: BTreeMap<String, i64> = {
            let mut stmt = self.db.prepare(
                "SELECT lineage_id, MAX(version_no) FROM materials
                 WHERE case_id=?1 GROUP BY lineage_id",
            )?;
            let rows = stmt
                .query_map(params![case_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows.into_iter().collect()
        };
        #[derive(Serialize)]
        struct MarkSnap {
            node_id: String,
            lineage_id: String,
            result_id: Option<String>,
            state: String,
            stale_reason: Option<String>,
        }
        let marks: Vec<MarkSnap> = {
            let mut stmt = self.db.prepare(
                "SELECT node_id,lineage_id,result_id,state,stale_reason FROM marks WHERE branch_id=?1",
            )?;
            stmt.query_map(params![bp], |r| {
                Ok(MarkSnap {
                    node_id: r.get(0)?,
                    lineage_id: r.get(1)?,
                    result_id: r.get(2)?,
                    state: r.get(3)?,
                    stale_reason: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let decisions: Vec<String> = {
            let mut stmt = self
                .db
                .prepare("SELECT id FROM decisions WHERE branch_id=?1 AND undone_event_id IS NULL");
            stmt.query_map(params![bp], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        self.db.execute(
            "INSERT INTO snapshots(event_id,heads_json,marks_json,decisions_json)
             VALUES(?1,?2,?3,?4)
             ON CONFLICT(event_id) DO UPDATE SET heads_json=excluded.heads_json,
               marks_json=excluded.marks_json, decisions_json=excluded.decisions_json",
            params![
                event_id,
                serde_json::to_string(&heads)?,
                serde_json::to_string(&marks)?,
                serde_json::to_string(&decisions)?
            ],
        )?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
struct NodeResult {
    node_id: String,
    lineage_id: String,
    kind: String, // compensation | gate
    status: String,
    input_hash: String,
    payload: Value,
    candidates: Vec<(String, String, Value)>, // id,label,payload
}

impl LineageStore {
    /// 材料变更后允许自动重算的节点（v1 导入或直接来源改动）。
    fn try_recompute(
        &mut self,
        case_id: &str,
        branch_id: &str,
        changed_kind: &str,
        changed_lineage: &str,
        event_id: &str,
    ) -> Result<Option<String>> {
        match changed_kind {
            "events" | "matrix" | "spillover" => {
                self.recompute_comp_all(case_id, branch_id, event_id)?;
                Ok(Some(format!("comp::all@{}", changed_lineage)))
            }
            "gate" => {
                // 仅 v1；v2+ 已经是下游修订，交给显式 recompute
                let node = self.recompute_gate(case_id, branch_id, changed_lineage, event_id)?;
                Ok(node.map(|_| changed_lineage.to_string()))
            }
            _ => Ok(None),
        }
    }

    fn events_lineage(&self, case_id: &str) -> Result<Option<String>> {
        let mut stmt = self.db.prepare(
            "SELECT lineage_id FROM materials WHERE case_id=?1 AND kind='events'
             ORDER BY version_no LIMIT 1",
        )?;
        let mut rows = stmt.query(params![case_id])?;
        if let Some(r) = rows.next()? {
            Ok(Some(r.get(0)?))
        } else {
            Ok(None)
        }
    }

    fn matrix_lineages(&self, case_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .db
            .prepare("SELECT DISTINCT lineage_id FROM materials WHERE case_id=?1 AND kind='matrix'");
        let rows = stmt
            .query_map(params![case_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn recompute_comp_all(
        &mut self,
        case_id: &str,
        branch_id: &str,
        event_id: &str,
    ) -> Result<()> {
        let matrices = self.matrix_lineages(case_id)?;
        for lin in matrices {
            self.eval_comp(case_id, branch_id, &lin, event_id)?;
        }
        Ok(())
    }

    fn load_events_values(
        &self,
        content: &Value,
    ) -> Vec<(String, Vec<Option<f64>>, Vec<String>)> {
        let arr = content["events"].as_array().cloned().unwrap_or_default();
        arr.into_iter()
            .map(|e| {
                let id = e["id"].as_str().unwrap_or("?").to_string();
                let vals: Vec<Option<f64>> = e["values"]
                    .as_array()
                    .map(|a| a.iter().map(|v| v.as_f64()).collect())
                    .unwrap_or_default();
                let raw: Vec<String> = e["raw_repr"]
                    .as_array()
                    .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
                    .unwrap_or_default();
                (id, vals, raw)
            })
            .collect()
    }

    fn eval_comp(
        &mut self,
        case_id: &str,
        branch_id: &str,
        matrix_lineage: &str,
        event_id: &str,
    ) -> Result<Option<String>> {
        let events_lin = match self.events_lineage(case_id)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let ev = match self.head_material(case_id, &events_lin)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let mx = match self.head_material(case_id, matrix_lineage)? {
            Some(v) => v,
            None => return Ok(None),
        };
        let md = &mx.content["matrix"];
        let matrix = domain::Matrix {
            rows: serde_json::from_value(md["rows"].clone()).unwrap_or_default(),
            detectors: md["detectors"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
                .unwrap_or_default(),
            fluorochromes: md["fluorochromes"]
                .as_array()
                .map(|a| a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect())
                .unwrap_or_default(),
        };
        self.add_dependency(case_id, matrix_lineage, &events_lin, "events")?;

        let raw_events = self.load_events_values(&ev.content);
        let n = matrix.rows.len();
        let mut numeric: Vec<Vec<f64>> = Vec::new();
        let mut numeric_idx: Vec<usize> = Vec::new();
        for (i, (_, vals, _)) in raw_events.iter().enumerate() {
            if vals.len() >= n && vals.iter().take(n).all(|v| v.is_some()) {
                numeric.push(vals.iter().take(n).map(|v| v.unwrap()).collect());
                numeric_idx.push(i);
            }
        }
        let outcome = domain::compensate(&matrix, &numeric);
        let mut diagnostics = outcome.diagnostics.clone();

        let mut out_events = Vec::new();
        for (i, (id, vals, raw)) in raw_events.iter().enumerate() {
            let unconverted: Vec<String> = vals
                .iter()
                .enumerate()
                .filter_map(|(c, v)| {
                    if v.is_none() {
                        matrix.detectors.get(c).cloned().or(Some(format!("ch{}", c)))
                    } else {
                        None
                    }
                })
                .collect();
            let mut extreme = false;
            let pos = numeric_idx.iter().position(|x| *x == i);
            let comp: Vec<Option<f64>> = if let Some(p) = pos {
                outcome.values.as_ref().map(|v| v[p].iter().map(|f| Some(*f)).collect()).unwrap_or_else(|| vec![None; n])
            } else {
                if !unconverted.is_empty() {
                    diagnostics.push(format!(
                        "事件 {} 含不可转换通道 {:?}，不参与求逆/补偿，原始值保留",
                        id, unconverted
                    ));
                }
                vec![None; n]
            };
            for v in vals.iter().take(n).flatten() {
                if v.abs() > 1e5 {
                    extreme = true;
                }
            }
            if extreme {
                diagnostics.push(format!("事件 {} 含极端原始信号，已保留并标记 extreme", id));
            }
            out_events.push(serde_json::json!({
                "id": id, "raw": vals, "raw_repr": raw,
                "comp": comp, "unconverted": unconverted, "extreme": extreme,
            }));
        }

        let payload = serde_json::json!({
            "type": "compensation",
            "matrix_lineage": matrix_lineage,
            "matrix_mvid": mx.material_id,
            "events_mvid": ev.material_id,
            "detectors": matrix.detectors,
            "fluorochromes": matrix.fluorochromes,
            "matrix_unit": md["unit"],
            "condition": outcome.condition,
            "determinant": outcome.determinant,
            "status": outcome.status,
            "events": out_events,
            "diagnostics": diagnostics,
        });
        let inputs = serde_json::json!({"matrix": mx.material_id, "events": ev.material_id});
        let input_hash = fnv64(&inputs.to_string());
        let mut candidates = Vec::new();
        for c in &outcome.candidates {
            let mut cand_events = out_events.clone();
            for (eidx, evrow) in cand_events.iter_mut().enumerate() {
                if let Some(p) = numeric_idx.iter().position(|x| *x == eidx) {
                    evrow["comp"] = serde_json::to_value(
                        c.values[p].iter().map(|f| Some(*f)).collect::<Vec<_>>(),
                    )
                    .unwrap();
                }
            }
            candidates.push((
                c.id.clone(),
                c.label.clone(),
                serde_json::json!({
                    "type": "compensation_candidate",
                    "ridge": c.ridge,
                    "residual": c.residual,
                    "events": cand_events,
                }),
            ));
        }
        let nr = NodeResult {
            node_id: matrix_lineage.to_string(),
            lineage_id: matrix_lineage.to_string(),
            kind: "compensation".into(),
            status: outcome.status.clone(),
            input_hash,
            payload,
            candidates,
        };
        let rid = self.persist_result(case_id, &nr, event_id)?;
        self.set_mark(case_id, branch_id, matrix_lineage, matrix_lineage, Some(&rid),
            match outcome.status.as_str() {
                "ok" => "valid",
                "near_singular" => "ambiguous",
                _ => "blocked",
            }, event_id, None)?;
        Ok(Some(rid))
    }
}

impl LineageStore {
    fn persist_result(
        &mut self,
        case_id: &str,
        nr: &NodeResult,
        event_id: &str,
    ) -> Result<String> {
        let rid = format!(
            "res-{}::{}::{}",
            nr.kind, nr.node_id, nr.input_hash
        );
        let exists: i64 = self.db.query_row(
            "SELECT COUNT(*) FROM results WHERE id=?1",
            params![rid],
            |r| r.get(0),
        )?;
        if exists == 0 {
            self.db.execute(
                "INSERT INTO results(id,node_id,lineage_id,kind,status,input_hash,payload_json,created_event_id)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    rid, nr.node_id, nr.lineage_id, nr.kind, nr.status,
                    nr.input_hash, nr.payload.to_string(), event_id
                ],
            )?;
            for (cid, label, cp) in &nr.candidates {
                self.db.execute(
                    "INSERT OR REPLACE INTO result_candidates(result_id,candidate_id,label,payload_json)
                     VALUES(?1,?2,?3,?4)",
                    params![rid, cid, label, cp.to_string()],
                )?;
            }
        }
        let _ = case_id;
        Ok(rid)
    }

    fn set_mark(
        &mut self,
        case_id: &str,
        branch_id: &str,
        node_id: &str,
        lineage_id: &str,
        result_id: Option<&str>,
        state: &str,
        event_id: &str,
        stale_reason: Option<&str>,
    ) -> Result<()> {
        // mark 不覆盖已存在的 stale 结论：新结果写新 mark 行（同节点新输入是新结果 id）。
        self.db.execute(
            "INSERT INTO marks(branch_id,node_id,lineage_id,result_id,state,updated_event_id,stale_reason)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(branch_id,node_id) DO UPDATE SET
               lineage_id=excluded.lineage_id,
               result_id=COALESCE(excluded.result_id, marks.result_id),
               state=CASE WHEN marks.state='stale' AND marks.result_id IS NOT NULL
                          AND excluded.result_id IS NOT NULL
                          AND excluded.result_id = marks.result_id
                     THEN 'stale' ELSE excluded.state END,
               updated_event_id=excluded.updated_event_id,
               stale_reason=COALESCE(excluded.stale_reason, marks.stale_reason)",
            params![
                branch_pk(case_id, branch_id),
                node_id,
                lineage_id,
                result_id,
                state,
                event_id,
                stale_reason
            ],
        )?;
        Ok(())
    }
}

#[derive(Clone)]
struct GateCtx {
    lineage_id: String,
    name: String,
    basis: String,
    x_channel: String,
    y_channel: String,
    x_cofactor: f64,
    y_cofactor: f64,
    x_trans_id: String,
    x_trans_mvid: String,
    y_trans_id: String,
    y_trans_mvid: String,
    vertices: Vec<[f64; 2]>,
    boundary: Option<domain::BoundaryRule>,
    gate_mvid: String,
    gate_version: i64,
}

impl LineageStore {
    fn recompute_gate(
        &mut self,
        case_id: &str,
        branch_id: &str,
        gate_lineage: &str,
        event_id: &str,
    ) -> Result<Option<String>> {
        self.eval_gate(case_id, branch_id, gate_lineage, event_id)
    }

    pub fn recompute_node(
        &mut self,
        case_id: &str,
        branch_id: &str,
        node_id: &str,
    ) -> Result<String> {
        let seq = self.next_seq(case_id, branch_id)?;
        let eid = format!("evt-{}-{}-{}", case_id, branch_id, seq);
        let bp = branch_pk(case_id, branch_id);
        // 判定节点类型
        let kind: String = self.db.query_row(
            "SELECT kind FROM materials WHERE case_id=?1 AND lineage_id=?2 LIMIT 1",
            params![case_id, node_id],
            |r| r.get(0),
        ).unwrap_or_else(|_| {
            // 可能是补偿节点（lineage=matrix lineage）
            "matrix".to_string()
        });
        self.db.execute(
            "INSERT INTO events(id,case_id,branch_id,seq,kind,summary,payload_json,recorded_at)
             VALUES(?1,?2,?3,?4,'recompute',?5,?6,?7)",
            params![eid, case_id, branch_id, seq,
                format!("显式重算节点 {}", node_id),
                serde_json::json!({"type":"recompute_requested","node_id":node_id}).to_string(),
                now_ms()],
        )?;
        let rid = if kind == "matrix" {
            self.eval_comp(case_id, branch_id, node_id, &eid)?
        } else {
            self.eval_gate(case_id, branch_id, node_id, &eid)?
        };
        let _ = bp;
        self.write_snapshot(&eid, case_id, branch_id)?;
        Ok(rid.unwrap_or_default())
    }

    fn parent_gate_lineage(
        &self,
        case_id: &str,
        gate_lineage: &str,
    ) -> Result<Option<String>> {
        let mut stmt = self.db.prepare(
            "SELECT content_json FROM materials WHERE case_id=?1 AND kind='lineage'",
        )?;
        let rows = stmt
            .query_map(params![case_id], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for cj in rows {
            let v: Value = serde_json::from_str(&cj)?;
            if let Some(edges) = v["edges"].as_array() {
                for e in edges {
                    if e["child_gate"].as_str() == Some(gate_lineage) {
                        if let Some(p) = e["parent_gate"].as_str() {
                            return Ok(Some(p.to_string()));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    fn load_gate_ctx(&self, case_id: &str, gate_lineage: &str) -> Result<Option<GateCtx>> {
        let gm = match self.head_material(case_id, gate_lineage)? {
            Some(v) if v.kind == "gate" => v,
            _ => return Ok(None),
        };
        let g = &gm.content;
        let cofactor_of = |axis: &Value| -> (f64, String, String) {
            let tid = axis["transform_id"].as_str().unwrap_or("").to_string();
            let tmvid = axis["transform_mvid"].as_str().unwrap_or("").to_string();
            if let Some(tm) = self.head_material(case_id, &tid).ok().flatten() {
                (tm.content["cofactor"].as_f64().unwrap_or(150.0), tid, tm.material_id)
            } else {
                (150.0, tid, tmvid)
            }
        };
        let (xc, xtid, xtmvid) = cofactor_of(&g["x_axis"]);
        let (yc, ytid, ytmvid) = cofactor_of(&g["y_axis"]);
        let boundary = match g["boundary"].as_str() {
            Some("edge-inclusive") => Some(domain::BoundaryRule::EdgeInclusive),
            Some("edge-exclusive") => Some(domain::BoundaryRule::EdgeExclusive),
            _ => None,
        };
        Ok(Some(GateCtx {
            lineage_id: gate_lineage.to_string(),
            name: g["name"].as_str().unwrap_or(gate_lineage).to_string(),
            basis: g["basis"].as_str().unwrap_or("").to_string(),
            x_channel: g["x_axis"]["channel"].as_str().unwrap_or("").to_string(),
            y_channel: g["y_axis"]["channel"].as_str().unwrap_or("").to_string(),
            x_cofactor: xc,
            y_cofactor: yc,
            x_trans_id: xtid,
            x_trans_mvid: xtmvid,
            y_trans_id: ytid,
            y_trans_mvid: ytmvid,
            vertices: serde_json::from_value(g["vertices"].clone()).unwrap_or_default(),
            boundary,
            gate_mvid: gm.material_id,
            gate_version: gm.version_no,
        }))
    }

    fn results_for_node(&self, node_id: &str) -> Result<Vec<(String, String, Value)>> {
        let mut stmt = self
            .db
            .prepare("SELECT id,status,payload_json FROM results WHERE node_id=?1 ORDER BY created_event_id");
        let rows = stmt
            .query_map(params![node_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .map(|(id, st, pj)| (id, st, serde_json::from_str(&pj).unwrap_or(Value::Null)))
            .collect())
    }

    fn mark_for(&self, case_id: &str, branch_id: &str, node_id: &str) -> Result<Option<(Option<String>, String, Option<String>)>> {
        let mut stmt = self.db.prepare(
            "SELECT result_id,state,stale_reason FROM marks WHERE branch_id=?1 AND node_id=?2",
        )?;
        let mut rows = stmt.query(params![branch_pk(case_id, branch_id), node_id])?;
        if let Some(r) = rows.next()? {
            Ok(Some((r.get(0)?, r.get(1)?, r.get(2)?)))
        } else {
            Ok(None)
        }
    }
}

impl LineageStore {
    fn eval_gate(
        &mut self,
        case_id: &str,
        branch_id: &str,
        gate_lineage: &str,
        event_id: &str,
    ) -> Result<Option<String>> {
        let ctx = match self.load_gate_ctx(case_id, gate_lineage)? {
            Some(c) => c,
            None => return Ok(None),
        };
        // 依赖：转换参数、父门
        self.add_dependency(case_id, gate_lineage, &ctx.x_trans_id, "transform")?;
        if ctx.y_trans_id != ctx.x_trans_id {
            self.add_dependency(case_id, gate_lineage, &ctx.y_trans_id, "transform")?;
        }
        let parent_lineage = self.parent_gate_lineage(case_id, gate_lineage)?;
        if let Some(p) = &parent_lineage {
            self.add_dependency(case_id, gate_lineage, p, "parent_gate")?;
        }
        // 选择补偿矩阵：取门轴通道检测器对应的矩阵（案例中唯一）
        let matrix_lineages = self.matrix_lineages(case_id)?;
        let mut chosen_matrix: Option<String> = None;
        for ml in &matrix_lineages {
            if let Some(mx) = self.head_material(case_id, ml)? {
                let dets = mx.content["matrix"]["detectors"]
                    .as_array()
                    .map(|a| {
                        a.iter().map(|v| v.as_str().unwrap_or("").to_string()).collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if dets.iter().any(|d| d == &ctx.x_channel || d == &ctx.y_channel) {
                    chosen_matrix = Some(ml.clone());
                    break;
                }
            }
        }
        if let Some(ml) = &chosen_matrix {
            self.add_dependency(case_id, gate_lineage, ml, "compensation")?;
        }

        // 父群集合与钉住版本
        let mut parent_options: Vec<(String, Value)> = Vec::new(); // (result_id, payload)
        let mut parent_state = "valid".to_string();
        if let Some(pl) = &parent_lineage {
            if let Some((prid, pstate, _reason)) = self.mark_for(case_id, branch_id, pl)? {
                parent_state = pstate.clone();
                if let Some(prid) = prid {
                    let pj: Value = self.db.query_row(
                        "SELECT payload_json FROM results WHERE id=?1",
                        params![prid],
                        |r| serde_json::from_str::<String>(r.get::<_, String>(0)?)
                            .map(|s| serde_json::from_str(&s).unwrap_or(Value::Null))
                            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                                0, rusqlite::types::Type::Text, Box::new(e))),
                    )?;
                    parent_options.push((prid, pj));
                }
            }
            if parent_options.is_empty() {
                // 父门尚无结果：尝试即时评估父门
                self.eval_gate(case_id, branch_id, pl, event_id)?;
                if let Some((prid, pstate, _)) = self.mark_for(case_id, branch_id, pl)? {
                    parent_state = pstate;
                    if let Some(prid) = prid {
                        let pj: String =
                            self.db.query_row("SELECT payload_json FROM results WHERE id=?1",
                                params![prid], |r| r.get(0))?;
                        parent_options.push((prid, serde_json::from_str(&pj)?));
                    }
                }
            }
        }
        RUSTEOF
echo ok