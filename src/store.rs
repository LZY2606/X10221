//! SQLite 存储层。事件只追加；任何导入在一个事务内完成，失败整体回滚。
use crate::domain::model::{DomainEvent, Envelope};
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Mutex;

#[derive(Debug)]
pub enum StoreError {
    Sql(rusqlite::Error),
    Conflict { expected: u64, actual: u64 },
    NotFound(String),
}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sql(e)
    }
}
impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Sql(e) => write!(f, "数据库错误: {e}"),
            StoreError::Conflict { expected, actual } => write!(
                f,
                "版本冲突：期望在 #{expected} 之后提交，当前头已到 #{actual}"
            ),
            StoreError::NotFound(s) => write!(f, "未找到: {s}"),
        }
    }
}
impl std::error::Error for StoreError {}

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../migrations/0001_init.sql")),
    ("0002_case_metadata", include_str!("../migrations/0002_case_metadata.sql")),
];

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &str) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let store = Store {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn migrate(&self) -> Result<u32, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT OR IGNORE INTO schema_meta(key,value) VALUES ('schema_version','0');",
        )?;
        let mut current: u32 = conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key='schema_version'",
                [],
                |r| r.get::<_, String>(0),
            )?
            .parse()
            .unwrap_or(0);
        let mut started = current;
        for (i, (_name, sql)) in MIGRATIONS.iter().enumerate() {
            let version = (i + 1) as u32;
            if version <= current {
                continue;
            }
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "UPDATE schema_meta SET value=?1 WHERE key='schema_version'",
                params![version.to_string()],
            )?;
            tx.commit()?;
            started = version;
            current = version;
        }
        Ok(started)
    }

    pub fn schema_version(&self) -> Result<u32, StoreError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT value FROM schema_meta WHERE key='schema_version'",
                [],
                |r| r.get::<_, String>(0),
            )?
            .parse()
            .unwrap_or(0))
    }

    pub fn create_project(&self, project_id: &str, title: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO projects(project_id, title) VALUES (?1, ?2)",
            params![project_id, title],
        )?;
        Ok(())
    }

    pub fn project_exists(&self, project_id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT 1 FROM projects WHERE project_id=?1",
                params![project_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn list_projects(&self) -> Result<Vec<(String, String, u64, Option<String>)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT project_id, title, head_seq, case_id FROM projects ORDER BY created_at, project_id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn head_seq(&self, project_id: &str) -> Result<u64, StoreError> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT head_seq FROM projects WHERE project_id=?1",
            params![project_id],
            |r| r.get(0),
        )?)
    }

    /// 追加事件 + 可选原始字节，全部在一个事务内；
    /// expected_head 不匹配时事务回滚并返回 Conflict（绝不留下半份记录）。
    pub fn append(
        &self,
        project_id: &str,
        expected_head: Option<u64>,
        env: &Envelope,
        blob: Option<(&str, &[u8])>,
    ) -> Result<u64, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let current: u64 = tx.query_row(
            "SELECT head_seq FROM projects WHERE project_id=?1",
            params![project_id],
            |r| r.get(0),
        )?;
        if let Some(exp) = expected_head {
            if exp != current {
                return Err(StoreError::Conflict {
                    expected: exp,
                    actual: current,
                });
            }
        }
        let seq = current + 1;
        let payload = serde_json::to_string(&env.payload).expect("事件可序列化");
        tx.execute(
            "INSERT INTO events(seq, project_id, branch, parent_branch, branch_start_seq,
                                event_id, kind, payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                seq,
                project_id,
                env.branch,
                env.parent_branch,
                env.branch_start_seq,
                env.event_id,
                env.payload.kind(),
                payload
            ],
        )?;
        if let Some((source_id, bytes)) = blob {
            let sha = {
                use sha2::{Digest, Sha256};
                format!("{:x}", Sha256::digest(bytes))
            };
            tx.execute(
                "INSERT OR REPLACE INTO sources_blobs(project_id, source_id, sha256, bytes)
                 VALUES (?1,?2,?3,?4)",
                params![project_id, source_id, sha, bytes],
            )?;
        }
        tx.execute(
            "UPDATE projects SET head_seq=?1 WHERE project_id=?2",
            params![seq, project_id],
        )?;
        tx.commit()?;
        Ok(seq)
    }

    pub fn set_case(&self, project_id: &str, case_id: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE projects SET case_id=?1 WHERE project_id=?2",
            params![case_id, project_id],
        )?;
        Ok(())
    }

    pub fn load_events(&self, project_id: &str) -> Result<Vec<Envelope>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT seq, branch, parent_branch, branch_start_seq, event_id, payload_json
             FROM events WHERE project_id=?1 ORDER BY seq",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| {
                let seq: u64 = r.get(0)?;
                let branch: String = r.get(1)?;
                let parent_branch: Option<String> = r.get(2)?;
                let branch_start_seq: u64 = r.get(3)?;
                let event_id: String = r.get(4)?;
                let payload_json: String = r.get(5)?;
                Ok((seq, branch, parent_branch, branch_start_seq, event_id, payload_json))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for (seq, branch, parent_branch, branch_start_seq, event_id, payload_json) in rows {
            let payload: DomainEvent = serde_json::from_str(&payload_json)
                .map_err(|e| StoreError::NotFound(format!("事件 #{seq} 反序列化失败: {e}")))?;
            out.push(Envelope {
                seq,
                branch,
                parent_branch,
                branch_start_seq,
                event_id,
                payload,
            });
        }
        Ok(out)
    }

    pub fn load_blob(&self, project_id: &str, source_id: &str) -> Result<Vec<u8>, StoreError> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT bytes FROM sources_blobs WHERE project_id=?1 AND source_id=?2",
            params![project_id, source_id],
            |r| r.get(0),
        )
        .map_err(|_| StoreError::NotFound(format!("原始字节 {source_id}")))
    }

    pub fn list_blobs(&self, project_id: &str) -> Result<Vec<(String, String, usize)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT source_id, sha256, length(bytes) FROM sources_blobs WHERE project_id=?1",
        )?;
        let rows = stmt
            .query_map(params![project_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

/// 导出/导入用整库快照
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Bundle {
    pub format: String,
    pub exported_from: String,
    pub projects: Vec<ProjectBundle>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProjectBundle {
    pub project_id: String,
    pub title: String,
    pub head_seq: u64,
    pub case_id: Option<String>,
    pub events: Vec<Envelope>,
    pub blobs: Vec<BlobEntry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct BlobEntry {
    pub source_id: String,
    pub sha256: String,
    #[serde(with = "base64ish")]
    pub bytes: Vec<u8>,
}

mod base64ish {
    use serde::{Deserialize, Deserializer, Serializer};
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    pub fn serialize<S: Serializer>(v: &Vec<u8>, s: S) -> Result<S::Ok, S::Error> {
        let mut out = String::with_capacity((v.len() * 4 + 2) / 3);
        for chunk in v.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = *chunk.get(1).unwrap_or(&0) as u32;
            let b2 = *chunk.get(2).unwrap_or(&0) as u32;
            let n = ((b0 << 16) | (b1 << 8) | b2) as u32;
            out.push(T[(n >> 18) as usize] as char);
            out.push(T[((n >> 12) & 63) as usize] as char);
            if chunk.len() > 1 {
                out.push(T[((n >> 6) & 63) as usize] as char);
            } else {
                out.push('=');
            }
            if chunk.len() > 2 {
                out.push(T[(n & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
        s.serialize_str(&out)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        let vals: Vec<Option<u8>> = s
            .bytes()
            .map(|b| match b {
                b'A'..=b'Z' => Some(b - b'A'),
                b'a'..=b'z' => Some(b - b'a' + 26),
                b'0'..=b'9' => Some(b - b'0' + 52),
                b'+' => Some(62),
                b'/' => Some(63),
                b'=' => None,
                _ => None,
            })
            .collect();
        let mut out = Vec::new();
        for chunk in vals.chunks(4) {
            let c: Vec<u8> = chunk.iter().filter_map(|x| *x).collect();
            if c.len() < 2 {
                break;
            }
            let n = ((c[0] as u32) << 18)
                | ((c.get(1).copied().unwrap_or(0) as u32) << 12)
                | ((c.get(2).copied().unwrap_or(0) as u32) << 6)
                | c.get(3).copied().unwrap_or(0) as u32;
            out.push((n >> 16) as u8);
            if c.len() > 2 {
                out.push((n >> 8) as u8);
            }
            if c.len() > 3 {
                out.push(n as u8);
            }
        }
        Ok(out)
    }
}

impl Store {
    pub fn export_bundle(&self, exported_from: &str) -> Result<Bundle, StoreError> {
        let mut projects = Vec::new();
        for (project_id, title, head_seq, case_id) in self.list_projects()? {
            let events = self.load_events(&project_id)?;
            let mut blobs = Vec::new();
            for (source_id, sha256, _len) in self.list_blobs(&project_id)? {
                let bytes = self.load_blob(&project_id, &source_id)?;
                blobs.push(BlobEntry { source_id, sha256, bytes });
            }
            projects.push(ProjectBundle {
                project_id,
                title,
                head_seq,
                case_id,
                events,
                blobs,
            });
        }
        Ok(Bundle {
            format: "flowlineage-bundle-v1".into(),
            exported_from: exported_from.into(),
            projects,
        })
    }

    /// 导入到一个全新空库；任一项目出错整体回滚（先建后删不暴露半成品）。
    pub fn import_bundle(conn_path: &str, bundle: &Bundle) -> Result<(), StoreError> {
        let store = Store::open(conn_path)?;
        for p in &bundle.projects {
            if store.project_exists(&p.project_id)? {
                return Err(StoreError::NotFound(format!(
                    "导入目标已存在项目 {}，请使用空数据库",
                    p.project_id
                )));
            }
        }
        for p in &bundle.projects {
            store.create_project(&p.project_id, &p.title)?;
            for env in &p.events {
                store
                    .conn
                    .lock()
                    .unwrap()
                    .execute(
                        "INSERT INTO events(seq, project_id, branch, parent_branch, branch_start_seq,
                                            event_id, kind, payload_json)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                        params![
                            env.seq,
                            p.project_id,
                            env.branch,
                            env.parent_branch,
                            env.branch_start_seq,
                            env.event_id,
                            env.payload.kind(),
                            serde_json::to_string(&env.payload).unwrap()
                        ],
                    )
                    .map_err(StoreError::Sql)?;
            }
            for b in &p.blobs {
                store
                    .conn
                    .lock()
                    .unwrap()
                    .execute(
                        "INSERT INTO sources_blobs(project_id, source_id, sha256, bytes)
                         VALUES (?1,?2,?3,?4)",
                        params![p.project_id, b.source_id, b.sha256, b.bytes],
                    )
                    .map_err(StoreError::Sql)?;
            }
            store.conn.lock().unwrap().execute(
                "UPDATE projects SET head_seq=?1, case_id=?2 WHERE project_id=?3",
                params![p.head_seq, p.case_id, p.project_id],
            )?;
        }
        Ok(())
    }
}
