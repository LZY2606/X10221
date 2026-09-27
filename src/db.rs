//! SQLite 存储：append-only 事件 + 分开版本化的来源 + 派生结果。
//! 迁移分两版，迁移测试从 v1 现场升级到 v2 验证数据保留。

use crate::domain::{
    ArtifactVersionRow, BranchRow, CandidateRow, EventRow, EventType, Json, ResultRow,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::sync::Mutex;

pub struct Db(pub Mutex<Connection>);

const MIGRATION_V1: &str = include_str!("../migrations/0001_init.sql");
const MIGRATION_V2: &str = include_str!("../migrations/0002_instance.sql");

impl Db {
    pub fn open(path: &str) -> rusqlite::Result<Db> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Db(Mutex::new(conn));
        db.migrate()?;
        Ok(db)
    }

    /// 仅建出 v1 schema（测试专用），用于状态迁移测试。
    pub fn open_at_v1(path: &str) -> rusqlite::Result<Db> {
        let conn = Connection::open(path)?;
        conn.execute_batch(MIGRATION_V1)?;
        Ok(Db(Mutex::new(conn)))
    }

    pub fn user_version(&self) -> rusqlite::Result<i64> {
        self.0.lock().unwrap().query_row(
            "PRAGMA user_version",
            [],
            |r| r.get::<_, i64>(0),
        )
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let mut c = self.0.lock().unwrap();
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if v < 1 {
            c.execute_batch(MIGRATION_V1)?;
        }
        if v < 2 {
            c.execute_batch(MIGRATION_V2)?;
            // 迁移数据：为既有实例登记默认槽位
            c.execute("INSERT OR IGNORE INTO instances(name) VALUES ('default')", [])?;
            c.execute_batch("PRAGMA user_version = 2;")?;
        }
        Ok(())
    }

    pub fn tx<T>(&self, f: impl FnOnce(&Transaction) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    // ---- 写入 ----

    pub fn insert_branch(tx: &Transaction, b: &BranchRow) -> rusqlite::Result<()> {
        tx.execute(
            "INSERT INTO branches(name,label,parent_branch,forked_from_event,head_event,
                base_version,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                b.name,
                b.label,
                b.parent_branch,
                b.forked_from_event,
                b.head_event,
                b.base_version,
                b.created_at
            ],
        )?;
        Ok(())
    }

    pub fn insert_event(tx: &Transaction, e: &EventRow) -> rusqlite::Result<()> {
        tx.execute(
            "INSERT INTO events(id,seq,branch,type,title,detail,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                e.id, e.seq, e.branch, e.kind, e.title, e.detail, e.created_at
            ],
        )?;
        tx.execute(
            "UPDATE branches SET head_event=?1 WHERE name=?2",
            params![e.id, e.branch],
        )?;
        Ok(())
    }

    pub fn insert_artifact(tx: &Transaction, a: &ArtifactVersionRow) -> rusqlite::Result<()> {
        tx.execute(
            "INSERT INTO artifact_versions(art_id,kind,version,label,payload,status,diagnostic,
                created_event,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                a.art_id,
                a.kind,
                a.version,
                a.label,
                a.payload,
                a.status,
                a.diagnostic,
                a.created_event,
                a.created_at
            ],
        )?;
        Ok(())
    }

    pub fn insert_result(tx: &Transaction, r: &ResultRow) -> rusqlite::Result<()> {
        tx.execute(
            "INSERT INTO results(id,branch,scope,label,payload,deps,parent_result,
                parent_result_version,status,produced_event,superseded_by,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                r.id,
                r.branch,
                r.scope,
                r.label,
                r.payload,
                r.deps,
                r.parent_result,
                r.parent_result_version,
                r.status,
                r.produced_event,
                r.superseded_by,
                r.created_at
            ],
        )?;
        Ok(())
    }

    pub fn insert_candidate(tx: &Transaction, c: &CandidateRow) -> rusqlite::Result<()> {
        tx.execute(
            "INSERT INTO candidates(id,branch,scope,label,payload,rationale,status,
                produced_event,adjudicated_event,created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                c.id,
                c.branch,
                c.scope,
                c.label,
                c.payload,
                c.rationale,
                c.status,
                c.produced_event,
                c.adjudicated_event,
                c.created_at
            ],
        )?;
    }

    // ---- 读取 ----

    pub fn branch(&self, name: &str) -> rusqlite::Result<Option<BranchRow>> {
        self.0.lock().unwrap().query_row(
            "SELECT name,label,parent_branch,forked_from_event,head_event,base_version,created_at
             FROM branches WHERE name=?1",
            params![name],
            |r| {
                Ok(BranchRow {
                    name: r.get(0)?,
                    label: r.get(1)?,
                    parent_branch: r.get(2)?,
                    forked_from_event: r.get(3)?,
                    head_event: r.get(4)?,
                    base_version: r.get(5)?,
                    created_at: r.get(6)?,
                })
            },
        ).optional()
    }

    pub fn all_branches(&self) -> rusqlite::Result<Vec<BranchRow>> {
        let c = self.0.lock().unwrap();
        let mut s = c.prepare(
            "SELECT name,label,parent_branch,forked_from_event,head_event,base_version,created_at
             FROM branches ORDER BY created_at, name",
        )?;
        let rows = s.query_map([], |r| {
            Ok(BranchRow {
                name: r.get(0)?,
                label: r.get(1)?,
                parent_branch: r.get(2)?,
                forked_from_event: r.get(3)?,
                head_event: r.get(4)?,
                base_version: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn events(&self, branch: &str) -> rusqlite::Result<Vec<EventRow>> {
        let c = self.0.lock().unwrap();
        let mut s = c.prepare(
            "SELECT id,seq,branch,type,title,detail,created_at FROM events
             WHERE branch=?1 ORDER BY seq",
        )?;
        let rows = s.query_map(params![branch], |r| {
            Ok(EventRow {
                id: r.get(0)?,
                seq: r.get(1)?,
                branch: r.get(2)?,
                kind: r.get(3)?,
                title: r.get(4)?,
                detail: r.get(5)?,
                created_at: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn event(&self, id: &str) -> rusqlite::Result<Option<EventRow>> {
        self.0.lock().unwrap().query_row(
            "SELECT id,seq,branch,type,title,detail,created_at FROM events WHERE id=?1",
            params![id],
            |r| {
                Ok(EventRow {
                    id: r.get(0)?,
                    seq: r.get(1)?,
                    branch: r.get(2)?,
                    kind: r.get(3)?,
                    title: r.get(4)?,
                    detail: r.get(5)?,
                    created_at: r.get(6)?,
                })
            },
        ).optional()
    }

    pub fn next_seq(&self, branch: &str) -> rusqlite::Result<i64> {
        self.0.lock().unwrap().query_row(
            "SELECT COALESCE(MAX(seq),0)+1 FROM events WHERE branch=?1",
            params![branch],
            |r| r.get(0),
        )
    }

    pub fn artifact(&self, art_id: &str, version: i64) -> rusqlite::Result<Option<ArtifactVersionRow>> {
        self.0.lock().unwrap().query_row(
            "SELECT art_id,kind,version,label,payload,status,diagnostic,created_event,created_at
             FROM artifact_versions WHERE art_id=?1 AND version=?2",
            params![art_id, version],
            |r| {
                Ok(ArtifactVersionRow {
                    art_id: r.get(0)?,
                    kind: r.get(1)?,
                    version: r.get(2)?,
                    label: r.get(3)?,
                    payload: r.get(4)?,
                    status: r.get(5)?,
                    diagnostic: r.get(6)?,
                    created_event: r.get(7)?,
                    created_at: r.get(8)?,
                })
            },
        ).optional()
    }

    pub fn artifacts_for_branch(&self, branch: &str) -> rusqlite::Result<Vec<ArtifactVersionRow>> {
        let c = self.0.lock().unwrap();
        let mut s = c.prepare(
            "SELECT DISTINCT a.art_id,a.kind,a.version,a.label,a.payload,a.status,a.diagnostic,
                    a.created_event,a.created_at
             FROM artifact_versions a
             JOIN events e ON e.id=a.created_event
             WHERE e.branch=?1
             ORDER BY a.kind,a.version",
        )?;
        let rows = s.query_map(params![branch], map_artifact)?;
        rows.collect()
    }

    pub fn all_artifacts(&self) -> rusqlite::Result<Vec<ArtifactVersionRow>> {
        let c = self.0.lock().unwrap();
        let mut s = c.prepare(
            "SELECT art_id,kind,version,label,payload,status,diagnostic,created_event,created_at
             FROM artifact_versions ORDER BY kind,art_id,version",
        )?;
        let rows = s.query_map([], map_artifact)?;
        rows.collect()
    }

    pub fn results_for_branch(&self, branch: &str) -> rusqlite::Result<Vec<ResultRow>> {
        let c = self.0.lock().unwrap();
        let mut s = c.prepare(
            "SELECT id,branch,scope,label,payload,deps,parent_result,parent_result_version,
                    status,produced_event,superseded_by,created_at
             FROM results WHERE branch=?1 ORDER BY created_at,id",
        )?;
        let rows = s.query_map(params![branch], map_result)?;
        rows.collect()
    }

    pub fn candidates_for_branch(&self, branch: &str) -> rusqlite::Result<Vec<CandidateRow>> {
        let c = self.0.lock().unwrap();
        let mut s = c.prepare(
            "SELECT id,branch,scope,label,payload,rationale,status,produced_event,
                    adjudicated_event,created_at
             FROM candidates WHERE branch=?1 ORDER BY created_at,id",
        )?;
        let rows = s.query_map(params![branch], |r| {
            Ok(CandidateRow {
                id: r.get(0)?,
                branch: r.get(1)?,
                scope: r.get(2)?,
                label: r.get(3)?,
                payload: r.get(4)?,
                rationale: r.get(5)?,
                status: r.get(6)?,
                produced_event: r.get(7)?,
                adjudicated_event: r.get(8)?,
                created_at: r.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn count_results(&self) -> rusqlite::Result<i64> {
        self.0
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM results", [], |r| r.get(0))
    }
}

fn map_artifact(r: &rusqlite::Row) -> rusqlite::Result<ArtifactVersionRow> {
    Ok(ArtifactVersionRow {
        art_id: r.get(0)?,
        kind: r.get(1)?,
        version: r.get(2)?,
        label: r.get(3)?,
        payload: r.get(4)?,
        status: r.get(5)?,
        diagnostic: r.get(6)?,
        created_event: r.get(7)?,
        created_at: r.get(8)?,
    })
}

fn map_result(r: &rusqlite::Row) -> rusqlite::Result<ResultRow> {
    Ok(ResultRow {
        id: r.get(0)?,
        branch: r.get(1)?,
        scope: r.get(2)?,
        label: r.get(3)?,
        payload: r.get(4)?,
        deps: r.get(5)?,
        parent_result: r.get(6)?,
        parent_result_version: r.get(7)?,
        status: r.get(8)?,
        produced_event: r.get(9)?,
        superseded_by: r.get(10)?,
        created_at: r.get(11)?,
    })
}

/// 构造事件行的工具：seq 由调用方在事务内分配。
pub fn make_event(
    id: String,
    seq: i64,
    branch: &str,
    kind: EventType,
    title: impl Into<String>,
    detail: Json,
    now: i64,
) -> EventRow {
    EventRow {
        id,
        seq,
        branch: branch.to_string(),
        kind: kind.as_str().to_string(),
        title: title.into(),
        detail,
        created_at: now,
    }
}
