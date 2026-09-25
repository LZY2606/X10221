//! SQLite 存储：schema 版本化、导入事务回滚、append-only 事件/派生、
//! 乐观并发冲突检测、整库导出/导入（身份/顺序/来源保持）。

use crate::error::{Error, Result};
use crate::event::{AuditEvent, Branch, EventKind};
use crate::ids::sha256_hex;
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::BTreeMap;

pub const SCHEMA_VERSION: u32 = 1;

pub struct Store {
    pub conn: Connection,
}

fn now_iso() -> String {
    // 确定性不做要求（created_at 不参与内容寻址）。
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("unix:{secs}")
}

impl Store {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let mut s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    fn migrate(&mut self) -> Result<()> {
        self.conn.execute_batch(include_str!("../migrations/schema_v1.sql"))?;
        self.conn.execute(
            "INSERT INTO schema_meta(key, value) VALUES('version', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![SCHEMA_VERSION.to_string()],
        )?;
        Ok(())
    }

    pub fn schema_version(&self) -> Result<u32> {
        let v: String =
            self.conn.query_row("SELECT value FROM schema_meta WHERE key='version'", [], |r| r.get(0))?;
        Ok(v.parse().unwrap_or(0))
    }

    // ---------- branches ----------

    pub fn ensure_branch(&mut self, id: &str, label: &str, forked_from: Option<&str>) -> Result<()> {
        let exists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM branches WHERE id=?1",
            params![id],
            |r| r.get(0),
        )?;
        if exists == 0 {
            self.conn.execute(
                "INSERT INTO branches(id,label,forked_from_event,adopted,created_at)
                 VALUES(?1,?2,?3,1,?4)",
                params![id, label, forked_from, now_iso()],
            )?;
        }
        Ok(())
    }

    pub fn list_branches(&self) -> Result<Vec<Branch>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,label,forked_from_event,adopted FROM branches ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Branch {
                id: r.get(0)?,
                label: r.get(1)?,
                forked_from_event: r.get(2)?,
                adopted: r.get::<_, i64>(3)? != 0,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn set_branch_adopted(&mut self, id: &str, adopted: bool) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE branches SET adopted=?2 WHERE id=?1",
            params![id, adopted as i64],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("branch {id}")));
        }
        Ok(())
    }

    pub fn last_event(&self, branch: &str) -> Result<Option<AuditEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,branch_id,seq,kind,summary,basis_json,snapshot_json,parent_event_id,base_event_id
             FROM events WHERE branch_id=?1 ORDER BY seq DESC LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![branch], row_to_event)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }
}

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEvent> {
    let kind: String = r.get(3)?;
    Ok(AuditEvent {
        id: r.get(0)?,
        branch: r.get(1)?,
        seq: r.get(2)?,
        kind: serde_json::from_value(Value::String(kind)).unwrap(),
        summary: r.get(4)?,
        basis: serde_json::from_str(&r.get::<_, String>(5)?).unwrap(),
        dependency_snapshot: serde_json::from_str(&r.get::<_, String>(6)?).unwrap(),
        parent_event_id: r.get(7)?,
        base_event_id: r.get(8)?,
    })
}

#[allow(dead_code)]
fn kind_str(k: &EventKind) -> &'static str {
    match k {
        EventKind::MaterialImported => "material_imported",
        EventKind::Derivation => "derivation",
        EventKind::Judgement => "judgement",
        EventKind::Decision => "decision",
        EventKind::BranchForked => "branch_forked",
        EventKind::ViewAction => "view_action",
        EventKind::Conflict => "conflict",
    }
}

#[allow(dead_code)]
fn _unused(_: &BTreeMap<String, String>) {}

// ---------- blobs ----------

impl Store {
    /// 写入原始字节（按哈希幂等）。返回 blob id。
    pub fn put_blob(&mut self, name: &str, bytes: &[u8]) -> Result<String> {
        let id = crate::ids::blob_id(bytes);
        let sha = sha256_hex(bytes);
        self.conn.execute(
            "INSERT INTO blobs(id,name,bytes,sha256,created_at)
             VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO NOTHING",
            params![id, name, bytes, sha, now_iso()],
        )?;
        Ok(id)
    }

    pub fn get_blob(&self, id: &str) -> Result<(String, Vec<u8>)> {
        self.conn.query_row(
            "SELECT name, bytes FROM blobs WHERE id=?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).map_err(|_| Error::NotFound(format!("blob {id}")))
    }
}

// ---------- materials ----------

impl Store {
    /// 下一个版本序号，按逻辑对象计数（门按 gate id，其余按 kind）。
    pub fn next_material_seq(&self, logical: &str) -> Result<u32> {
        let pattern = format!("mat_{logical}_v%");
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM materials WHERE id LIKE ?1",
            params![pattern],
            |r| r.get(0),
        )?;
        Ok(n as u32 + 1)
    }

    /// 事务性写入一个材料版本。失败时整体回滚，不留半份记录。
    pub fn write_material(
        &mut self,
        kind: &str,
        logical: &str,
        seq: u32,
        body_json: &str,
        source_blob_id: Option<&str>,
        source_name: Option<&str>,
        note: &str,
    ) -> Result<String> {
        let id = format!(
            "mat_{}_v{:02}_{}",
            logical,
            seq,
            &sha256_hex(crate::ids::canonical_json(&serde_json::from_str::<Value>(body_json)?).as_bytes())[..12]
        );
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO materials(id,kind,seq,body_json,source_blob_id,source_name,note,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(id) DO NOTHING",
            params![id, kind, seq, body_json, source_blob_id, source_name, note, now_iso()],
        )?;
        tx.commit()?;
        Ok(id)
    }

    pub fn get_material_envelope(&self, id: &str) -> Result<crate::materials::MaterialEnvelope> {
        let body = self.conn.query_row(
            "SELECT body_json, kind, seq, source_blob_id, source_name, note
             FROM materials WHERE id=?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? as u32,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        );
        match body {
            Ok((body_json, kind, seq, blob, name, note)) => {
                Ok(crate::materials::MaterialEnvelope {
                    id: id.to_string(),
                    kind,
                    seq,
                    body: serde_json::from_str(&body_json)?,
                    source_blob_id: blob,
                    source_name: name,
                    note,
                })
            }
            Err(_) => Err(Error::NotFound(format!("material {id}"))),
        }
    }

    /// 某逻辑对象（kind 或 gate_<id>）的最新版本信封。
    pub fn latest_material(&self, logical: &str) -> Result<Option<crate::materials::MaterialEnvelope>> {
        let pattern = format!("mat_{logical}_v%");
        let id: Option<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM materials WHERE id LIKE ?1 ORDER BY seq DESC LIMIT 1",
            )?;
            let mut rows = stmt.query_map(params![pattern], |r| r.get::<_, String>(0))?;
            match rows.next() {
                Some(v) => Some(v?),
                None => None,
            }
        };
        match id {
            Some(id) => Ok(Some(self.get_material_envelope(&id)?)),
            None => Ok(None),
        }
    }

    pub fn list_materials(&self) -> Result<Vec<(String, String, u32)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, kind, seq FROM materials ORDER BY kind, seq, id")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? as u32))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

// ---------- derived ----------

impl Store {
    /// 幂等保存一个派生结果（内容寻址，重复导入不产生副本）。
    pub fn save_derived(
        &mut self,
        id: &str,
        kind: &str,
        node_id: Option<&str>,
        branch: &str,
        body: &impl serde::Serialize,
    ) -> Result<()> {
        let json = serde_json::to_string(body)?;
        self.conn.execute(
            "INSERT INTO derived(id,kind,node_id,branch_id,body_json,created_at)
             VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO NOTHING",
            params![id, kind, node_id, branch, json, now_iso()],
        )?;
        Ok(())
    }

    pub fn get_derived_json(&self, id: &str) -> Result<String> {
        self.conn
            .query_row("SELECT body_json FROM derived WHERE id=?1", params![id], |r| {
                r.get::<_, String>(0)
            })
            .map_err(|_| Error::NotFound(format!("derived {id}")))
    }
}

// ---------- events ----------

impl Store {
    /// 追加事件。base_event_id 必须等于该分支当前最后一个事件（乐观并发）。
    /// 不一致时记录冲突并返回 Conflict（同事务写入 conflicts 行，事件不追加）。
    #[allow(clippy::too_many_arguments)]
    pub fn append_event(
        &mut self,
        branch: &str,
        kind: EventKind,
        summary: &str,
        basis: &Value,
        snapshot: &Value,
        base_event_id: Option<&str>,
        attempted_by: &str,
    ) -> Result<AuditEvent> {
        let last = self.last_event(branch)?;
        let last_id = last.as_ref().map(|e| e.id.clone());
        if base_event_id.map(|s| s.to_string()) != last_id {
            // 同版本并发修改：保留输入片段与双方差异。
            let existing = match &last {
                Some(e) => serde_json::to_value(e).unwrap_or(Value::Null),
                None => Value::Null,
            };
            let attempted = serde_json::json!({
                "by": attempted_by,
                "kind": serde_json::to_value(&kind).unwrap(),
                "summary": summary,
                "basis": basis,
            });
            let diff = serde_json::json!({
                "base_event_id": base_event_id,
                "current_head_event_id": last_id,
                "attempted": &attempted,
                "existing": &existing,
            });
            let cid = format!("cnf_{}", &sha256_hex(serde_json::to_vec(&diff)?)[..12]);
            self.conn.execute(
                "INSERT INTO conflicts(id,branch_id,base_event_id,attempted_by,attempted_json,existing_json,diff_json,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(id) DO NOTHING",
                params![
                    cid,
                    branch,
                    base_event_id,
                    attempted_by,
                    attempted.to_string(),
                    existing.to_string(),
                    diff.to_string(),
                    now_iso()
                ],
            )?;
            return Err(Error::Conflict(format!(
                "branch {branch} moved since base {:?}; conflict recorded as {cid}",
                base_event_id
            )));
        }

        let seq = last.as_ref().map(|e| e.seq + 1).unwrap_or(1);
        let parent = last_id.clone();
        let canonical = serde_json::json!({
            "branch": branch, "seq": seq, "kind": kind, "summary": summary,
            "basis": basis, "snapshot": snapshot, "parent": parent, "base": base_event_id,
        });
        let id = format!("evt_{}", &sha256_hex(crate::ids::canonical_json(&canonical).as_bytes())[..16]);
        self.conn.execute(
            "INSERT INTO events(id,branch_id,seq,kind,summary,basis_json,snapshot_json,parent_event_id,base_event_id,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                id,
                branch,
                seq,
                serde_json::to_value(&kind).unwrap().as_str().unwrap(),
                summary,
                basis.to_string(),
                snapshot.to_string(),
                parent,
                base_event_id,
                now_iso()
            ],
        )?;
        Ok(AuditEvent {
            id,
            branch: branch.into(),
            seq,
            kind,
            summary: summary.into(),
            basis: basis.clone(),
            dependency_snapshot: snapshot.clone(),
            parent_event_id: parent,
            base_event_id: base_event_id.map(|s| s.into()),
        })
    }

    pub fn list_events(&self, branch: &str) -> Result<Vec<AuditEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,branch_id,seq,kind,summary,basis_json,snapshot_json,parent_event_id,base_event_id
             FROM events WHERE branch_id=?1 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![branch], row_to_event)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_event(&self, id: &str) -> Result<AuditEvent> {
        self.conn
            .query_row(
                "SELECT id,branch_id,seq,kind,summary,basis_json,snapshot_json,parent_event_id,base_event_id
                 FROM events WHERE id=?1",
                params![id],
                row_to_event,
            )
            .map_err(|_| Error::NotFound(format!("event {id}")))
    }

    pub fn list_conflicts(&self) -> Result<Vec<Value>> {
        let mut stmt = self
            .conn
            .prepare("SELECT diff_json FROM conflicts ORDER BY created_at, id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }
}

// ---------- export / import ----------

/// 导出包：单 JSON 信封，表按稳定顺序排列，身份与外键原样保留。
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ExportPackage {
    pub format: String,
    pub schema_version: u32,
    pub blobs: Vec<BlobRow>,
    pub materials: Vec<MaterialRow>,
    pub derived: Vec<DerivedRow>,
    pub branches: Vec<BranchRow>,
    pub events: Vec<EventRow>,
    pub conflicts: Vec<ConflictRow>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct BlobRow {
    pub id: String,
    pub name: String,
    pub bytes_b64: String,
    pub sha256: String,
    pub created_at: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct MaterialRow {
    pub id: String,
    pub kind: String,
    pub seq: u32,
    pub body_json: String,
    pub source_blob_id: Option<String>,
    pub source_name: Option<String>,
    pub note: String,
    pub created_at: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DerivedRow {
    pub id: String,
    pub kind: String,
    pub node_id: Option<String>,
    pub branch_id: String,
    pub body_json: String,
    pub created_at: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct BranchRow {
    pub id: String,
    pub label: String,
    pub forked_from_event: Option<String>,
    pub adopted: bool,
    pub created_at: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct EventRow {
    pub id: String,
    pub branch_id: String,
    pub seq: u32,
    pub kind: String,
    pub summary: String,
    pub basis_json: String,
    pub snapshot_json: String,
    pub parent_event_id: Option<String>,
    pub base_event_id: Option<String>,
    pub created_at: String,
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ConflictRow {
    pub id: String,
    pub branch_id: String,
    pub base_event_id: String,
    pub attempted_by: String,
    pub attempted_json: String,
    pub existing_json: String,
    pub diff_json: String,
    pub created_at: String,
}

impl Store {
    pub fn export_all(&self) -> Result<ExportPackage> {
        let blobs = {
            let mut stmt = self.conn.prepare(
                "SELECT id,name,bytes,sha256,created_at FROM blobs ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| {
                let bytes: Vec<u8> = r.get(2)?;
                Ok(BlobRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    bytes_b64: hex::encode(bytes),
                    sha256: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let materials = {
            let mut stmt = self.conn.prepare(
                "SELECT id,kind,seq,body_json,source_blob_id,source_name,note,created_at
                 FROM materials ORDER BY kind,seq,id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(MaterialRow {
                    id: r.get(0)?, kind: r.get(1)?, seq: r.get::<_, i64>(2)? as u32,
                    body_json: r.get(3)?, source_blob_id: r.get(4)?, source_name: r.get(5)?,
                    note: r.get(6)?, created_at: r.get(7)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let derived = {
            let mut stmt = self.conn.prepare(
                "SELECT id,kind,node_id,branch_id,body_json,created_at
                 FROM derived ORDER BY branch_id,id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(DerivedRow {
                    id: r.get(0)?, kind: r.get(1)?, node_id: r.get(2)?, branch_id: r.get(3)?,
                    body_json: r.get(4)?, created_at: r.get(5)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let branches = {
            let mut stmt = self.conn.prepare(
                "SELECT id,label,forked_from_event,adopted,created_at FROM branches ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(BranchRow {
                    id: r.get(0)?, label: r.get(1)?, forked_from_event: r.get(2)?,
                    adopted: r.get::<_, i64>(3)? != 0, created_at: r.get(4)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let events = {
            let mut stmt = self.conn.prepare(
                "SELECT id,branch_id,seq,kind,summary,basis_json,snapshot_json,parent_event_id,base_event_id,created_at
                 FROM events ORDER BY branch_id,seq",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(EventRow {
                    id: r.get(0)?, branch_id: r.get(1)?, seq: r.get::<_, i64>(2)? as u32,
                    kind: r.get(3)?, summary: r.get(4)?, basis_json: r.get(5)?,
                    snapshot_json: r.get(6)?, parent_event_id: r.get(7)?,
                    base_event_id: r.get(8)?, created_at: r.get(9)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let conflicts = {
            let mut stmt = self.conn.prepare(
                "SELECT id,branch_id,base_event_id,attempted_by,attempted_json,existing_json,diff_json,created_at
                 FROM conflicts ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(ConflictRow {
                    id: r.get(0)?, branch_id: r.get(1)?, base_event_id: r.get(2)?,
                    attempted_by: r.get(3)?, attempted_json: r.get(4)?, existing_json: r.get(5)?,
                    diff_json: r.get(6)?, created_at: r.get(7)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        Ok(ExportPackage {
            format: "flowline-v1".into(),
            schema_version: SCHEMA_VERSION,
            blobs,
            materials,
            derived,
            branches,
            events,
            conflicts,
        })
    }

    /// 导入到（通常为空的）数据库。任一表失败则整体回滚；哈希与身份必须自洽。
    pub fn import_all(&mut self, pkg: &ExportPackage) -> Result<()> {
        if pkg.format != "flowline-v1" {
            return Err(Error::Import(format!("unsupported package format {}", pkg.format)));
        }
        let tx = self.conn.transaction()?;
        for b in &pkg.blobs {
            let bytes = hex::decode(&b.bytes_b64)
                .map_err(|e| Error::Import(format!("blob {} decode: {e}", b.id)))?;
            if sha256_hex(&bytes) != b.sha256 {
                return Err(Error::Import(format!("blob {} sha mismatch", b.id)));
            }
            if crate::ids::blob_id(&bytes) != b.id {
                return Err(Error::Import(format!("blob {} identity mismatch", b.id)));
            }
            tx.execute(
                "INSERT INTO blobs(id,name,bytes,sha256,created_at) VALUES(?1,?2,?3,?4,?5)",
                params![b.id, b.name, bytes, b.sha256, b.created_at],
            )?;
        }
        for m in &pkg.materials {
            tx.execute(
                "INSERT INTO materials(id,kind,seq,body_json,source_blob_id,source_name,note,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![m.id, m.kind, m.seq, m.body_json, m.source_blob_id, m.source_name, m.note, m.created_at],
            )?;
        }
        for d in &pkg.derived {
            tx.execute(
                "INSERT INTO derived(id,kind,node_id,branch_id,body_json,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![d.id, d.kind, d.node_id, d.branch_id, d.body_json, d.created_at],
            )?;
        }
        for br in &pkg.branches {
            tx.execute(
                "INSERT INTO branches(id,label,forked_from_event,adopted,created_at)
                 VALUES(?1,?2,?3,?4,?5)",
                params![br.id, br.label, br.forked_from_event, br.adopted as i64, br.created_at],
            )?;
        }
        for e in &pkg.events {
            tx.execute(
                "INSERT INTO events(id,branch_id,seq,kind,summary,basis_json,snapshot_json,parent_event_id,base_event_id,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![e.id,e.branch_id,e.seq,e.kind,e.summary,e.basis_json,e.snapshot_json,
                        e.parent_event_id,e.base_event_id,e.created_at],
            )?;
        }
        for c in &pkg.conflicts {
            tx.execute(
                "INSERT INTO conflicts(id,branch_id,base_event_id,attempted_by,attempted_json,existing_json,diff_json,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![c.id,c.branch_id,c.base_event_id,c.attempted_by,c.attempted_json,c.existing_json,c.diff_json,c.created_at],
            )?;
        }
        tx.execute(
            "INSERT INTO schema_meta(key,value) VALUES('version',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![pkg.schema_version.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn is_empty(&self) -> Result<bool> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM materials", [], |r| r.get(0))?;
        Ok(n == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_sets_version_and_is_idempotent() {
        let mut s = Store::open_in_memory().unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
        s.migrate().unwrap();
        assert_eq!(s.schema_version().unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn optimistic_concurrency_records_conflict() {
        let mut s = Store::open_in_memory().unwrap();
        s.ensure_branch("b", "B", None).unwrap();
        let e1 = s
            .append_event("b", EventKind::ViewAction, "first", &serde_json::json!({}), &serde_json::json!({}), None, "reviewer-a")
            .unwrap();
        // 两个窗口都基于 e1：第一个成功
        s.append_event("b", EventKind::Decision, "window1", &serde_json::json!({"w":1}), &serde_json::json!({}), Some(&e1.id), "reviewer-a")
            .unwrap();
        // 第二个仍基于 e1：冲突
        let err = s
            .append_event("b", EventKind::Decision, "window2", &serde_json::json!({"w":2}), &serde_json::json!({}), Some(&e1.id), "reviewer-b")
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
        let conflicts = s.list_conflicts().unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0]["base_event_id"], serde_json::json!(e1.id));
        // 冲突事件未追加，分支头仍是 window1
        let last = s.last_event("b").unwrap().unwrap();
        assert_eq!(last.summary, "window1");
    }
}
