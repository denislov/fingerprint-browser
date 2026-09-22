use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("record not found: {0}")]
    NotFound(String),

    #[error("record conflict: {0}")]
    Conflict(String),

    /// A record names another record that is not stored.
    ///
    /// Its own variant rather than a [`StorageError::Database`], because a
    /// foreign key violation is the database reporting a mistake the caller can
    /// name - unlike a duplicate identifier, which it reports with the same SQLite
    /// code and which means something else entirely. Saying "conflict" about a
    /// dangling reference, or "database error" about a duplicate, sends the
    /// reader after the wrong thing.
    #[error("dangling reference: {0}")]
    Dangling(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("storage IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("storage error: {0}")]
    Other(String),
}
