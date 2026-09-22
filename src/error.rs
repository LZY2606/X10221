#[derive(Debug, Clone)]
pub struct AppError {
    pub message: String,
    pub status: u16,
    pub code: String,
    pub detail: serde_json::Value,
}

impl AppError {
    pub fn new(message: impl Into<String>) -> Self {
        AppError { message: message.into(), status: 400, code: "error".into(), detail: serde_json::Value::Null }
    }
    pub fn conflict(status: u16, code: &str, message: impl Into<String>, detail: serde_json::Value) -> Self {
        AppError { message: message.into(), status, code: code.into(), detail }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}
impl std::error::Error for AppError {}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self { AppError::new(e.to_string()) }
}
impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self { AppError::new(e.to_string()) }
}
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self { AppError::new(e.to_string()) }
}
impl From<String> for AppError {
    fn from(e: String) -> Self { AppError::new(e) }
}
impl From<&str> for AppError {
    fn from(e: &str) -> Self { AppError::new(e) }
}

pub type AppResult<T> = Result<T, AppError>;
