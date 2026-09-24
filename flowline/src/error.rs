#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    Fcs(String),
    Domain(String),
    Conflict { message: String, server: serde_json::Value, base: serde_json::Value },
    NotFound(String),
    RollbackOnly(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Sqlite(e) => write!(f, "sqlite: {e}"),
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Fcs(s) => write!(f, "fcs: {s}"),
            Error::Domain(s) => write!(f, "domain: {s}"),
            Error::Conflict { message, .. } => write!(f, "conflict: {message}"),
            Error::NotFound(s) => write!(f, "not found: {s}"),
            Error::RollbackOnly(s) => write!(f, "rolled back: {s}"),
        }
    }
}
impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self { Error::Sqlite(e) }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self { Error::Io(e) }
}

pub type Result<T> = std::result::Result<T, Error>;
