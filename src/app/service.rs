//! 命令服务。所有写操作：单事务 append 事件；查询：重放事件为 BranchState。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::app::commands::*;
use crate::casepack;
use crate::db;
use crate::domain::derive::derive_tree;
use crate::domain::events::{EventKind, EventRecord, Judgement};
use crate::domain::fcs::{parse_fcs, FcsData};
use crate::domain::gate::{CoordBasis, Point, PolygonGate};
use crate::domain::ids::{content_rev, content_rev_parts, stable_id};
use crate::domain::matrix::{invert_spillover, InversionStatus};
use crate::domain::model::*;
use crate::domain::project::*;
use crate::domain::transform::TransformSpec;

#[derive(Debug)]
pub struct ServiceError {
    pub code: String,
    pub message: String,
}
impl std::fmt::Display for ServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}
impl std::error::Error for ServiceError {}

pub(crate) fn err(code: &str, msg: impl Into<String>) -> ServiceError {
    ServiceError { code: code.into(), message: msg.into() }
}

pub type SrvResult<T> = Result<T, ServiceError>;

pub struct Service {
    pub(crate) conn: Mutex<Connection>,
}

impl Service {
    pub fn new(conn: Connection) -> Self {
        Service { conn: Mutex::new(conn) }
    }

    pub fn open_file(path: &str) -> SrvResult<Self> {
        let conn = db::open(path).map_err(|e| err("db", e.0))?;
        Ok(Self::new(conn))
    }

    pub fn open_memory() -> SrvResult<Self> {
        let conn = db::open_memory().map_err(|e| err("db", e.0))?;
        Ok(Self::new(conn))
    }

    // ---------- 哈希链 ----------

    fn event_hash(seq: i64, event_id: &str, prev_hash: &str, at: &str, payload: &Value) -> String {
        content_rev_parts(&[
            "flowline-event-v1",
            &seq.to_string(),
            event_id,
            prev_hash,
            at,
            &payload.to_string(),
        ])
    }

    pub(crate) fn append(
        &self,
        conn: &mut Connection,
        branch: &str,
        kind: EventKind,
        at: &str,
        payload: Value,
        judgements: Vec<Judgement>,
        expected_head: Option<&str>,
        actor: &str,
    ) -> SrvResult<db::AppendOutcome> {
        let jv = serde_json::to_value(&judgements).unwrap_or(json!([]));
        db::append_event(
            conn, branch, kind.as_str(), at, &payload, &jv, expected_head, actor,
            Self::event_hash,
        )
        .map_err(|e| {
            if e.0.starts_with("VERSION_CONFLICT") {
                err("VERSION_CONFLICT", e.0)
            } else {
                err("db", e.0)
            }
        })
    }

    pub fn state(&self, branch: &str) -> SrvResult<BranchState> {
        let conn = self.conn.lock().unwrap();
        let events = db::list_events(&conn, branch).map_err(|e| err("db", e.0))?;
        Ok(replay(branch, &events))
    }

    pub fn state_at(&self, branch: &str, seq: i64) -> SrvResult<BranchState> {
        let conn = self.conn.lock().unwrap();
        let mut events = db::list_events(&conn, branch).map_err(|e| err("db", e.0))?;
        events.retain(|e| e.seq <= seq);
        Ok(replay(branch, &events))
    }

    pub fn branches(&self) -> SrvResult<Vec<(String, i64, String)>> {
        let conn = self.conn.lock().unwrap();
        db::list_branches(&conn).map_err(|e| err("db", e.0))
    }

    pub fn events(&self, branch: &str) -> SrvResult<Vec<EventRecord>> {
        let conn = self.conn.lock().unwrap();
        db::list_events(&conn, branch).map_err(|e| err("db", e.0))
    }
}
