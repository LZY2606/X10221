//! SQLite 存储层：追加式（append-only）谱系表 + 显式迁移。
//! 除 drafts 的当前头指针外，所有表只插不改；过期通过 invalidations 表表达，旧结论不被覆盖。
use rusqlite::{params, Connection};
use serde_json::Value;

/// 迁移列表：按顺序应用，版本号 = 索引 + 1。
pub const MIGRATIONS: &[&str] = &[
    // v1：初始结构。来源（artifacts/datasets）与解释（populations/rulings）分开保存。
    r#"
    CREATE TABLE events(
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        ts REAL NOT NULL,
        kind TEXT NOT NULL,
        payload TEXT NOT NULL
    );
    CREATE TABLE artifacts(
        kind TEXT NOT NULL,
        logical_id TEXT NOT NULL,
        version INTEGER NOT NULL,
        content_hash TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        PRIMARY KEY(kind, logical_id, version)
    );
    CREATE TABLE datasets(
        id TEXT NOT NULL,
        version INTEGER NOT NULL,
        payload TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        PRIMARY KEY(id, version)
    );
    CREATE TABLE runs(
        id TEXT PRIMARY KEY,
        dataset_id TEXT NOT NULL,
        dataset_version INTEGER NOT NULL,
        comp_ref TEXT NOT NULL,
        transform_ref TEXT NOT NULL,
        gates_ref TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE populations(
        id TEXT PRIMARY KEY,
        run_id TEXT NOT NULL,
        gate_name TEXT NOT NULL,
        parent_pop TEXT,
        parent_pop_seq INTEGER,
        count_in INTEGER NOT NULL,
        count_boundary INTEGER NOT NULL,
        unevaluable INTEGER NOT NULL,
        denominator INTEGER NOT NULL,
        pct REAL NOT NULL,
        candidates TEXT NOT NULL,
        deps TEXT NOT NULL,
        diag TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE invalidations(
        target_kind TEXT NOT NULL,
        target_id TEXT NOT NULL,
        reason TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE branches(
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE rulings(
        id TEXT PRIMARY KEY,
        branch_id TEXT NOT NULL,
        target_kind TEXT NOT NULL,
        target_id TEXT NOT NULL,
        decision TEXT NOT NULL,
        params TEXT NOT NULL,
        rationale TEXT NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE drafts(
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        version INTEGER NOT NULL,
        created_seq INTEGER NOT NULL
    );
    CREATE TABLE draft_versions(
        draft_id TEXT NOT NULL,
        version INTEGER NOT NULL,
        payload TEXT NOT NULL,
        created_seq INTEGER NOT NULL,
        PRIMARY KEY(draft_id, version)
    );
    "#,
    // v2：事件增加 actor 列与常用索引。
    r#"
    ALTER TABLE events ADD COLUMN actor TEXT NOT NULL DEFAULT 'system';
    CREATE INDEX idx_events_kind ON events(kind);
    CREATE INDEX idx_invalidations_target ON invalidations(target_kind, target_id);
    CREATE INDEX idx_populations_run ON populations(run_id);
    "#,
];

pub const EXPORT_TABLES: &[&str] = &[
    "events",
    "artifacts",
    "datasets",
    "runs",
    "populations",
    "invalidations",
    "branches",
    "rulings",
    "drafts",
    "draft_versions",
];

#[derive(Debug)]
pub struct ArtifactConflict {
    pub kind: String,
    pub logical_id: String,
    pub version: u32,
    pub existing_hash: String,
    pub incoming_hash: String,
}

pub struct Store {
    pub conn: Connection,
}

fn now_ts() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl Store {
    pub fn open(path: &std::path::Path) -> rusqlite::Result<Store> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_memory() -> rusqlite::Result<Store> {
        let conn = Connection::open_in_memory()?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, applied_at REAL NOT NULL);",
        )?;
        let current: u32 = self
            .conn
            .query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |r| r.get(0))
            .unwrap_or(0);
        for (idx, sql) in MIGRATIONS.iter().enumerate() {
            let version = (idx + 1) as u32;
            if version > current {
                self.conn.execute_batch(sql)?;
                self.conn.execute(
                    "INSERT INTO schema_migrations(version, applied_at) VALUES(?1, ?2)",
                    params![version, now_ts()],
                )?;
            }
        }
        Ok(())
    }

    /// 记录一条机器可读事件，返回全局序号（即时间旅行锚点）。
    pub fn record_event(&self, kind: &str, payload: &Value) -> rusqlite::Result<i64> {
        self.record_event_as(kind, payload, "system")
    }

    pub fn record_event_as(&self, kind: &str, payload: &Value, actor: &str) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO events(ts, kind, payload, actor) VALUES(?1, ?2, ?3, ?4)",
            params![now_ts(), kind, payload.to_string(), actor],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// 写入 artifact 版本。同版本同内容 → 幂等；同版本不同内容 → 冲突事件 + 拒绝（版本不可变）。
    pub fn put_artifact(
        &self,
        kind: &str,
        logical_id: &str,
        version: u32,
        payload: &Value,
    ) -> Result<String, ArtifactConflict> {
        let hash = crate::ids::hash_value(payload);
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT content_hash FROM artifacts WHERE kind=?1 AND logical_id=?2 AND version=?3",
                params![kind, logical_id, version],
                |r| r.get(0),
            )
            .ok();
        if let Some(existing_hash) = existing {
            if existing_hash == hash {
                return Ok(hash);
            }
            let _ = self.record_event(
                "artifact.conflict",
                &serde_json::json!({
                    "kind": kind, "logical_id": logical_id, "version": version,
                    "existing_hash": existing_hash, "incoming_hash": hash,
                    "rationale": "同一版本号不允许承载不同内容；版本不可变，需递增版本",
                }),
            );
            return Err(ArtifactConflict {
                kind: kind.into(),
                logical_id: logical_id.into(),
                version,
                existing_hash,
                incoming_hash: hash,
            });
        }
        let seq = self
            .record_event(
                "artifact.stored",
                &serde_json::json!({
                    "kind": kind, "logical_id": logical_id, "version": version, "content_hash": hash,
                }),
            )
            .map_err(|_| ArtifactConflict {
                kind: kind.into(),
                logical_id: logical_id.into(),
                version,
                existing_hash: String::new(),
                incoming_hash: hash.clone(),
            })?;
        self.conn
            .execute(
                "INSERT INTO artifacts(kind, logical_id, version, content_hash, payload, created_seq) VALUES(?1,?2,?3,?4,?5,?6)",
                params![kind, logical_id, version, hash, payload.to_string(), seq],
            )
            .map_err(|_| ArtifactConflict {
                kind: kind.into(),
                logical_id: logical_id.into(),
                version,
                existing_hash: String::new(),
                incoming_hash: hash.clone(),
            })?;
        Ok(hash)
    }

    pub fn get_artifact(&self, kind: &str, logical_id: &str, version: u32) -> Option<Value> {
        let payload: String = self
            .conn
            .query_row(
                "SELECT payload FROM artifacts WHERE kind=?1 AND logical_id=?2 AND version=?3",
                params![kind, logical_id, version],
                |r| r.get(0),
            )
            .ok()?;
        serde_json::from_str(&payload).ok()
    }

    pub fn artifact_hash(&self, kind: &str, logical_id: &str, version: u32) -> Option<String> {
        self.conn
            .query_row(
                "SELECT content_hash FROM artifacts WHERE kind=?1 AND logical_id=?2 AND version=?3",
                params![kind, logical_id, version],
                |r| r.get(0),
            )
            .ok()
    }

    pub fn latest_version(&self, kind: &str, logical_id: &str) -> Option<u32> {
        self.conn
            .query_row(
                "SELECT MAX(version) FROM artifacts WHERE kind=?1 AND logical_id=?2",
                params![kind, logical_id],
                |r| r.get::<_, Option<u32>>(0),
            )
            .ok()
            .flatten()
    }

    /// 标记目标过期（追加式；不修改旧结论）。
    pub fn invalidate(&self, target_kind: &str, target_id: &str, reason: &str) -> rusqlite::Result<i64> {
        let seq = self.record_event(
            "lineage.invalidated",
            &serde_json::json!({
                "target_kind": target_kind, "target_id": target_id, "reason": reason,
            }),
        )?;
        self.conn.execute(
            "INSERT INTO invalidations(target_kind, target_id, reason, created_seq) VALUES(?1,?2,?3,?4)",
            params![target_kind, target_id, reason, seq],
        )?;
        Ok(seq)
    }

    /// 目标在事件序号 as_of 时是否已过期（as_of=None 表示当前）。
    pub fn is_stale_at(&self, target_kind: &str, target_id: &str, as_of: Option<i64>) -> bool {
        let sql = match as_of {
            Some(_) => {
                "SELECT COUNT(*) FROM invalidations WHERE target_kind=?1 AND target_id=?2 AND created_seq<=?3"
            }
            None => "SELECT COUNT(*) FROM invalidations WHERE target_kind=?1 AND target_id=?2",
        };
        let n: i64 = match as_of {
            Some(s) => self
                .conn
                .query_row(sql, params![target_kind, target_id, s], |r| r.get(0))
                .unwrap_or(0),
            None => self
                .conn
                .query_row(sql, params![target_kind, target_id], |r| r.get(0))
                .unwrap_or(0),
        };
        n > 0
    }

    /// 通用表查询：返回 (列名, 行) 列表，按 rowid 顺序。
    pub fn dump_table(&self, table: &str) -> Vec<Value> {
        let mut stmt = match self.conn.prepare(&format!("SELECT * FROM {}", table)) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let rows = stmt
            .query_map([], |row| {
                let mut map = serde_json::Map::new();
                for (i, c) in cols.iter().enumerate() {
                    let v: rusqlite::types::Value = row.get(i)?;
                    map.insert(c.clone(), sql_value_to_json(&v));
                }
                Ok(Value::Object(map))
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    /// 导出整库为 JSON（身份、顺序、来源关系全部随数据走）。
    pub fn export_json(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("format".into(), Value::String("flowline-export/1".into()));
        for t in EXPORT_TABLES {
            out.insert(t.to_string(), Value::Array(self.dump_table(t)));
        }
        Value::Object(out)
    }

    /// 导入：清空后按导出顺序恢复，显式保留 seq/created_seq（身份与顺序不变）。
    pub fn import_json(&self, data: &Value) -> Result<(), String> {
        let tx = self.conn.unchecked_transaction().map_err(|e| e.to_string())?;
        for t in EXPORT_TABLES {
            tx.execute(&format!("DELETE FROM {}", t), []).map_err(|e| e.to_string())?;
        }
        for t in EXPORT_TABLES {
            let rows = data
                .get(*t)
                .and_then(|v| v.as_array())
                .ok_or_else(|| format!("export missing table {}", t))?;
            for row in rows {
                let obj = row.as_object().ok_or("row must be object")?;
                let cols: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
                let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{}", i)).collect();
                let sql = format!(
                    "INSERT INTO {}({}) VALUES({})",
                    t,
                    cols.join(","),
                    placeholders.join(",")
                );
                let values: Vec<rusqlite::types::Value> =
                    obj.values().map(json_to_sql_value).collect();
                tx.execute(&sql, rusqlite::params_from_iter(values)).map_err(|e| e.to_string())?;
            }
        }
        // 恢复自增序列，保证导入后继续追加的事件序号不与历史冲突。
        let max_seq: i64 = tx
            .query_row("SELECT COALESCE(MAX(seq),0) FROM events", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM sqlite_sequence WHERE name='events'", []).map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO sqlite_sequence(name, seq) VALUES('events', ?1)", params![max_seq])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn sql_value_to_json(v: &rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as V;
    match v {
        V::Null => Value::Null,
        V::Integer(i) => Value::Number((*i).into()),
        V::Real(f) => serde_json::Number::from_f64(*f).map(Value::Number).unwrap_or(Value::Null),
        V::Text(s) => Value::String(s.clone()),
        V::Blob(b) => Value::String(format!("blob:{} bytes", b.len())),
    }
}

fn json_to_sql_value(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match v {
        Value::Null => V::Null,
        Value::Bool(b) => V::Integer(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                V::Integer(i)
            } else {
                V::Real(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => V::Text(s.clone()),
        other => V::Text(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_version_different_content_conflicts() {
        let store = Store::open_memory().unwrap();
        let p1 = serde_json::json!({"a": 1});
        let p2 = serde_json::json!({"a": 2});
        store.put_artifact("comp_matrix", "m", 1, &p1).unwrap();
        // 幂等：同内容同版本可重复写。
        store.put_artifact("comp_matrix", "m", 1, &p1).unwrap();
        // 同版本不同内容 → 冲突，且旧内容不被覆盖。
        let err = store.put_artifact("comp_matrix", "m", 1, &p2).unwrap_err();
        assert_ne!(err.existing_hash, err.incoming_hash);
        assert_eq!(store.get_artifact("comp_matrix", "m", 1).unwrap(), p1);
        let events = store.dump_table("events");
        assert!(events.iter().any(|e| e["kind"] == "artifact.conflict"));
    }

    #[test]
    fn export_import_roundtrip_preserves_identity_and_order() {
        let store = Store::open_memory().unwrap();
        store.record_event("test.a", &serde_json::json!({"n": 1})).unwrap();
        store.record_event("test.b", &serde_json::json!({"n": 2})).unwrap();
        store.put_artifact("transform", "t", 1, &serde_json::json!({"k": "v"})).unwrap();
        let exported = store.export_json();

        let restored = Store::open_memory().unwrap();
        restored.import_json(&exported).unwrap();
        let reexported = restored.export_json();
        assert_eq!(exported, reexported);
        // 导入后继续追加事件，序号不与历史冲突。
        let seq = restored.record_event("test.c", &serde_json::json!({})).unwrap();
        assert_eq!(seq, 4); // 2 手工事件 + 1 artifact.stored，导入后从 4 继续
    }
}
