//! 版本化写入的服务封装：乐观并发 + 冲突事件。

use crate::domain::{EntityKind, EntityVersion, EventPayload};
use crate::store::repo::{err, StoreError, StoreResult};
use crate::store::versions::ProposedVersion;
use crate::store::Repo;

pub struct CommitVersion {
    pub kind: EntityKind,
    pub entity_id: String,
    pub data: serde_json::Value,
    pub source_material_id: Option<String>,
    pub note: String,
    pub expected_version: i64,
}

#[derive(Debug, serde::Serialize)]
pub struct ConflictBody {
    pub code: String,
    pub kind: String,
    pub entity_id: String,
    pub expected_version: i64,
    pub actual_version: i64,
    pub incoming: serde_json::Value,
    pub current: serde_json::Value,
}

impl Repo {
    /// 提交一个版本。乐观锁失败时先落一条 ConflictDetected 事件，再返回 409 数据。
    pub fn commit_version(
        &mut self,
        branch_id: &str,
        actor: &str,
        req: CommitVersion,
    ) -> StoreResult<EntityVersion> {
        let tx = self.conn.transaction()?;
        let actual = current_in_tx(&tx, branch_id, req.kind, &req.entity_id);
        if actual != req.expected_version {
            // 冲突事件落库（该事件本身提交），本次修改回滚。
            Self::append_event_tx(
                &tx,
                branch_id,
                actor,
                &EventPayload::ConflictDetected {
                    kind: req.kind.as_str().into(),
                    entity_id: req.entity_id.clone(),
                    expected_version: req.expected_version,
                    actual_version: actual,
                },
            )?;
            tx.commit()?;
            return Err(StoreError {
                code: "VERSION_CONFLICT".into(),
                message: serde_json::to_string(&ConflictBody {
                    code: "VERSION_CONFLICT".into(),
                    kind: req.kind.as_str().into(),
                    entity_id: req.entity_id.clone(),
                    expected_version: req.expected_version,
                    actual_version: actual,
                    incoming: req.data.clone(),
                    current: self
                        .current_version(branch_id, req.kind, &req.entity_id)
                        .ok()
                        .flatten()
                        .map(|v| v.data)
                        .unwrap_or(serde_json::Value::Null),
                })
                .unwrap(),
            });
        }
        let v = Self::add_version_tx(
            &tx,
            branch_id,
            ProposedVersion {
                kind: req.kind,
                entity_id: &req.entity_id,
                data: &req.data,
                source_material_id: req.source_material_id.as_deref(),
                note: &req.note,
                expected_version: req.expected_version,
                actor,
            },
        )?;
        tx.commit()?;
        Ok(v)
    }
}

fn current_in_tx(
    tx: &rusqlite::Transaction,
    branch_id: &str,
    kind: EntityKind,
    entity_id: &str,
) -> i64 {
    use rusqlite::OptionalExtension;
    tx.query_row(
        "SELECT MAX(version) FROM entity_versions WHERE branch_id=?1 AND kind=?2 AND entity_id=?3",
        rusqlite::params![branch_id, kind.as_str(), entity_id],
        |r| r.get::<_, Option<i64>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
    .unwrap_or(0)
}

pub fn parse_conflict(e: &StoreError) -> Option<ConflictBody> {
    if e.code == "VERSION_CONFLICT" {
        serde_json::from_str(&e.message).ok()
    } else {
        None
    }
}

#[allow(dead_code)]
fn _e(_: &StoreError) -> Option<()> {
    let _ = err;
    None
}
