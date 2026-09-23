use crate::state::{AppData, AppError, CASE_ID};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

pub fn open(path: &str) -> Result<Connection, AppError> {
    let mut conn = if path == ":memory:" {
        Connection::open_in_memory()
    } else {
        Connection::open(path)
    }
    .map_err(|error| AppError::unprocessable(error.to_string(), json!({})))?;
    conn.pragma_update(None, "foreign_keys", "on").ok();
    migrate(&mut conn)?;
    Ok(conn)
}

pub fn migrate(conn: &mut Connection) -> Result<(), AppError> {
    conn.pragma_update(None, "user_version", 0).ok();
    let version: i64 = conn.query_row("pragma user_version", [], |row| row.get(0)).unwrap_or(0);
    if version < 1 {
        conn.execute_batch(MIGRATION_1).map_err(|e| AppError::unprocessable(e.to_string(), json!({ "migration": 1 })))?;
        conn.pragma_update(None, "user_version", 1).ok();
    }
    if version < 2 {
        conn.execute_batch(MIGRATION_2).map_err(|e| AppError::unprocessable(e.to_string(), json!({ "migration": 2 })))?;
        conn.pragma_update(None, "user_version", 2).ok();
    }
    Ok(())
}

pub fn migrate_to_one_for_test(conn: &mut Connection) -> Result<(), AppError> {
    conn.execute_batch(MIGRATION_1).map_err(|e| AppError::unprocessable(e.to_string(), json!({ "migration": 1 })))?;
    conn.pragma_update(None, "user_version", 1).ok();
    Ok(())
}

const MIGRATION_1: &str = r#"
create table schema_migrations (
    version integer primary key,
    applied_at text not null
);
insert into schema_migrations(version, applied_at) values (1, 'fixed-offline-case-clock');

create table case_source_files (
    file_id text primary key,
    path text not null,
    media_type text not null,
    material_kind text not null,
    version text not null,
    content_hash text not null,
    original_representation text not null,
    ordinal integer not null
);

create table material_versions (
    material_kind text not null,
    version text not null,
    file_id text not null,
    raw_json text not null,
    content_hash text not null,
    immutable integer not null default 1,
    ordinal integer not null,
    primary key (material_kind, version)
);

create table raw_channel_values (
    stable_id text primary key,
    event_id text not null,
    channel text not null,
    value real not null,
    unit text not null,
    material_version text not null,
    representation text not null,
    ordinal integer not null
);

create table app_events (
    event_id text primary key,
    sequence integer unique not null,
    event_type text not null,
    branch text not null,
    parent_event_id text not null,
    payload_json text not null,
    judgment_json text,
    affected_json text not null,
    created_at text not null,
    ordinal integer not null
);

create table derived_objects (
    stable_id text primary key,
    derived_kind text not null,
    status text not null,
    object_json text not null,
    introduced_by_event text not null,
    invalidated_by_event text,
    ordinal integer unique not null
);

create table invalidations (
    invalidation_id text primary key,
    derived_stable_id text not null,
    from_event text not null,
    reason_code text not null,
    scope text not null,
    affected_json text not null,
    ordinal integer not null
);
"#;

const MIGRATION_2: &str = r#"
insert into schema_migrations(version, applied_at) values (2, 'fixed-offline-case-clock');

create table if not exists state_snapshots (
    branch text primary key,
    head_event_id text not null,
    state_json text not null,
    updated_at text not null
);

create table if not exists save_conflicts (
    conflict_id text primary key,
    branch text not null,
    base_event_id text not null,
    current_event_id text not null,
    attempted_operation text not null,
    client_window text,
    difference_json text not null,
    created_event_id text not null,
    ordinal integer not null
);

alter table material_versions add column branch text not null default 'baseline';
alter table derived_objects add column scope text not null default 'baseline';
alter table app_events add column machine_readable integer not null default 1;
"#;

fn array_value(conn: &Connection, sql: &str) -> Result<Vec<Value>, AppError> {
    let mut statement = conn.prepare(sql).map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    let rows = statement.query_map([], |row| {
        let raw: String = row.get(0)?;
        Ok(raw)
    }).map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    let mut values = Vec::new();
    for row in rows {
        let raw = row.map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
        values.push(serde_json::from_str(&raw).unwrap_or(json!({ "raw": raw })));
    }
    Ok(values)
}

pub fn load_data(conn: &Connection) -> Result<AppData, AppError> {
    let initialized = conn.query_row("select exists(select 1 from material_versions limit 1)", [], |row| row.get::<_, i64>(0)).unwrap_or(0) == 1;
    let source_files = array_value(conn, "select json_object('file_id',file_id,'path',path,'media_type',media_type,'material_kind',material_kind,'version',version,'sha256_like_content_hash',content_hash,'original_representation',original_representation) from case_source_files order by ordinal")?;
    let materials = array_value(conn, "select json_object('material_kind',material_kind,'version',version,'file_id',file_id,'raw_json',raw_json,'content_hash',content_hash,'immutable',immutable,'branch',branch) from material_versions order by ordinal")?;
    let raw_channels = array_value(conn, "select json_object('stable_id',stable_id,'event_id',event_id,'channel',channel,'value',value,'unit',unit,'material_version',material_version,'representation',representation) from raw_channel_values order by ordinal")?;
    let events = array_value(conn, "select json_object('event_id',event_id,'sequence',sequence,'event_type',event_type,'branch',branch,'parent_event_id',parent_event_id,'payload',json(payload_json),'judgment',json(judgment_json),'affected_objects',json(affected_json),'timestamp',created_at,'machine_readable',machine_readable) from app_events order by ordinal")?;
    let derived = array_value(conn, "select object_json from derived_objects order by ordinal")?;
    let invalidations = array_value(conn, "select json_object('invalidation_id',invalidation_id,'derived_stable_id',derived_stable_id,'from_event',from_event,'reason_code',reason_code,'scope',scope,'affected_dependencies',json(affected_json)) from invalidations order by ordinal")?;
    let conflicts = array_value(conn, "select json_object('conflict_id',conflict_id,'branch',branch,'base_event_id',base_event_id,'current_event_id',current_event_id,'attempted_operation',attempted_operation,'client_window',client_window,'difference',json(difference_json),'created_event_id',created_event_id) from save_conflicts order by ordinal")?;
    Ok(AppData {
        case_id: CASE_ID.into(),
        initialized,
        source_files,
        materials,
        raw_channels,
        events,
        conflicts,
        derived,
        invalidations,
    })
}

pub fn persist_all(conn: &Connection, data: &AppData) -> Result<(), AppError> {
    conn.execute_batch("delete from save_conflicts; delete from invalidations; delete from derived_objects; delete from app_events; delete from raw_channel_values; delete from material_versions; delete from case_source_files; delete from state_snapshots;")
        .map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    insert_data(conn, data)
}

pub fn append_data(conn: &mut Connection, data: &AppData) -> Result<(), AppError> {
    let tx = conn.transaction().map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    insert_data(&tx, data)?;
    tx.commit().map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    Ok(())
}

pub fn insert_data(conn: &Connection, data: &AppData) -> Result<(), AppError> {
    for (ordinal, file) in data.source_files.iter().enumerate() {
        conn.execute(
            "insert or replace into case_source_files(file_id,path,media_type,material_kind,version,content_hash,original_representation,ordinal) values (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                file["file_id"], file["path"], file["media_type"], file["material_kind"], file["version"],
                file["sha256_like_content_hash"], file["original_representation"], ordinal as i64
            ],
        ).map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    }
    for (ordinal, item) in data.materials.iter().enumerate() {
        conn.execute(
            "insert or replace into material_versions(material_kind,version,file_id,raw_json,content_hash,immutable,ordinal,branch) values (?1,?2,?3,?4,?5,?6,?7,coalesce(?8,'baseline'))",
            rusqlite::params![
                item["material_kind"], item["version"], item["file_id"], item["raw_json"], item["content_hash"],
                item["immutable"].as_i64().unwrap_or(1), ordinal as i64, item.get("branch").and_then(|v| v.as_str())
            ],
        ).map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    }
    for (ordinal, item) in data.raw_channels.iter().enumerate() {
        conn.execute(
            "insert or replace into raw_channel_values(stable_id,event_id,channel,value,unit,material_version,representation,ordinal) values (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                item["stable_id"], item["event_id"], item["channel"], item["value"], item["unit"],
                item["material_version"], item["representation"], ordinal as i64
            ],
        ).map_err(|e| AppError::unprocessable(e.to_string(), json!({ "stable_id": item["stable_id"] })))?;
    }
    for (ordinal, event) in data.events.iter().enumerate() {
        let judgment = event.get("judgment").filter(|value| !value.is_null()).map(|value| value.to_string());
        conn.execute(
            "insert or replace into app_events(event_id,sequence,event_type,branch,parent_event_id,payload_json,judgment_json,affected_json,created_at,ordinal,machine_readable) values (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![
                event["event_id"], event["sequence"], event["event_type"], event["branch"], event["parent_event_id"],
                event["payload"].to_string(), judgment, event["affected_objects"].to_string(), event["timestamp"],
                ordinal as i64, event["machine_readable"].as_i64().unwrap_or(1)
            ],
        ).map_err(|e| AppError::unprocessable(e.to_string(), json!({ "event_id": event["event_id"] })))?;
    }
    for (ordinal, object) in data.derived.iter().enumerate() {
        conn.execute(
            "insert or replace into derived_objects(stable_id,derived_kind,status,object_json,introduced_by_event,invalidated_by_event,ordinal,scope) values (?1,?2,?3,?4,?5,?6,?7,coalesce(?8,'baseline'))",
            rusqlite::params![
                object["stable_id"], object["derived_kind"], object["status"], object.to_string(),
                object["introduced_by_event"], object.get("invalidated_by_event").and_then(|v| v.as_str()),
                ordinal as i64, object.get("scope").and_then(|v| v.as_str())
            ],
        ).map_err(|e| AppError::unprocessable(e.to_string(), json!({ "stable_id": object["stable_id"] })))?;
    }
    for (ordinal, item) in data.invalidations.iter().enumerate() {
        conn.execute(
            "insert or replace into invalidations(invalidation_id,derived_stable_id,from_event,reason_code,scope,affected_json,ordinal) values (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                item["invalidation_id"], item["derived_stable_id"], item["from_event"], item["reason_code"],
                item["scope"], item["affected_dependencies"].to_string(), ordinal as i64
            ],
        ).map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    }
    for (ordinal, item) in data.conflicts.iter().enumerate() {
        conn.execute(
            "insert or replace into save_conflicts(conflict_id,branch,base_event_id,current_event_id,attempted_operation,client_window,difference_json,created_event_id,ordinal) values (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                item["conflict_id"], item["branch"], item["base_event_id"], item["current_event_id"], item["attempted_operation"],
                item["client_window"], item["difference"].to_string(), item["created_event_id"], ordinal as i64
            ],
        ).map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    }
    let head = data.events.last().and_then(|event| event["event_id"].as_str()).unwrap_or("event-000");
    conn.execute(
        "insert or replace into state_snapshots(branch,head_event_id,state_json,updated_at) values ('workspace',?1,?2,?3)",
        rusqlite::params![head, serde_json::to_string(data).unwrap_or_default(), "fixed-offline-case-clock"],
    ).map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    Ok(())
}

#[derive(serde::Deserialize)]
struct Archive {
    format: String,
    data: AppData,
}

pub fn export_archive(conn: &Connection) -> Result<String, AppError> {
    let data = load_data(conn)?;
    Ok(serde_json::to_string_pretty(&json!({
        "format": "flow-boundary-chamber-sqlite-archive/v1",
        "exported_at": "fixed-offline-case-clock",
        "data": data
    })).unwrap_or_default())
}

pub fn import_archive(conn: &mut Connection, body: &str) -> Result<(), AppError> {
    let archive: Archive = serde_json::from_str(body).map_err(|error| AppError::unprocessable(
        "archive is not a valid flow boundary chamber SQLite archive",
        json!({ "diagnostic": error.to_string() }),
    ))?;
    if archive.format != "flow-boundary-chamber-sqlite-archive/v1" || archive.data.case_id != CASE_ID {
        return Err(AppError::unprocessable("archive format or case identity does not match", json!({})));
    }
    let tx = conn.transaction().map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    tx.execute_batch("delete from save_conflicts; delete from state_snapshots; delete from invalidations; delete from derived_objects; delete from app_events; delete from raw_channel_values; delete from material_versions; delete from case_source_files;")
        .map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    insert_data(&tx, &archive.data)?;
    tx.commit().map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    Ok(())
}

pub fn reset_empty(conn: &mut Connection) -> Result<(), AppError> {
    let tx = conn.transaction().map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    tx.execute_batch("delete from save_conflicts; delete from state_snapshots; delete from invalidations; delete from derived_objects; delete from app_events; delete from raw_channel_values; delete from material_versions; delete from case_source_files;")
        .map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    tx.commit().map_err(|e| AppError::unprocessable(e.to_string(), json!({})))?;
    Ok(())
}

pub fn current_event_id(conn: &Connection) -> Option<String> {
    conn.query_row("select event_id from app_events order by sequence desc limit 1", [], |row| row.get(0)).optional().flatten()
}
