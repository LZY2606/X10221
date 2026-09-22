use serde::Serialize;

#[derive(Debug)]
pub enum FlowError {
    Sql(rusqlite::Error),
    Msg(String),
    Conflict { head_seq: i64, head_event_id: String, incoming: String, current: String },
}

impl std::fmt::Display for FlowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlowError::Sql(e) => write!(f, "sqlite: {e}"),
            FlowError::Msg(m) => write!(f, "{m}"),
            FlowError::Conflict { head_seq, .. } => write!(f, "版本冲突：当前 head_seq={head_seq}"),
        }
    }
}
impl std::error::Error for FlowError {}
impl From<rusqlite::Error> for FlowError {
    fn from(e: rusqlite::Error) -> Self { FlowError::Sql(e) }
}

#[derive(Serialize)]
pub struct ConflictBody {
    pub error: String,
    pub code: String,
    pub server_head_seq: i64,
    pub server_head_event_id: String,
    pub incoming: serde_json::Value,
    pub current: serde_json::Value,
}
