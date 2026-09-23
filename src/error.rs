use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("fcs parse: {0}")]
    Fcs(String),
    #[error("domain: {0}")]
    Domain(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict at base revision {base}: tip is {tip}")]
    Conflict { base: i64, tip: i64 },
    #[error("import rejected (rolled back): {0}")]
    Import(String),
}

pub type Result<T> = std::result::Result<T, Error>;
