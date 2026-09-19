use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("record not found: {0}")]
    NotFound(String),

    #[error("record conflict: {0}")]
    Conflict(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("storage IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("storage error: {0}")]
    Other(String),
}
