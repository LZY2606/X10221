//! SQLite 存储层：来源材料与解释分层、追加式版本化、机器可读取事件、
//! 乐观并发裁定、群体分母版本钉住、全量导出/导入。

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::sync::Mutex;

pub struct Store {
    conn: Mutex<Connection>,
}

const V1: &str = r#"
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta(key,value) VALUES ('global_seq','0');
CREATE TABLE materials (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  version INTEGER NOT NULL,
  supersedes TEXT,
  branch TEXT NOT NULL DEFAULT 'main',
  raw_json TEXT NOT NULL,
  unit_json TEXT,
  seq INTEGER NOT NULL
);
CREATE TABLE events (
  seq INTEGER PRIMARY KEY,
  id TEXT UNIQUE NOT NULL,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  ts TEXT NOT NULL
);
CREATE TABLE populations (
  id TEXT PRIMARY KEY,
  gate_id TEXT NOT NULL,
  gate_version INTEGER NOT NULL,
  parent_pop_id TEXT,
  count INTEGER NOT NULL,
  denom_count INTEGER,
  pct REAL,
  comp_material_id TEXT,
  transform_version INTEGER NOT NULL,
  status TEXT NOT NULL,
  branch TEXT NOT NULL,
  source_ruling TEXT,
  seq INTEGER NOT NULL
);
"#;

const V2: &str = r#"
CREATE TABLE candidates (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  subject TEXT NOT NULL,
  options_json TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open',
  resolved_ruling TEXT,
  branch TEXT NOT NULL DEFAULT 'main',
  seq INTEGER NOT NULL
);
CREATE TABLE rulings (
  id TEXT PRIMARY KEY,
  seq INTEGER NOT NULL,
  target_kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  decision TEXT NOT NULL,
  option TEXT,
  params_json TEXT,
  base_seq INTEGER NOT NULL,
  branch TEXT NOT NULL,
  undone INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE diagnostics (
  id TEXT PRIMARY KEY,
  material_id TEXT NOT NULL,
  position TEXT NOT NULL,
  severity TEXT NOT NULL,
  message TEXT NOT NULL,
  seq INTEGER NOT NULL
);
"#;

const V3: &str = r#"
CREATE INDEX idx_materials_kind ON materials(kind, version);
CREATE INDEX idx_populations_branch ON populations(branch, status);
CREATE INDEX idx_rulings_target ON rulings(target_kind, target_id);
CREATE INDEX idx_events_kind ON events(kind);
"#;

impl Store {
    pub fn open(path: &str) -> crate::err::Result<Store> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn memory() -> crate::err::Result<Store> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> crate::err::Result<Store> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut s = Store {
            conn: Mutex::new(conn),
        };
        s.migrate()?;
        Ok(s)
    }

    pub fn migrate(&mut self) -> crate::err::Result<()> {
        let conn = self.conn.get_mut().unwrap();
        let current: i64 =
            conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let steps: Vec<(i64, &str)> = vec![(1, V1), (2, V2), (3, V3)];
        for (ver, sql) in steps {
            if current < ver {
                conn.execute_batch(sql)?;
                conn.execute_batch(&format!("PRAGMA user_version = {ver}"))?;
            }
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn force_schema_v1(&self) -> crate::err::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("DROP TABLE IF EXISTS meta")?;
        conn.execute_batch("DROP TABLE IF EXISTS candidates")?;
        conn.execute_batch("DROP TABLE IF EXISTS rulings")?;
        conn.execute_batch("DROP TABLE IF EXISTS diagnostics")?;
        conn.execute_batch("DROP TABLE IF EXISTS materials")?;
        conn.execute_batch("DROP TABLE IF EXISTS events")?;
        conn.execute_batch("DROP TABLE IF EXISTS populations")?;
        conn.execute_batch(V1)?;
        conn.execute_batch("PRAGMA user_version = 1")?;
        Ok(())
    }

    pub fn schema_version(&self) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn next_seq(&self) -> i64 {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        let cur: i64 = tx
            .query_row("SELECT value FROM meta WHERE key='global_seq'", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        let nxt = cur + 1;
        tx.execute(
            "UPDATE meta SET value=?1 WHERE key='global_seq'",
            params![nxt.to_string()],
        )
        .unwrap();
        tx.commit().unwrap();
        nxt
    }
}

impl Store {
    pub fn add_event(&self, kind: &str, payload: Value) -> (i64, String) {
        let seq = self.next_seq();
        let id = format!("evt:{seq:06}");
        let ts = format!("seq-{seq}");
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO events(seq,id,kind,payload_json,ts) VALUES (?1,?2,?3,?4,?5)",
                params![seq, id, kind, payload.to_string(), ts],
            )
            .unwrap();
        (seq, id)
    }

    pub fn insert_material(
        &self,
        kind: &str,
        raw: Value,
        units: Option<Value>,
        supersedes: Option<&str>,
        branch: &str,
    ) -> (i64, String) {
        let seq = self.next_seq();
        let version = {
            let conn = self.conn.lock().unwrap();
            conn.query_row(
                "SELECT COALESCE(MAX(version),0)+1 FROM materials WHERE kind=?1",
                params![kind],
                |r| r.get(0),
            )
            .unwrap_or(1)
        };
        let id = format!("mat:{kind}:v{version}");
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO materials(id,kind,version,supersedes,branch,raw_json,unit_json,seq)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    id,
                    kind,
                    version,
                    supersedes,
                    branch,
                    raw.to_string(),
                    units.map(|u| u.to_string()),
                    seq
                ],
            )
            .unwrap();
        (seq, id)
    }

    pub fn latest_material(&self, kind: &str) -> Option<(i64, String, Value)> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT seq,id,raw_json FROM materials WHERE kind=?1
                 ORDER BY version DESC LIMIT 1",
                params![kind],
                |r| {
                    let seq: i64 = r.get(0)?;
                    let id: String = r.get(1)?;
                    let raw: String = r.get(2)?;
                    Ok((seq, id, serde_json::from_str(&raw).unwrap_or(Value::Null)))
                },
            )
            .optional()
            .unwrap()
    }

    pub fn material_by_id(&self, id: &str) -> Option<Value> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT raw_json FROM materials WHERE id=?1",
                params![id],
                |r| {
                    let raw: String = r.get(0)?;
                    Ok(serde_json::from_str(&raw).unwrap_or(Value::Null))
                },
            )
            .optional()
            .unwrap()
    }
}

impl Store {
    pub fn insert_candidate(
        &self,
        kind: &str,
        subject: &str,
        options: Value,
        branch: &str,
    ) -> (i64, String) {
        let seq = self.next_seq();
        let id = format!("cand:{seq:06}");
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO candidates(id,kind,subject,options_json,status,branch,seq)
                 VALUES (?1,?2,?3,?4,'open',?5,?6)",
                params![id, kind, subject, options.to_string(), branch, seq],
            )
            .unwrap();
        (seq, id)
    }

    pub fn resolve_candidate(&self, id: &str, ruling_id: &str, status: &str) {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE candidates SET status=?1, resolved_ruling=?2 WHERE id=?3",
                params![status, ruling_id, id],
            )
            .unwrap();
    }

    /// 写入裁定。同目标上 base_seq 落后于已接受裁定 => 版本冲突（保留双方差异，不覆盖）。
    pub fn insert_ruling(
        &self,
        target_kind: &str,
        target_id: &str,
        decision: &str,
        option: Option<&str>,
        params: Value,
        base_seq: i64,
        branch: &str,
    ) -> Result<RulingOut, ConflictOut> {
        let last: Option<(i64, String, String)> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT seq,decision,COALESCE(option,'') FROM rulings
                 WHERE target_kind=?1 AND target_id=?2 AND undone=0
                 ORDER BY seq DESC LIMIT 1",
                params![target_kind, target_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .unwrap();
        if let Some((their_seq, their_decision, their_option)) = last {
            if their_seq > base_seq {
                return Err(ConflictOut {
                    target_id: target_id.to_string(),
                    base_seq,
                    yours: json!({ "decision": decision, "option": option }),
                    theirs: json!({ "seq": their_seq, "decision": their_decision,
                                    "option": their_option }),
                });
            }
        }
        let seq = self.next_seq();
        let id = format!("rule:{seq:06}");
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO rulings(id,seq,target_kind,target_id,decision,option,
                                     params_json,base_seq,branch,undone)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0)",
                params![
                    id,
                    seq,
                    target_kind,
                    target_id,
                    decision,
                    option,
                    params.to_string(),
                    base_seq,
                    branch
                ],
            )
            .unwrap();
        Ok(RulingOut { seq, id })
    }

    pub fn active_ruling_for(&self, target_kind: &str, target_id: &str) -> Option<Value> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT seq,id,decision,COALESCE(option,''),COALESCE(params_json,'{}'),branch
                 FROM rulings WHERE target_kind=?1 AND target_id=?2 AND undone=0
                 ORDER BY seq DESC LIMIT 1",
                params![target_kind, target_id],
                |r| {
                    Ok(json!({
                        "seq": r.get::<_, i64>(0)?,
                        "id": r.get::<_, String>(1)?,
                        "decision": r.get::<_, String>(2)?,
                        "option": r.get::<_, String>(3)?,
                        "params": serde_json::from_str::<Value>(
                            &r.get::<_, String>(4)?).unwrap_or(Value::Null),
                        "branch": r.get::<_, String>(5)?,
                    }))
                },
            )
            .optional()
            .unwrap()
    }

    pub fn undo_ruling(&self, ruling_id: &str) -> bool {
        self.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE rulings SET undone=1 WHERE id=?1 AND undone=0",
                params![ruling_id],
            )
            .unwrap()
            > 0
    }
}

pub struct RulingOut {
    pub seq: i64,
    pub id: String,
}

pub struct ConflictOut {
    pub target_id: String,
    pub base_seq: i64,
    pub yours: Value,
    pub theirs: Value,
}

impl Store {
    /// 写入群体（同一稳定身份保留首个 seq）；分母 parent_pop_id 在计算时钉住。
    pub fn upsert_population(&self, p: &PopRow) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO populations(id,gate_id,gate_version,parent_pop_id,count,denom_count,
                 pct,comp_material_id,transform_version,status,branch,source_ruling,seq)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(id) DO UPDATE SET
               count=excluded.count, denom_count=excluded.denom_count,
               pct=excluded.pct, status=CASE WHEN populations.status='stale'
                 THEN 'current' ELSE populations.status END,
               source_ruling=COALESCE(excluded.source_ruling, populations.source_ruling)",
            params![
                p.id,
                p.gate_id,
                p.gate_version,
                p.parent_pop_id,
                p.count,
                p.denom_count,
                p.pct,
                p.comp_material_id,
                p.transform_version,
                p.status,
                p.branch,
                p.source_ruling,
                p.seq
            ],
        )
        .unwrap();
    }

    pub fn mark_populations_stale(&self, ids: &[String], cause_seq: i64) -> usize {
        if ids.is_empty() {
            return 0;
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        let mut n = 0usize;
        for id in ids {
            n += tx
                .execute(
                    "UPDATE populations SET status='stale' WHERE id=?1 AND status='current'",
                    params![id],
                )
                .unwrap();
        }
        tx.execute(
            "INSERT INTO events(seq,id,kind,payload_json,ts) VALUES (?1,?2,?3,?4,?5)",
            params![
                cause_seq,
                format!("evt:{cause_seq:06}"),
                "lineage.stale_marked",
                json!({ "affected": ids, "cause_seq": cause_seq }).to_string(),
                format!("seq-{cause_seq}")
            ],
        )
        .unwrap();
        tx.commit().unwrap();
        n
    }

    pub fn list_populations(&self, branch: &str) -> Vec<Value> {
        let mut stmt = self
            .conn
            .lock()
            .unwrap()
            .prepare(
                "SELECT id,gate_id,gate_version,parent_pop_id,count,denom_count,pct,
                        comp_material_id,transform_version,status,branch,
                        COALESCE(source_ruling,''),seq
                 FROM populations WHERE branch=?1 ORDER BY seq",
            )
            .unwrap();
        stmt.query_map(params![branch], |r| {
            Ok(json!({
                "id": r.get::<_,String>(0)?,
                "gate_id": r.get::<_,String>(1)?,
                "gate_version": r.get::<_,i64>(2)?,
                "parent_pop_id": r.get::<_,Option<String>>(3)?,
                "count": r.get::<_,i64>(4)?,
                "denom_count": r.get::<_,Option<i64>>(5)?,
                "pct": r.get::<_,Option<f64>>(6)?,
                "comp_material_id": r.get::<_,Option<String>>(7)?,
                "transform_version": r.get::<_,i64>(8)?,
                "status": r.get::<_,String>(9)?,
                "branch": r.get::<_,String>(10)?,
                "source_ruling": r.get::<_,String>(11)?,
                "seq": r.get::<_,i64>(12)?,
            }))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    pub fn all_branches(&self) -> Vec<String> {
        let mut stmt = self
            .conn
            .lock()
            .unwrap()
            .prepare("SELECT DISTINCT branch FROM populations ORDER BY branch")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }
}

pub struct PopRow<'a> {
    pub id: String,
    pub gate_id: &'a str,
    pub gate_version: i64,
    pub parent_pop_id: Option<String>,
    pub count: i64,
    pub denom_count: Option<i64>,
    pub pct: Option<f64>,
    pub comp_material_id: Option<String>,
    pub transform_version: i64,
    pub status: &'a str,
    pub branch: &'a str,
    pub source_ruling: Option<String>,
    pub seq: i64,
}

impl Store {
    pub fn add_diagnostic(&self, material_id: &str, position: &str, severity: &str, message: &str) -> (i64, String) {
        let seq = self.next_seq();
        let id = format!("diag:{seq:06}");
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO diagnostics(id,material_id,position,severity,message,seq)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![id, material_id, position, severity, message, seq],
            )
            .unwrap();
        (seq, id)
    }

}

impl Store {
    fn dump_query(&self, sql: &str) -> Vec<Value> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql).unwrap();
        let names: Vec<String> = stmt
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let rows = stmt
            .query_map([], |r| {
                let mut map = serde_json::Map::new();
                for (i, name) in names.iter().enumerate() {
                    use rusqlite::types::ValueRef;
                    let v = match r.get_ref(i).unwrap() {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(n) => json!(n),
                        ValueRef::Real(x) => json!(x),
                        ValueRef::Text(t) => {
                            let s = String::from_utf8_lossy(t).into_owned();
                            serde_json::from_str(&s).unwrap_or(Value::String(s))
                        }
                        ValueRef::Blob(b) => {
                            json!(String::from_utf8_lossy(b).into_owned())
                        }
                    };
                    map.insert(name.clone(), v);
                }
                Ok(Value::Object(map))
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn export_archive(&self) -> Value {
        let global_seq: String = self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT value FROM meta WHERE key='global_seq'", [], |r| {
                r.get(0)
            })
            .unwrap_or_else(|_| "0".into());
        json!({
            "format": "flowline-archive",
            "format_version": 1,
            "global_seq": global_seq,
            "meta": self.dump_query("SELECT key,value FROM meta ORDER BY key"),
            "materials": self.dump_query("SELECT * FROM materials ORDER BY seq"),
            "events": self.dump_query("SELECT * FROM events ORDER BY seq"),
            "candidates": self.dump_query("SELECT * FROM candidates ORDER BY seq"),
            "rulings": self.dump_query("SELECT * FROM rulings ORDER BY seq"),
            "diagnostics": self.dump_query("SELECT * FROM diagnostics ORDER BY seq"),
            "populations": self.dump_query("SELECT * FROM populations ORDER BY seq"),
        })
    }

    /// 全量导入：事务内清空并按身份/顺序写回；任一行失败整体回滚，不留半份记录。
    pub fn import_archive(&self, archive: &Value) -> Result<usize, String> {
        if archive.get("format").and_then(|v| v.as_str()) != Some("flowline-archive") {
            return Err("bad archive format".into());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        tx.execute_batch(
            "DELETE FROM populations; DELETE FROM diagnostics; DELETE FROM rulings;
             DELETE FROM candidates; DELETE FROM events; DELETE FROM materials;",
        )
        .map_err(|e| e.to_string())?;
        let mut total = 0usize;
        let tables = [
            ("materials", "INSERT INTO materials VALUES (:id,:kind,:version,:supersedes,:branch,:raw_json,:unit_json,:seq)"),
            ("events", "INSERT INTO events VALUES (:seq,:id,:kind,:payload_json,:ts)"),
            ("candidates", "INSERT INTO candidates VALUES (:id,:kind,:subject,:options_json,:status,:resolved_ruling,:branch,:seq)"),
            ("rulings", "INSERT INTO rulings VALUES (:id,:seq,:target_kind,:target_id,:decision,:option,:params_json,:base_seq,:branch,:undone)"),
            ("diagnostics", "INSERT INTO diagnostics VALUES (:id,:material_id,:position,:severity,:message,:seq)"),
            ("populations", "INSERT INTO populations VALUES (:id,:gate_id,:gate_version,:parent_pop_id,:count,:denom_count,:pct,:comp_material_id,:transform_version,:status,:branch,:source_ruling,:seq)"),
        ];
        for (table, sql) in tables {
            let rows = archive.get(table).and_then(|v| v.as_array());
            let rows = match rows {
                Some(r) => r,
                None => return Err(format!("missing section {table}")),
            };
            let mut stmt = tx.prepare(sql).map_err(|e| e.to_string())?;
            for row in rows {
                let obj = row.as_object().ok_or("row must be object")?;
                let keys: Vec<&str> = sql
                    .split(':')
                    .skip(1)
                    .map(|s| s.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap())
                    .collect();
                let vals: Vec<Option<String>> = keys
                    .iter()
                    .map(|k| match obj.get(*k) {
                        None | Some(Value::Null) => None,
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(other) => Some(other.to_string()),
                    })
                    .collect();
                let refs: Vec<&dyn rusqlite::ToSql> =
                    vals.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
                stmt.raw_execute(&refs).map_err(|e| format!("{table}: {e}"))?;
                total += 1;
            }
        }
        let seq = match archive.get("global_seq") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => "0".to_string(),
        };
        let seq = seq.as_str();
        tx.execute(
            "UPDATE meta SET value=?1 WHERE key='global_seq'",
            params![seq],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(total)
    }

    pub fn reset(&self) {
        self.conn
            .lock()
            .unwrap()
            .execute_batch(
                "DELETE FROM populations; DELETE FROM diagnostics; DELETE FROM rulings;
                 DELETE FROM candidates; DELETE FROM events; DELETE FROM materials;
                 UPDATE meta SET value='0' WHERE key='global_seq';",
            )
            .unwrap();
    }

    pub fn count(&self, table: &str) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                &format!("SELECT COUNT(*) FROM {table}"),
                [],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }
}

impl Store {
    pub fn candidate_for_subject(&self, subject: &str) -> Option<(String, String)> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id,status FROM candidates WHERE subject=?1 ORDER BY seq DESC LIMIT 1",
                params![subject],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap()
    }

    pub fn max_active_ruling_seq(&self) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COALESCE(MAX(seq),0) FROM rulings WHERE undone=0",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    pub fn current_pop_ids(&self, branch: &str) -> Vec<String> {
        let mut stmt = self
            .conn
            .lock()
            .unwrap()
            .prepare("SELECT id FROM populations WHERE branch=?1 AND status='current'")
            .unwrap();
        stmt.query_map(params![branch], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }
}
