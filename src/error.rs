use std::fmt;

#[derive(Debug)]
pub enum AppError {
    Db(rusqlite::Error),
    Json(serde_json::Error),
    Io(std::io::Error),
    BadRequest(String),
    NotFound(String),
    Conflict(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Db(e) => write!(f, "database error: {e}"),
            AppError::Json(e) => write!(f, "json error: {e}"),
            AppError::Io(e) => write!(f, "io error: {e}"),
            AppError::BadRequest(m) => write!(f, "{m}"),
            AppError::NotFound(m) => write!(f, "{m}"),
            AppError::Conflict(m) => write!(f, "{m}"),
        }
    }
}
impl std::error::Error for AppError {}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Db(e)
    }
}
impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::Json(e)
    }
}
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        use axum::http::StatusCode;
        let (status, code) = match &self {
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "version_conflict"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        let body = serde_json::json!({"error": code, "message": self.to_string()});
        (status, axum::Json(body)).into_response()
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
