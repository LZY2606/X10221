use std::fmt;

#[derive(Debug)]
pub enum AppError {
    Sql(rusqlite::Error),
    Io(std::io::Error),
    Fcs(String),
    Case(String),
    Matrix(String),
    Conflict { message: String, diff: serde_json::Value },
    BadInput(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Sql(e) => write!(f, "sqlite: {e}"),
            AppError::Io(e) => write!(f, "io: {e}"),
            AppError::Fcs(s) | AppError::Case(s) | AppError::Matrix(s) | AppError::BadInput(s) => {
                write!(f, "{s}")
            }
            AppError::Conflict { message, .. } => write!(f, "version conflict: {message}"),
        }
    }
}
impl std::error::Error for AppError {}
impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::Sql(e)
    }
}
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e)
    }
}

pub type AppResult<T> = Result<T, AppError>;
