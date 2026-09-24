//! SQLite 持久化。事件日志是唯一事实来源；artifacts 表只是查询索引。
//! 导入/修订在单事务内完成，失败整体回滚，不会留下半份记录。

use crate::model::{Event, EventKind};
use rusqlite::{params, Connection};
use serde_json::Value;

const MIGRATIONS: &[&str] = &[
    // v1：初始 schema
    "CREATE TABLE events (
        seq    INTEGER PRIMARY KEY,
        id     TEXT UNIQUE NOT NULL,
        ts     TEXT NOT NULL,
        actor  TEXT NOT NULL,
        kind   TEXT NOT NULL,
        payload TEXT NOT NULL
     );
     CREATE TABLE artifacts (
        id        TEXT NOT NULL,
        version   INTEGER NOT NULL,
        kind      TEXT NOT NULL,
        branch    TEXT NOT NULL,
        payload   TEXT NOT NULL,
        raw       BLOB,
        created_by TEXT NOT NULL,
        PRIMARY KEY (id, version)
     );",
    // v2：事件支持分支；增加冲突/裁定查询索引。
    "ALTER TABLE events ADD COLUMN branch TEXT NOT NULL DEFAULT 'main';
     CREATE INDEX idx_events_kind ON events(kind);
     CREATE INDEX idx_artifacts_kind ON artifacts(kind, branch);",
];

pub struct Store {
    pub conn: Connection,
}

#[derive(Debug)]
pub struct StoreError(pub String);
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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

pub fn migrate(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
         );",
    )?;
    let applied: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
        .unwrap_or(0);
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = i as i64 + 1;
        if v > applied {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                params![v, now_ts()],
            )?;
        }
    }
    Ok(())
}

fn now_ts() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{}", d.as_millis()))
        .unwrap_or_default()
}

impl Store {
    pub fn open(path: &str) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// 测试辅助：纯内存库。
    pub fn open_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    pub fn max_seq(&self) -> Result<u64, StoreError> {
        Ok(self
            .conn
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|v| v as u64)?)
    }

    /// 在事务内执行一批写入；任何错误整体回滚。
    pub fn transaction<F, T>(&mut self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&Connection) -> Result<T, StoreError>,
    {
        let tx = self.conn.transaction()?;
        match f(&tx) {
            Ok(v) => {
                tx.commit()?;
                Ok(v)
            }
            Err(e) => {
                // rollback 由 Drop 自动完成
                let _ = tx.rollback();
                Err(e)
            }
        }
    }

    pub fn insert_event_tx(tx: &Connection, ev: &Event) -> Result<(), StoreError> {
        tx.execute(
            "INSERT INTO events(seq, id, ts, actor, branch, kind, payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                ev.seq as i64,
                ev.id,
                ev.ts,
                ev.actor,
                ev.branch,
                ev.kind.as_str(),
                serde_json::to_string(&ev.payload)?,
            ],
        )?;
        // 物化 artifact 索引
        if ev.kind == EventKind::ArtifactCommitted {
            if let Some(artifact) = ev.payload.get("artifact") {
                let raw: Option<Vec<u8>> = artifact
                    .get("raw_b64")
                    .and_then(|v| v.as_str())
                    .and_then(|s| {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD.decode(s).ok()
                    });
                tx.execute(
                    "INSERT OR REPLACE INTO artifacts(id, version, kind, branch, payload, raw, created_by)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        artifact["id"].as_str(),
                        artifact["version"].as_u64().unwrap() as i64,
                        artifact["kind"].as_str(),
                        artifact["branch"].as_str(),
                        serde_json::to_string(&artifact["payload"])?,
                        raw,
                        artifact["created_by"].as_str(),
                    ],
                )?;
            }
        }
        Ok(())
    }

    pub fn append_event(&mut self, kind: EventKind, branch: &str, actor: &str, payload: Value)
        -> Result<Event, StoreError>
    {
        self.transaction(|tx| Self::append_event_tx(tx, kind, branch, actor, payload))
    }


    pub fn next_seq(tx: &Connection) -> Result<u64, StoreError> {
        Ok(tx
            .query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM events", [], |r| {
                r.get::<_, i64>(0)
            })? as u64)
    }

    pub fn append_event_tx(
        tx: &Connection,
        kind: EventKind,
        branch: &str,
        actor: &str,
        payload: Value,
    ) -> Result<Event, StoreError> {
        let seq = tx
            .query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM events", [], |r| {
                r.get::<_, i64>(0)
            })
            .map(|v| v as u64)?;
        let id = format!("ev-{seq:06}");
        let ev = Event {
            seq,
            id,
            ts: now_ts(),
            actor: actor.to_string(),
            branch: branch.to_string(),
            kind,
            payload,
        };
        Self::insert_event_tx(tx, &ev)?;
        Ok(ev)
    }

    pub fn load_events(&self) -> Result<Vec<Event>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, id, ts, actor, branch, kind, payload FROM events ORDER BY seq")?;
        let rows = stmt.query_map([], |r| {
            let kind_s: String = r.get(5)?;
            let payload_s: String = r.get(6)?;
            Ok((r, kind_s, payload_s))
        })?;
        let mut out = Vec::new();
        for item in rows {
            let (r, kind_s, payload_s) = item?;
            let kind = EventKind::from_str(&kind_s)
                .ok_or_else(|| rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(StoreError(format!("未知事件类型 {kind_s}"))),
                ))?;
            out.push(Event {
                seq: r.get::<_, i64>(0)? as u64,
                id: r.get(1)?,
                ts: r.get(2)?,
                actor: r.get(3)?,
                branch: r.get::<_, Option<String>>(4)?.unwrap_or_else(|| "main".to_string()),
                kind,
                payload: serde_json::from_str(&payload_s)?,
            });
        }
        Ok(out)
    }

    /// 导出完整档案：事件有序，身份、顺序、来源关系全部在事件负载内。
    pub fn export_json(&self) -> Result<Value, StoreError> {
        let events: Vec<Value> = self
            .load_events()?
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?;
        Ok(serde_json::json!({
            "format": "flowchamber-archive/1",
            "exported_at": now_ts(),
            "events": events,
        }))
    }

    /// 清空后按事件顺序重放；全部在一个事务内，失败回滚。
    pub fn import_json(&mut self, archive: &Value) -> Result<usize, StoreError> {
        let raw_events = archive
            .get("events")
            .and_then(|v| v.as_array())
            .ok_or(StoreError("档案缺少 events 数组".into()))?;
        self.transaction(|tx| {
            tx.execute("DELETE FROM artifacts", [])?;
            tx.execute("DELETE FROM events", [])?;
            let mut n = 0usize;
            for value in raw_events {
                let ev: Event = serde_json::from_value(value.clone())?;
                Self::insert_event_tx(tx, &ev)?;
                n += 1;
            }
            Ok(n)
        })
    }

    pub fn wipe(&mut self) -> Result<(), StoreError> {
        self.transaction(|tx| {
            tx.execute("DELETE FROM artifacts", [])?;
            tx.execute("DELETE FROM events", [])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn transaction_rolls_back_on_failure() {
        let mut store = Store::open_memory().unwrap();
        let r = store.transaction(|tx| {
            Store::append_event_tx(tx, EventKind::CaseImported, "main", "tester", json!({}))?;
            Err::<(), _>(StoreError("故意失败".into()))
        });
        assert!(r.is_err());
        assert_eq!(store.max_seq().unwrap(), 0);
    }

    /// 状态迁移测试：先手工建出 v1 库（没有 branch 列），写入事件后执行迁移，
    /// 旧数据必须保留，新列以默认值 'main' 补齐。
    #[test]
    fn migration_v1_to_v2_preserves_events() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);
             INSERT INTO schema_migrations(version, applied_at) VALUES (1, 't0');",
        )
        .unwrap();
        // v1 表没有 branch 列
        conn.execute(
            "INSERT INTO events(seq, id, ts, actor, kind, payload)
             VALUES (1, 'ev-000001', 't0', 'legacy', 'case_imported', '{}')",
            [],
        )
        .unwrap();

        migrate(&conn).unwrap();

        let (version,): (i64,) = conn
            .query_row("SELECT version FROM schema_migrations WHERE version = 2", [], |r| {
                Ok(r.get(0)?)
            })
            .unwrap();
        assert_eq!(version, 2);
        let (id, branch): (String, String) = conn
            .query_row(
                "SELECT id, branch FROM events WHERE seq = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(id, "ev-000001");
        assert_eq!(branch, "main");
        // 迁移后索引可用，且可正常追加带分支事件
        conn.execute(
            "INSERT INTO events(seq,id,ts,actor,branch,kind,payload)
             VALUES (2,'ev-000002','t1','x','review-B','invalidated','{}')",
            [],
        )
        .unwrap();
    }
}
