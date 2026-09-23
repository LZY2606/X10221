use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("validation: {0}")]
    Validation(String),
    #[error("rollback: {0}")]
    Rollback(String),
    #[error("{0}")]
    Other(String),
}

impl AppError {
    pub fn status(&self) -> u16 {
        match self {
            AppError::Conflict(_) => 409,
            AppError::NotFound(_) => 404,
            AppError::Validation(_) | AppError::Rollback(_) => 422,
            _ => 500,
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;
