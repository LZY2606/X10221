//! Repository layer: cases, append-only event log, branches, refs and the
//! separately versioned entities. All writes are transactional.

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

use crate::db::Db;
use crate::id::{digest_hex, obj_id};

pub const STREAM_RAW: &str = "raw_fcs_events";
pub const STREAM_SPILL_META: &str = "spillover_meta";
pub const STREAM_SPILL_MATRIX: &str = "spill_matrix";
pub const STREAM_COMP: &str = "comp_matrix";
pub const STREAM_XFORM: &str = "transform_params";
pub const STREAM_GATES: &str = "gate_polygon";
pub const STREAM_HIER: &str = "gate_hierarchy";

pub fn stream_for(kind: &str, key: &str) -> String {
    format!("{}:{}", kind, key)
}

pub const ALL_STREAMS: &[&str] = &[
    STREAM_RAW,
    STREAM_SPILL_META,
    STREAM_SPILL_MATRIX,
    STREAM_COMP,
    STREAM_XFORM,
    STREAM_GATES,
    STREAM_HIER,
];

#[derive(Debug, Clone)]
pub struct EventRow {
    pub branch_id: String,
    pub event_id: String,
    pub seq: i64,
    pub event_type: String,
    pub payload: Value,
    pub ts: i64,
}

#[derive(Debug, Clone)]
pub struct EntityRow {
    pub id: String,
    pub stream_id: String,
    pub kind: String,
    pub version: i64,
    pub payload: Value,
}

#[derive(Debug, Clone)]
pub struct BranchRow {
    pub branch_id: String,
    pub key_name: String,
    pub parent_branch_id: Option<String>,
    pub base_event_id: Option<String>,
    pub label: String,
    pub revision: i64,
    pub archived: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResultCandidateRow {
    pub candidate_id: String,
    pub label: String,
    pub accepted: Option<bool>,
    pub value: Value,
    pub rationale: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ResultRow {
    pub result_id: String,
    pub branch_id: String,
    pub kind: String,
    pub target_key: String,
    pub status: String,
    pub value: Option<Value>,
    pub denominator_result_id: Option<String>,
    pub computed_at_event: String,
    pub stale: bool,
    pub stale_reason: Option<String>,
    pub ruling_id: Option<String>,
    pub deps: Vec<(String, String, i64)>,
    pub candidates: Vec<ResultCandidateRow>,
}

fn entity_id(stream_id: &str, version: i64, payload: &Value) -> String {
    let bytes = canonical(payload);
    obj_id(
        stream_id.split(':').next().unwrap_or("ent"),
        &[version.to_le_bytes().as_slice(), bytes.as_slice()].concat(),
    )
}

/// Deterministic JSON rendering: object keys sorted, NaN-free domain values
/// already encoded as tagged strings in `domain::Signal`.
pub fn canonical(v: &Value) -> Vec<u8> {
    let mut buf = Vec::new();
    write_canon(v, &mut buf);
    buf
}

fn write_canon(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(b.as_str().as_bytes()),
        Value::Number(n) => out.extend_from_slice(n.to_string().as_bytes()),
        Value::String(s) => {
            // JSON-escape deterministically.
            let escaped = serde_json::to_string(s).unwrap();
            out.extend_from_slice(escaped.as_bytes());
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canon(it, out);
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(serde_json::to_string(k).unwrap().as_bytes());
                out.push(b':');
                write_canon(&map[*k], out);
            }
            out.push(b'}');
        }
    }
}

impl Db {
    pub fn create_case(
        &self,
        case_id: &str,
        slug: &str,
        title: &str,
        manifest: &Value,
        root_event_id: &str,
        ts: i64,
    ) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO cases(case_id,key_slug,title,manifest_json,created_event,created_at_ts)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                case_id,
                slug,
                title,
                manifest.to_string(),
                root_event_id,
                ts
            ],
        )
        .expect("insert case");
    }

    pub fn insert_blob(&self, case_id: &str, blob_id: &str, filename: &str, media: &str, bytes: &[u8]) {
        let sha = digest_hex(&[bytes]);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO raw_blobs(case_id,blob_id,filename,media_type,bytes,sha256)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![case_id, blob_id, filename, media, bytes, sha],
        )
        .expect("insert blob");
    }

    pub fn list_case_ids(&self) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT case_id FROM cases ORDER BY key_slug").unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    pub fn get_case_meta(&self, case_id: &str) -> Option<(String, String, Value)> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT key_slug,title,manifest_json FROM cases WHERE case_id=?1",
            params![case_id],
            |r| {
                let slug: String = r.get(0)?;
                let title: String = r.get(1)?;
                let manifest: String = r.get(2)?;
                Ok((slug, title, serde_json::from_str(&manifest).unwrap()))
            },
        )
        .optional()
        .unwrap()
    }

    pub fn get_blob(&self, case_id: &str, blob_id: &str) -> Option<(String, String, Vec<u8>, String)> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT filename,media_type,bytes,sha256 FROM raw_blobs WHERE case_id=?1 AND blob_id=?2",
            params![case_id, blob_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .unwrap()
    }

    pub fn list_blobs(&self, case_id: &str) -> Vec<(String, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT blob_id,filename FROM raw_blobs WHERE case_id=?1 ORDER BY filename")
            .unwrap();
        stmt.query_map(params![case_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }
}

impl Db {
    // ---------- branches ----------

    pub fn insert_branch(
        &self,
        case_id: &str,
        branch: &BranchRow,
        ts: i64,
    ) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO branches(case_id,branch_id,key_name,parent_branch_id,base_event_id,label,revision,created_at_ts,archived)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0)",
            params![
                case_id,
                branch.branch_id,
                branch.key_name,
                branch.parent_branch_id,
                branch.base_event_id,
                branch.label,
                branch.revision,
                ts
            ],
        )
        .expect("insert branch");
    }

    pub fn get_branch(&self, case_id: &str, branch_id_or_key: &str) -> Option<BranchRow> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT branch_id,key_name,parent_branch_id,base_event_id,label,revision,archived
             FROM branches WHERE case_id=?1 AND (branch_id=?2 OR key_name=?2)",
            params![case_id, branch_id_or_key],
            |r| {
                Ok(BranchRow {
                    branch_id: r.get(0)?,
                    key_name: r.get(1)?,
                    parent_branch_id: r.get(2)?,
                    base_event_id: r.get(3)?,
                    label: r.get(4)?,
                    revision: r.get(5)?,
                    archived: r.get::<_, i64>(6)? != 0,
                })
            },
        )
        .optional()
        .unwrap()
    }

    pub fn list_branches(&self, case_id: &str) -> Vec<BranchRow> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT branch_id,key_name,parent_branch_id,base_event_id,label,revision,archived
                 FROM branches WHERE case_id=?1 ORDER BY created_at_ts, branch_id",
            )
            .unwrap();
        stmt.query_map(params![case_id], |r| {
            Ok(BranchRow {
                branch_id: r.get(0)?,
                key_name: r.get(1)?,
                parent_branch_id: r.get(2)?,
                base_event_id: r.get(3)?,
                label: r.get(4)?,
                revision: r.get(5)?,
                archived: r.get::<_, i64>(6)? != 0,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    /// Bump the branch revision; returns the new revision. Uses a conditional
    /// update so two saves against the same revision cannot both succeed.
    pub fn bump_revision(
        &self,
        case_id: &str,
        branch_id: &str,
        expected: i64,
    ) -> rusqlite::Result<i64> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let affected = tx.execute(
            "UPDATE branches SET revision=revision+1 WHERE case_id=?1 AND branch_id=?2 AND revision=?3",
            params![case_id, branch_id, expected],
        )?;
        if affected == 0 {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ffi::ErrorCode::ConstraintViolation,
                    extended_code: 0,
                },
                Some("revision conflict".into()),
            ));
        }
        let rev: i64 = tx.query_row(
            "SELECT revision FROM branches WHERE case_id=?1 AND branch_id=?2",
            params![case_id, branch_id],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(rev)
    }

    pub fn current_revision(&self, case_id: &str, branch_id: &str) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT revision FROM branches WHERE case_id=?1 AND branch_id=?2",
            params![case_id, branch_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    // ---------- event log ----------

    pub fn next_event_seq(&self, case_id: &str, branch_id: &str) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(seq)+1,0) FROM events WHERE case_id=?1 AND branch_id=?2",
            params![case_id, branch_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_event(
        &self,
        case_id: &str,
        branch_id: &str,
        event_id: &str,
        seq: i64,
        event_type: &str,
        payload: &Value,
        ts: i64,
    ) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events(case_id,branch_id,event_id,seq,event_type,payload_json,created_at_ts)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![case_id, branch_id, event_id, seq, event_type, payload.to_string(), ts],
        )
        .expect("append event");
    }

    pub fn list_events(&self, case_id: &str, branch_id: &str) -> Vec<EventRow> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT branch_id,event_id,seq,event_type,payload_json,created_at_ts
                 FROM events WHERE case_id=?1 AND branch_id=?2 ORDER BY seq",
            )
            .unwrap();
        stmt.query_map(params![case_id, branch_id], |r| {
            let payload: String = r.get(4)?;
            Ok(EventRow {
                branch_id: r.get(0)?,
                event_id: r.get(1)?,
                seq: r.get(2)?,
                event_type: r.get(3)?,
                payload: serde_json::from_str(&payload).unwrap(),
                ts: r.get(5)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    pub fn get_event(&self, case_id: &str, event_id: &str) -> Option<EventRow> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT branch_id,event_id,seq,event_type,payload_json,created_at_ts
             FROM events WHERE case_id=?1 AND event_id=?2",
            params![case_id, event_id],
            |r| {
                let payload: String = r.get(4)?;
                Ok(EventRow {
                    branch_id: r.get(0)?,
                    event_id: r.get(1)?,
                    seq: r.get(2)?,
                    event_type: r.get(3)?,
                    payload: serde_json::from_str(&payload).unwrap(),
                    ts: r.get(5)?,
                })
            },
        )
        .optional()
        .unwrap()
    }

    pub fn record_result_timeline(&self, case_id: &str, result_id: &str, event_id: &str, state: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO result_timeline(case_id,result_id,event_id,state)
             VALUES(?1,?2,?3,?4)",
            params![case_id, result_id, event_id, state],
        )
        .expect("timeline");
    }
}

impl Db {
    // ---------- versioned entities ----------

    pub fn current_entity_version(&self, case_id: &str, stream_id: &str) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(MAX(version),0) FROM entities WHERE case_id=?1 AND stream_id=?2",
            params![case_id, stream_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Insert a new entity version. Returns (id, version). Old versions stay
    /// intact and readable.
    pub fn insert_entity(
        &self,
        case_id: &str,
        stream_id: &str,
        kind: &str,
        payload: &Value,
        created_event: Option<&str>,
    ) -> (String, i64) {
        let version = self.current_entity_version(case_id, stream_id) + 1;
        let id = entity_id(stream_id, version, payload);
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO entities(case_id,id,stream_id,kind,version,payload_json,created_event)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![case_id, id, stream_id, kind, version, payload.to_string(), created_event],
        )
        .expect("insert entity");
        (id, version)
    }

    pub fn get_entity(&self, case_id: &str, entity_id: &str) -> Option<EntityRow> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT id,stream_id,kind,version,payload_json FROM entities WHERE case_id=?1 AND id=?2",
            params![case_id, entity_id],
            |r| {
                let payload: String = r.get(4)?;
                Ok(EntityRow {
                    id: r.get(0)?,
                    stream_id: r.get(1)?,
                    kind: r.get(2)?,
                    version: r.get(3)?,
                    payload: serde_json::from_str(&payload).unwrap(),
                })
            },
        )
        .optional()
        .unwrap()
    }

    pub fn list_entity_versions(&self, case_id: &str, stream_id: &str) -> Vec<EntityRow> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id,stream_id,kind,version,payload_json FROM entities
                 WHERE case_id=?1 AND stream_id=?2 ORDER BY version",
            )
            .unwrap();
        stmt.query_map(params![case_id, stream_id], |r| {
            let payload: String = r.get(4)?;
            Ok(EntityRow {
                id: r.get(0)?,
                stream_id: r.get(1)?,
                kind: r.get(2)?,
                version: r.get(3)?,
                payload: serde_json::from_str(&payload).unwrap(),
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    // ---------- branch refs (snapshot of stream tips) ----------

    pub fn set_ref(&self, case_id: &str, branch_id: &str, stream_id: &str, entity_id: &str, version: i64) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO branch_refs(case_id,branch_id,stream_id,entity_id,version)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(case_id,branch_id,stream_id) DO UPDATE SET entity_id=excluded.entity_id, version=excluded.version",
            params![case_id, branch_id, stream_id, entity_id, version],
        )
        .expect("set ref");
    }

    pub fn get_refs(&self, case_id: &str, branch_id: &str) -> Vec<(String, String, i64)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT stream_id,entity_id,version FROM branch_refs WHERE case_id=?1 AND branch_id=?2")
            .unwrap();
        stmt.query_map(params![case_id, branch_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    /// Restore refs from an event replay: only include ref_updated events with
    /// seq <= the chosen point.
    pub fn refs_at_event(
        &self,
        case_id: &str,
        branch_id: &str,
        up_to_seq: i64,
    ) -> Vec<(String, String, i64)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT payload_json FROM events
                 WHERE case_id=?1 AND branch_id=?2 AND seq<=?3 AND event_type='ref_updated'
                 ORDER BY seq",
            )
            .unwrap();
        let rows: Vec<String> = stmt
            .query_map(params![case_id, branch_id, up_to_seq], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let mut map = std::collections::BTreeMap::new();
        for row in rows {
            let v: Value = serde_json::from_str(&row).unwrap();
            map.insert(
                v["stream_id"].as_str().unwrap().to_string(),
                (
                    v["entity_id"].as_str().unwrap().to_string(),
                    v["version"].as_i64().unwrap(),
                ),
            );
        }
        map.into_iter().map(|(s, (id, ver))| (s, id, ver)).collect()
    }

    // ---------- results ----------

    #[allow(clippy::too_many_arguments)]
    pub fn insert_result(
        &self,
        case_id: &str,
        row: &ResultRow,
    ) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO results(result_id,case_id,branch_id,kind,target_key,status,value_json,
              denominator_result_id,computed_at_event,stale,stale_reason,ruling_id)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                row.result_id,
                case_id,
                row.branch_id,
                row.kind,
                row.target_key,
                row.status,
                row.value.as_ref().map(|v| v.to_string()),
                row.denominator_result_id,
                row.computed_at_event,
                row.stale as i64,
                row.stale_reason,
                row.ruling_id
            ],
        )
        .expect("insert result");
        for (stream_id, entity_id, version) in &row.deps {
            conn.execute(
                "INSERT INTO result_deps(case_id,result_id,stream_id,entity_id,version)
                 VALUES(?1,?2,?3,?4,?5)",
                params![case_id, row.result_id, stream_id, entity_id, version],
            )
            .expect("insert dep");
        }
        for c in &row.candidates {
            conn.execute(
                "INSERT INTO result_candidates(case_id,result_id,candidate_id,label,accepted,value_json,rationale)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![
                    case_id,
                    row.result_id,
                    c.candidate_id,
                    c.label,
                    c.accepted.map(|b| b as i64),
                    c.value.to_string(),
                    c.rationale
                ],
            )
            .expect("insert candidate");
        }
    }

    fn hydrate_result(conn: &Connection, case_id: &str, result_id: &str) -> Option<ResultRow> {
        conn.query_row(
            "SELECT result_id,branch_id,kind,target_key,status,value_json,denominator_result_id,
                    computed_at_event,stale,stale_reason,ruling_id
             FROM results WHERE case_id=?1 AND result_id=?2",
            params![case_id, result_id],
            |r| {
                let value: Option<String> = r.get(5)?;
                Ok(ResultRow {
                    result_id: r.get(0)?,
                    branch_id: r.get(1)?,
                    kind: r.get(2)?,
                    target_key: r.get(3)?,
                    status: r.get(4)?,
                    value: value.and_then(|s| serde_json::from_str(&s).ok()),
                    denominator_result_id: r.get(6)?,
                    computed_at_event: r.get(7)?,
                    stale: r.get::<_, i64>(8)? != 0,
                    stale_reason: r.get(9)?,
                    ruling_id: r.get(10)?,
                    deps: Vec::new(),
                    candidates: Vec::new(),
                })
            },
        )
        .optional()
        .unwrap()
        .map(|mut row| {
            {
                let mut stmt = conn
                    .prepare("SELECT stream_id,entity_id,version FROM result_deps WHERE case_id=?1 AND result_id=?2")
                    .unwrap();
                row.deps = stmt
                    .query_map(params![case_id, result_id], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
                    })
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect();
            }
            {
                let mut stmt = conn
                    .prepare("SELECT candidate_id,label,accepted,value_json,rationale FROM result_candidates WHERE case_id=?1 AND result_id=?2 ORDER BY candidate_id")
                    .unwrap();
                row.candidates = stmt
                    .query_map(params![case_id, result_id], |r| {
                        let accepted: Option<i64> = r.get(2)?;
                        let value: String = r.get(3)?;
                        Ok(ResultCandidateRow {
                            candidate_id: r.get(0)?,
                            label: r.get(1)?,
                            accepted: accepted.map(|a| a != 0),
                            value: serde_json::from_str(&value).unwrap(),
                            rationale: r.get(4)?,
                        })
                    })
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect();
            }
            row
        })
    }

    pub fn get_result(&self, case_id: &str, result_id: &str) -> Option<ResultRow> {
        let conn = self.conn.lock().unwrap();
        Self::hydrate_result(&conn, case_id, result_id)
    }

    pub fn list_results_for_branch(&self, case_id: &str, branch_id: &str) -> Vec<ResultRow> {
        let conn = self.conn.lock().unwrap();
        let ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT result_id FROM results WHERE case_id=?1 AND branch_id=?2 ORDER BY kind,target_key,result_id")
                .unwrap();
            stmt.query_map(params![case_id, branch_id], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        ids.iter()
            .filter_map(|id| Self::hydrate_result(&conn, case_id, id))
            .collect()
    }

    pub fn mark_result_stale(&self, case_id: &str, result_id: &str, reason: &str) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE results SET stale=1, stale_reason=?3 WHERE case_id=?1 AND result_id=?2 AND stale=0",
            params![case_id, result_id, reason],
        )
        .expect("mark stale");
    }

    pub fn set_candidate_ruling(
        &self,
        case_id: &str,
        result_id: &str,
        candidate_id: &str,
        accept: bool,
        ruling_event_id: &str,
    ) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE result_candidates SET accepted=?4 WHERE case_id=?1 AND result_id=?2 AND candidate_id=?3",
            params![case_id, result_id, candidate_id, accept as i64],
        )
        .expect("set candidate");
        conn.execute(
            "UPDATE results SET ruling_id=?4 WHERE case_id=?1 AND result_id=?2 AND (ruling_id IS NULL OR ruling_id='')",
            params![case_id, result_id, candidate_id, ruling_event_id],
        )
        .expect("set ruling");
    }

    pub fn list_result_ids_depending_on(
        &self,
        case_id: &str,
        branch_id: &str,
        entity_id: &str,
    ) -> Vec<String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT r.result_id FROM results r
                 JOIN result_deps d ON d.case_id=r.case_id AND d.result_id=r.result_id
                 WHERE r.case_id=?1 AND r.branch_id=?2 AND d.entity_id=?3",
            )
            .unwrap();
        stmt.query_map(params![case_id, branch_id, entity_id], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    /// Run `f` inside one transaction. Any error returned by `f` rolls the
    /// whole change set back (atomic import rule).
    pub fn with_transaction<T, E>(
        &self,
        f: impl FnOnce(&mut TransactionCtx) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut guard = self.conn.lock().unwrap();
        let tx = guard.transaction().expect("begin tx");
        let mut ctx = TransactionCtx { tx };
        match f(&mut ctx) {
            Ok(v) => {
                ctx.tx.commit().expect("commit tx");
                Ok(v)
            }
            Err(e) => {
                // `ctx` (and the transaction) is dropped here -> ROLLBACK.
                drop(ctx);
                Err(e)
            }
        }
    }
}


/// Thin wrapper exposing the SQL primitives needed by the importer inside a
/// single transaction. Kept minimal on purpose.
pub struct TransactionCtx<'a> {
    pub tx: Transaction<'a>,
}

impl TransactionCtx<'_> {
    pub fn execute(&self, sql: &str, p: &[&dyn rusqlite::ToSql]) {
        self.tx.execute(sql, p).expect("tx execute");
    }
    pub fn query_row_i64(&self, sql: &str, p: &[&dyn rusqlite::ToSql]) -> i64 {
        self.tx.query_row(sql, p, |r| r.get::<_, i64>(0)).unwrap()
    }
}

impl Db {
    // ---------- human decision ledger ----------

    pub fn record_decision(
        &self,
        case_id: &str,
        branch_id: &str,
        slot: &str,
        candidate_id: &str,
        rationale: &str,
        event_id: &str,
        seq: i64,
    ) {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO branch_decisions(case_id,branch_id,decision_slot,candidate_id,rationale,event_id,seq)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![case_id, branch_id, slot, candidate_id, rationale, event_id, seq],
        )
        .expect("record decision");
    }

    /// Latest accepted candidate per decision slot at a given point in the log.
    pub fn decisions_at(&self, case_id: &str, branch_id: &str, up_to_seq: i64) -> Vec<(String, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT decision_slot, candidate_id FROM branch_decisions
                 WHERE case_id=?1 AND branch_id=?2 AND seq<=?3
                 ORDER BY decision_slot, seq",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![case_id, branch_id, up_to_seq], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect::<Vec<_>>();
        let mut map = std::collections::BTreeMap::new();
        for (slot, cand) in rows {
            map.insert(slot, cand);
        }
        map.into_iter().collect()
    }

    pub fn list_decisions(&self, case_id: &str, branch_id: &str) -> Vec<(String, String, String)> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT decision_slot,candidate_id,rationale FROM branch_decisions
                 WHERE case_id=?1 AND branch_id=?2 ORDER BY seq",
            )
            .unwrap();
        stmt.query_map(params![case_id, branch_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }
}
