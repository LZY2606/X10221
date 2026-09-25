#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("fcs parse: {0}")]
    Fcs(String),
    #[error("import rollback: {0}")]
    Import(String),
    #[error("version conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl axum::response::IntoResponse for Error {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        let (status, msg) = match &self {
            Error::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            Error::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            Error::BadRequest(m) | Error::Fcs(m) | Error::Import(m) => {
                (StatusCode::BAD_REQUEST, m.clone())
            }
            other => (StatusCode::INTERNAL_SERVER_ERROR, other.to_string()),
        };
        (status, axum::Json(serde_json::json!({"error": msg}))).into_response()
    }
}
