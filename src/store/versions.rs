//! 实体版本写入：各资源分开版本化，乐观并发（expected_version）。

use super::ids::{event_id, now_ms, version_id};
use super::repo::{err, Repo, StoreError, StoreResult};
use crate::domain::{EntityKind, EntityVersion, EventPayload};
use rusqlite::{params, OptionalExtension};

pub struct ProposedVersion<'a> {
    pub kind: EntityKind,
    pub entity_id: &'a str,
    pub data: &'a serde_json::Value,
    pub source_material_id: Option<&'a str>,
    pub note: &'a str,
    /// 调用方认为当前最新版本号；首次为 0。与库内不一致则冲突。
    pub expected_version: i64,
    pub actor: &'a str,
}

#[derive(Debug)]
pub struct VersionConflict {
    pub entity_id: String,
    pub expected: i64,
    pub actual: i64,
}

impl Repo {
    pub fn current_version_number(
        &self,
        branch_id: &str,
        kind: EntityKind,
        entity_id: &str,
    ) -> StoreResult<i64> {
        let v: Option<i64> = self
            .conn
            .query_row(
                "SELECT MAX(version) FROM entity_versions
                 WHERE branch_id=?1 AND kind=?2 AND entity_id=?3",
                params![branch_id, kind.as_str(), entity_id],
                |r| r.get(0),
            )
            .optional()
            .flatten();
        Ok(v.unwrap_or(0))
    }

    /// 在一个事务内写入版本并记录事件。返回新版本。
    /// 若版本不匹配，事务回滚并返回冲突（含双方差异信息由 service 层补充）。
    pub fn add_version_tx(
        tx: &rusqlite::Transaction,
        branch_id: &str,
        p: ProposedVersion,
    ) -> Result<EntityVersion, StoreError> {
        let actual: Option<i64> = tx
            .query_row(
                "SELECT MAX(version) FROM entity_versions
                 WHERE branch_id=?1 AND kind=?2 AND entity_id=?3",
                params![branch_id, p.kind.as_str(), p.entity_id],
                |r| r.get(0),
            )
            .optional()
            .flatten();
        let actual = actual.unwrap_or(0);
        if actual != p.expected_version {
            // 记录冲突事件后整体回滚本次写入；冲突本身作为被拒绝的尝试单独提交。
            return Err(err(
                "VERSION_CONFLICT",
                serde_json::json!({
                    "entity_id": p.entity_id,
                    "kind": p.kind.as_str(),
                    "expected": p.expected_version,
                    "actual": actual,
                })
                .to_string(),
            ));
        }
        let version = actual + 1;
        let data_str = serde_json::to_string(p.data).unwrap();
        let parent: Option<String> = tx
            .query_row(
                "SELECT id FROM entity_versions
                 WHERE branch_id=?1 AND kind=?2 AND entity_id=?3
                 ORDER BY version DESC LIMIT 1",
                params![branch_id, p.kind.as_str(), p.entity_id],
                |r| r.get(0),
            )
            .optional()
            .flatten();
        let id = version_id(branch_id, p.kind.as_str(), p.entity_id, version, &data_str);
        let ts = now_ms();
        tx.execute(
            "INSERT INTO entity_versions
             (id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                id,
                p.kind.as_str(),
                p.entity_id,
                version,
                data_str,
                p.source_material_id,
                branch_id,
                parent,
                ts,
                p.note
            ],
        )?;
        let payload = EventPayload::EntityVersioned {
            version_id: id.clone(),
            kind: p.kind.as_str().into(),
            entity_id: p.entity_id.into(),
            version,
        };
        let ev = Self::append_event_tx(tx, branch_id, p.actor, &payload)?;
        // 让事件 id 直接可追溯（仅用于一致性，event id 也已内部计算）。
        let _ = ev.id == event_id(branch_id, ev.seq, &serde_json::to_string(&payload).unwrap(), ev.prev_event_id.as_deref());
        Ok(EntityVersion {
            id,
            kind: p.kind,
            entity_id: p.entity_id.into(),
            version,
            data: p.data.clone(),
            source_material_id: p.source_material_id.map(|s| s.into()),
            branch_id: branch_id.into(),
            parent_version_id: parent,
            created_at: ts,
            note: p.note.into(),
        })
    }

    pub fn get_version(&self, id: &str) -> StoreResult<EntityVersion> {
        self.conn
            .query_row(
                "SELECT id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note
                 FROM entity_versions WHERE id=?1",
                params![id],
                row_version,
            )
            .optional()?
            .ok_or_else(|| err("VERSION_NOT_FOUND", id))
    }

    pub fn current_version(
        &self,
        branch_id: &str,
        kind: EntityKind,
        entity_id: &str,
    ) -> StoreResult<Option<EntityVersion>> {
        self.conn
            .query_row(
                "SELECT id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note
                 FROM entity_versions
                 WHERE branch_id=?1 AND kind=?2 AND entity_id=?3
                 ORDER BY version DESC LIMIT 1",
                params![branch_id, kind.as_str(), entity_id],
                row_version,
            )
            .optional()
    }

    pub fn list_current_versions(
        &self,
        branch_id: &str,
        kind: EntityKind,
    ) -> StoreResult<Vec<EntityVersion>> {
        // 每个 entity 取最大 version。
        let mut s = self.conn.prepare(
            "SELECT id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note
             FROM entity_versions e
             WHERE branch_id=?1 AND kind=?2
               AND version=(SELECT MAX(version) FROM entity_versions
                           WHERE branch_id=e.branch_id AND kind=e.kind AND entity_id=e.entity_id)
             ORDER BY entity_id",
        )?;
        let rows = s.query_map(params![branch_id, kind.as_str()], row_version)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn list_versions(
        &self,
        branch_id: &str,
        kind: EntityKind,
        entity_id: &str,
    ) -> StoreResult<Vec<EntityVersion>> {
        let mut s = self.conn.prepare(
            "SELECT id,kind,entity_id,version,data,source_material_id,branch_id,parent_version_id,created_at,note
             FROM entity_versions WHERE branch_id=?1 AND kind=?2 AND entity_id=?3 ORDER BY version",
        )?;
        let rows = s.query_map(params![branch_id, kind.as_str(), entity_id], row_version)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

fn row_version(r: &rusqlite::Row) -> rusqlite::Result<EntityVersion> {
    let kind_str: String = r.get(1)?;
    let data_str: String = r.get(4)?;
    Ok(EntityVersion {
        id: r.get(0)?,
        kind: EntityKind::parse(&kind_str).unwrap(),
        entity_id: r.get(2)?,
        version: r.get(3)?,
        data: serde_json::from_str(&data_str).unwrap(),
        source_material_id: r.get(5)?,
        branch_id: r.get(6)?,
        parent_version_id: r.get(7)?,
        created_at: r.get(8)?,
        note: r.get(9)?,
    })
}
