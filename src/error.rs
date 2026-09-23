use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("版本冲突：{0}")]
    Conflict(String),
    #[error("未找到：{0}")]
    NotFound(String),
    #[error("近奇异或奇异补偿矩阵：{0}")]
    NearSingular(String),
    #[error("存储错误：{0}")]
    Db(#[from] rusqlite::Error),
    #[error("数据错误：{0}")]
    Serde(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "version_conflict"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::NearSingular(_) => (StatusCode::UNPROCESSABLE_ENTITY, "near_singular_matrix"),
            AppError::Db(_) | AppError::Serde(_) | AppError::Other(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal")
            }
        };
        let body = Json(json!({
            "error": self.to_string(),
            "code": code,
        }));
        (status, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
