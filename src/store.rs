//! SQLite 存储：原始层与派生层分开版本化；可变状态由追加式事件日志折叠得到。
use crate::domain::{DepRef, Diag, EventRow, RawEvent};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

pub const KIND_SPILLOVER: &str = "spillover";
pub const KIND_MATRIX: &str = "matrix";
pub const KIND_TRANSFORM: &str = "transform";
pub const KIND_GATE: &str = "gate";
pub const KIND_TREE: &str = "tree";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRow {
    pub file_id: String,
    pub kind: String,
    pub name: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
    pub imported_event: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FcsParamRow {
    pub file_id: String,
    pub index: i64,
    pub name: String,
    pub stain: String,
    pub range: Option<f64>,
    pub gain: crate::domain::Qty,
    pub amp: String,
    pub bits: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionRow {
    pub kind: String,
    pub id: String,
    pub version: i64,
    pub json: serde_json::Value,
    pub event: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DerivedRow {
    pub derived_id: String,
    pub version: i64,
    pub kind: String, // "compensated" | "transformed"
    pub status: String, // "ready" | "blocked"
    pub payload: serde_json::Value,
    pub deps: Vec<DepRef>,
    pub candidates: Vec<crate::domain::Candidate>,
    pub event: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PopStatRow {
    pub stat_id: String,
    pub version: i64,
    pub gate_id: String,
    pub gate_version: i64,
    pub parent_stat_id: Option<String>,
    pub parent_stat_version: Option<i64>,
    pub count: i64,
    pub denom_count: i64,
    pub pct: f64,
    pub status: String,
    pub deps: Vec<DepRef>,
    pub event: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchRow {
    pub branch_id: String,
    pub parent: Option<String>,
    pub base_event: i64,
    pub head_event: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdjRow {
    pub adj_id: String,
    pub branch_id: String,
    pub subject_id: String,
    pub decision: String,
    pub params: serde_json::Value,
    pub event: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagRow {
    pub diag_id: String,
    pub object_id: String,
    pub diag: Diag,
    pub event: i64,
}

const MIGRATIONS: &[&str] = &[
    // v1: 原始层（文件、FCS 参数、原始事件、事件级诊断）
    "CREATE TABLE files(
       file_id TEXT PRIMARY KEY,
       kind TEXT NOT NULL,
       name TEXT NOT NULL,
       sha256 TEXT NOT NULL,
       bytes BLOB NOT NULL,
       imported_event INTEGER NOT NULL
     );
     CREATE TABLE fcs_params(
       file_id TEXT NOT NULL, idx INTEGER NOT NULL,
       name TEXT NOT NULL, stain TEXT NOT NULL,
       range_value REAL, gain_json TEXT NOT NULL,
       amp TEXT NOT NULL, bits INTEGER NOT NULL,
       PRIMARY KEY(file_id, idx)
     );
     CREATE TABLE events_raw(
       event_id TEXT PRIMARY KEY,
       file_id TEXT NOT NULL,
       row_idx INTEGER NOT NULL,
       values_json TEXT NOT NULL,
       raw_hex_json TEXT NOT NULL
     );
     CREATE TABLE event_diags(
       event_id TEXT NOT NULL,
       seq INTEGER NOT NULL,
       diag_json TEXT NOT NULL,
       PRIMARY KEY(event_id, seq)
     );",
    // v2: 分版本实体（spillover/矩阵/变换/门/树 共表，kind 区分）
    "CREATE TABLE entity_versions(
       kind TEXT NOT NULL,
       id TEXT NOT NULL,
       version INTEGER NOT NULL,
       json TEXT NOT NULL,
       event INTEGER NOT NULL,
       PRIMARY KEY(kind, id, version)
     );",
    // v3: 派生层与依赖图
    "CREATE TABLE derived_sets(
       derived_id TEXT NOT NULL,
       version INTEGER NOT NULL,
       kind TEXT NOT NULL,
       status TEXT NOT NULL,
       payload TEXT NOT NULL,
       candidates TEXT NOT NULL,
       event INTEGER NOT NULL,
       PRIMARY KEY(derived_id, version)
     );
     CREATE TABLE derived_deps(
       derived_id TEXT NOT NULL,
       version INTEGER NOT NULL,
       dep_kind TEXT NOT NULL,
       dep_id TEXT NOT NULL,
       dep_version INTEGER NOT NULL,
       PRIMARY KEY(derived_id, version, dep_kind, dep_id)
     );
     CREATE TABLE pop_stats(
       stat_id TEXT NOT NULL,
       version INTEGER NOT NULL,
       gate_id TEXT NOT NULL,
       gate_version INTEGER NOT NULL,
       parent_stat_id TEXT,
       parent_stat_version INTEGER,
       cnt INTEGER NOT NULL,
       denom_count INTEGER NOT NULL,
       pct REAL NOT NULL,
       status TEXT NOT NULL,
       event INTEGER NOT NULL,
       PRIMARY KEY(stat_id, version)
     );
     CREATE TABLE popstat_deps(
       stat_id TEXT NOT NULL,
       version INTEGER NOT NULL,
       dep_kind TEXT NOT NULL,
       dep_id TEXT NOT NULL,
       dep_version INTEGER NOT NULL,
       PRIMARY KEY(stat_id, version, dep_kind, dep_id)
     );",
    // v4: 事件日志、分支、裁定、诊断
    "CREATE TABLE event_log(
       event_id INTEGER PRIMARY KEY AUTOINCREMENT,
       kind TEXT NOT NULL,
       payload TEXT NOT NULL,
       created_at TEXT NOT NULL
     );
     CREATE TABLE adjudications(
       adj_id TEXT PRIMARY KEY,
       branch_id TEXT NOT NULL,
       subject_id TEXT NOT NULL,
       decision TEXT NOT NULL,
       params TEXT NOT NULL,
       event INTEGER NOT NULL
     );
     CREATE TABLE diagnostics(
       diag_id TEXT PRIMARY KEY,
       object_id TEXT NOT NULL,
       severity TEXT NOT NULL,
       code TEXT NOT NULL,
       message TEXT NOT NULL,
       payload TEXT NOT NULL,
       event INTEGER NOT NULL
     );",
];

pub struct Store {
    pub conn: Connection,
}

impl Store {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        if path != ":memory:" {
            if let Some(p) = std::path::Path::new(path).parent() {
                std::fs::create_dir_all(p).ok();
            }
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    pub fn open_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    pub fn migrate(&self) -> anyhow::Result<()> {
        let current: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate() {
            let v = (i + 1) as i64;
            if v > current {
                self.conn.execute_batch(sql)?;
                self.conn.execute_batch(&format!("PRAGMA user_version = {}", v))?;
            }
        }
        Ok(())
    }

    pub fn migration_version(&self) -> anyhow::Result<i64> {
        Ok(self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    // ---------- 事件日志（追加式，永不改写） ----------

    pub fn record_event(&self, kind: &str, payload: &serde_json::Value) -> anyhow::Result<i64> {
        self.conn.execute(
            "INSERT INTO event_log(kind, payload, created_at) VALUES (?1, ?2, ?3)",
            params![kind, serde_json::to_string(payload)?, crate::domain::now_iso()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn all_events(&self) -> anyhow::Result<Vec<EventRow>> {
        self.events_before(i64::MAX)
    }

    pub fn events_before(&self, e: i64) -> anyhow::Result<Vec<EventRow>> {
        let mut st = self.conn.prepare(
            "SELECT event_id, kind, payload, created_at FROM event_log WHERE event_id <= ?1 ORDER BY event_id",
        )?;
        let rows = st.query_map(params![e], |r| {
            let p: String = r.get(2)?;
            Ok(EventRow {
                event_id: r.get(0)?,
                kind: r.get(1)?,
                payload: serde_json::from_str(&p).unwrap_or(serde_json::Value::Null),
                created_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn event(&self, id: i64) -> anyhow::Result<Option<EventRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT event_id, kind, payload, created_at FROM event_log WHERE event_id = ?1",
                params![id],
                |r| {
                    let p: String = r.get(2)?;
                    Ok(EventRow {
                        event_id: r.get(0)?,
                        kind: r.get(1)?,
                        payload: serde_json::from_str(&p).unwrap_or(serde_json::Value::Null),
                        created_at: r.get(3)?,
                    })
                },
            )
            .ok())
    }

    // ---------- 原始层 ----------

    pub fn insert_file(&self, f: &FileRow) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO files(file_id, kind, name, sha256, bytes, imported_event) VALUES (?1,?2,?3,?4,?5,?6)",
            params![f.file_id, f.kind, f.name, f.sha256, f.bytes, f.imported_event],
        )?;
        Ok(())
    }

    pub fn list_files(&self) -> anyhow::Result<Vec<(String, String, String)>> {
        let mut st = self.conn.prepare("SELECT file_id, kind, name FROM files ORDER BY file_id")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn file_bytes(&self, file_id: &str) -> Option<Vec<u8>> {
        self.conn
            .query_row("SELECT bytes FROM files WHERE file_id = ?1", params![file_id], |r| r.get(0))
            .ok()
    }

    pub fn insert_fcs_params(&self, p: &FcsParamRow) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO fcs_params(file_id, idx, name, stain, range_value, gain_json, amp, bits) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![p.file_id, p.index, p.name, p.stain, p.range,
                    serde_json::to_string(&p.gain)?, p.amp, p.bits],
        )?;
        Ok(())
    }

    pub fn list_fcs_params(&self, file_id: &str) -> anyhow::Result<Vec<FcsParamRow>> {
        let mut st = self.conn.prepare(
            "SELECT file_id, idx, name, stain, range_value, gain_json, amp, bits FROM fcs_params WHERE file_id=?1 ORDER BY idx",
        )?;
        let rows = st.query_map(params![file_id], |r| {
            let g: String = r.get(5)?;
            Ok(FcsParamRow {
                file_id: r.get(0)?,
                index: r.get(1)?,
                name: r.get(2)?,
                stain: r.get(3)?,
                range: r.get(4)?,
                gain: serde_json::from_str(&g).unwrap(),
                amp: r.get(6)?,
                bits: r.get(7)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn insert_raw_event(&self, ev: &RawEvent) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO events_raw(event_id, file_id, row_idx, values_json, raw_hex_json) VALUES (?1,?2,?3,?4,?5)",
            params![ev.event_id, ev.file_id, ev.row as i64,
                    serde_json::to_string(&ev.values)?, serde_json::to_string(&ev.raw_hex)?],
        )?;
        for (seq, d) in ev.diagnostics.iter().enumerate() {
            self.conn.execute(
                "INSERT INTO event_diags(event_id, seq, diag_json) VALUES (?1,?2,?3)",
                params![ev.event_id, seq as i64, serde_json::to_string(d)?],
            )?;
        }
        Ok(())
    }

    pub fn raw_events(&self, file_id: &str) -> anyhow::Result<Vec<RawEvent>> {
        let mut st = self.conn.prepare(
            "SELECT event_id, file_id, row_idx, values_json, raw_hex_json FROM events_raw WHERE file_id=?1 ORDER BY row_idx",
        )?;
        let rows = st.query_map(params![file_id], |r| {
            let v: String = r.get(3)?;
            let h: String = r.get(4)?;
            Ok(RawEvent {
                event_id: r.get(0)?,
                file_id: r.get(1)?,
                row: r.get::<_, i64>(2)? as usize,
                values: serde_json::from_str(&v).unwrap_or_default(),
                raw_hex: serde_json::from_str(&h).unwrap_or_default(),
                diagnostics: Vec::new(),
            })
        })?;
        let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut dq = self
            .conn
            .prepare("SELECT diag_json FROM event_diags WHERE event_id=?1 ORDER BY seq")?;
        for ev in out.iter_mut() {
            let diags = dq
                .query_map(params![ev.event_id], |r| {
                    let s: String = r.get(0)?;
                    Ok(serde_json::from_str::<Diag>(&s).unwrap())
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ev.diagnostics = diags;
        }
        Ok(out)
    }

    // ---------- 分版本实体 ----------

    pub fn put_version(&self, kind: &str, id: &str, version: i64, json: &serde_json::Value, event: i64) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO entity_versions(kind, id, version, json, event) VALUES (?1,?2,?3,?4,?5)",
            params![kind, id, version, serde_json::to_string(json)?, event],
        )?;
        Ok(())
    }

    pub fn get_version(&self, kind: &str, id: &str, version: i64) -> anyhow::Result<Option<VersionRow>> {
        let row = self.conn.query_row(
            "SELECT kind, id, version, json, event FROM entity_versions WHERE kind=?1 AND id=?2 AND version=?3",
            params![kind, id, version],
            |r| {
                let j: String = r.get(3)?;
                Ok(VersionRow {
                    kind: r.get(0)?,
                    id: r.get(1)?,
                    version: r.get(2)?,
                    json: serde_json::from_str(&j).unwrap_or(serde_json::Value::Null),
                    event: r.get(4)?,
                })
            },
        );
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn latest_version_num(&self, kind: &str, id: &str) -> anyhow::Result<Option<i64>> {
        let v: Option<i64> = self.conn.query_row(
            "SELECT MAX(version) FROM entity_versions WHERE kind=?1 AND id=?2",
            params![kind, id],
            |r| r.get(0),
        )?;
        Ok(v)
    }

    pub fn latest_version(&self, kind: &str, id: &str) -> anyhow::Result<Option<VersionRow>> {
        match self.latest_version_num(kind, id)? {
            Some(v) => self.get_version(kind, id, v),
            None => Ok(None),
        }
    }

    pub fn list_version_nums(&self, kind: &str, id: &str) -> anyhow::Result<Vec<i64>> {
        let mut st = self.conn.prepare(
            "SELECT version FROM entity_versions WHERE kind=?1 AND id=?2 ORDER BY version",
        )?;
        let rows = st.query_map(params![kind, id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_entities(&self, kind: &str) -> anyhow::Result<Vec<String>> {
        let mut st = self
            .conn
            .prepare("SELECT DISTINCT id FROM entity_versions WHERE kind=?1 ORDER BY id")?;
        let rows = st.query_map(params![kind], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---------- 派生层 ----------

    pub fn insert_derived(&self, d: &DerivedRow) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO derived_sets(derived_id, version, kind, status, payload, candidates, event) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![d.derived_id, d.version, d.kind, d.status,
                    serde_json::to_string(&d.payload)?, serde_json::to_string(&d.candidates)?, d.event],
        )?;
        for dep in &d.deps {
            self.conn.execute(
                "INSERT INTO derived_deps(derived_id, version, dep_kind, dep_id, dep_version) VALUES (?1,?2,?3,?4,?5)",
                params![d.derived_id, d.version, dep.kind, dep.id, dep.version],
            )?;
        }
        Ok(())
    }

    pub fn get_derived(&self, derived_id: &str, version: i64) -> anyhow::Result<Option<DerivedRow>> {
        let row = self.conn.query_row(
            "SELECT derived_id, version, kind, status, payload, candidates, event FROM derived_sets WHERE derived_id=?1 AND version=?2",
            params![derived_id, version],
            |r| {
                let p: String = r.get(4)?;
                let c: String = r.get(5)?;
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?, p, c, r.get::<_, i64>(6)?))
            },
        );
        let (did, ver, kind, status, p, c, event) = match row {
            Ok(t) => t,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut st = self.conn.prepare(
            "SELECT dep_kind, dep_id, dep_version FROM derived_deps WHERE derived_id=?1 AND version=?2",
        )?;
        let deps = st
            .query_map(params![did, ver], |r| {
                Ok(DepRef { kind: r.get(0)?, id: r.get(1)?, version: r.get(2)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(DerivedRow {
            derived_id: did,
            version: ver,
            kind,
            status,
            payload: serde_json::from_str(&p).unwrap_or(serde_json::Value::Null),
            deps,
            candidates: serde_json::from_str(&c).unwrap_or_default(),
            event,
        }))
    }

    pub fn latest_derived(&self, derived_id: &str) -> anyhow::Result<Option<DerivedRow>> {
        let v: Option<i64> = self.conn.query_row(
            "SELECT MAX(version) FROM derived_sets WHERE derived_id=?1",
            params![derived_id],
            |r| r.get(0),
        )?;
        match v {
            Some(v) => self.get_derived(derived_id, v),
            None => Ok(None),
        }
    }

    pub fn derived_versions(&self, derived_id: &str) -> anyhow::Result<Vec<i64>> {
        let mut st = self
            .conn
            .prepare("SELECT version FROM derived_sets WHERE derived_id=?1 ORDER BY version")?;
        let rows = st.query_map(params![derived_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 反查：哪些派生集合依赖某个 (kind, id)。
    pub fn derived_depending_on(&self, dep_kind: &str, dep_id: &str) -> anyhow::Result<Vec<(String, i64, i64)>> {
        let mut st = self.conn.prepare(
            "SELECT derived_id, version, dep_version FROM derived_deps WHERE dep_kind=?1 AND dep_id=?2",
        )?;
        let rows = st.query_map(params![dep_kind, dep_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---------- 群体统计（分母钉住父群版本） ----------

    pub fn insert_popstat(&self, s: &PopStatRow) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO pop_stats(stat_id, version, gate_id, gate_version, parent_stat_id, parent_stat_version, cnt, denom_count, pct, status, event)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![s.stat_id, s.version, s.gate_id, s.gate_version,
                    s.parent_stat_id, s.parent_stat_version, s.count, s.denom_count, s.pct, s.status, s.event],
        )?;
        for dep in &s.deps {
            self.conn.execute(
                "INSERT INTO popstat_deps(stat_id, version, dep_kind, dep_id, dep_version) VALUES (?1,?2,?3,?4,?5)",
                params![s.stat_id, s.version, dep.kind, dep.id, dep.version],
            )?;
        }
        Ok(())
    }

    pub fn get_popstat(&self, stat_id: &str, version: i64) -> anyhow::Result<Option<PopStatRow>> {
        let row = self.conn.query_row(
            "SELECT stat_id, version, gate_id, gate_version, parent_stat_id, parent_stat_version, cnt, denom_count, pct, status, event
             FROM pop_stats WHERE stat_id=?1 AND version=?2",
            params![stat_id, version],
            |r| {
                Ok((
                    r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?, r.get::<_, Option<i64>>(5)?, r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?, r.get::<_, f64>(8)?, r.get::<_, String>(9)?, r.get::<_, i64>(10)?,
                ))
            },
        );
        let t = match row {
            Ok(t) => t,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut st = self
            .conn
            .prepare("SELECT dep_kind, dep_id, dep_version FROM popstat_deps WHERE stat_id=?1 AND version=?2")?;
        let deps = st
            .query_map(params![t.0, t.1], |r| {
                Ok(DepRef { kind: r.get(0)?, id: r.get(1)?, version: r.get(2)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(PopStatRow {
            stat_id: t.0,
            version: t.1,
            gate_id: t.2,
            gate_version: t.3,
            parent_stat_id: t.4,
            parent_stat_version: t.5,
            count: t.6,
            denom_count: t.7,
            pct: t.8,
            status: t.9,
            deps,
            event: t.10,
        }))
    }

    pub fn latest_popstat(&self, stat_id: &str) -> anyhow::Result<Option<PopStatRow>> {
        let v: Option<i64> = self.conn.query_row(
            "SELECT MAX(version) FROM pop_stats WHERE stat_id=?1",
            params![stat_id],
            |r| r.get(0),
        )?;
        match v {
            Some(v) => self.get_popstat(stat_id, v),
            None => Ok(None),
        }
    }

    pub fn popstat_versions(&self, stat_id: &str) -> anyhow::Result<Vec<i64>> {
        let mut st = self
            .conn
            .prepare("SELECT version FROM pop_stats WHERE stat_id=?1 ORDER BY version")?;
        let rows = st.query_map(params![stat_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn popstats_depending_on(&self, dep_kind: &str, dep_id: &str) -> anyhow::Result<Vec<(String, i64, i64)>> {
        let mut st = self.conn.prepare(
            "SELECT stat_id, version, dep_version FROM popstat_deps WHERE dep_kind=?1 AND dep_id=?2",
        )?;
        let rows = st.query_map(params![dep_kind, dep_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn all_popstat_ids(&self) -> anyhow::Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT DISTINCT stat_id FROM pop_stats ORDER BY stat_id")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---------- 裁定（插入式；撤销 = adjudication.reverted 事件） ----------

    pub fn insert_adjudication(&self, a: &AdjRow) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO adjudications(adj_id, branch_id, subject_id, decision, params, event) VALUES (?1,?2,?3,?4,?5,?6)",
            params![a.adj_id, a.branch_id, a.subject_id, a.decision, serde_json::to_string(&a.params)?, a.event],
        )?;
        Ok(())
    }

    pub fn adjudications(&self) -> anyhow::Result<Vec<AdjRow>> {
        let mut st = self.conn.prepare(
            "SELECT adj_id, branch_id, subject_id, decision, params, event FROM adjudications ORDER BY event",
        )?;
        let rows = st.query_map([], |r| {
            let p: String = r.get(4)?;
            Ok(AdjRow {
                adj_id: r.get(0)?,
                branch_id: r.get(1)?,
                subject_id: r.get(2)?,
                decision: r.get(3)?,
                params: serde_json::from_str(&p).unwrap_or(serde_json::Value::Null),
                event: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ---------- 诊断 ----------

    pub fn insert_diagnostic(&self, d: &DiagRow) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO diagnostics(diag_id, object_id, severity, code, message, payload, event) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![d.diag_id, d.object_id, d.diag.severity, d.diag.code, d.diag.message, "{}", d.event],
        )?;
        Ok(())
    }

    pub fn diagnostics(&self) -> anyhow::Result<Vec<DiagRow>> {
        let mut st = self.conn.prepare(
            "SELECT diag_id, object_id, severity, code, message, event FROM diagnostics ORDER BY diag_id",
        )?;
        let rows = st.query_map([], |r| {
            Ok(DiagRow {
                diag_id: r.get(0)?,
                object_id: r.get(1)?,
                diag: Diag { severity: r.get(2)?, code: r.get(3)?, message: r.get(4)? },
                event: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
