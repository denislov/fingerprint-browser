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

    /// The database was written by a version of the program that knows more
    /// migrations than this one does.
    ///
    /// Refused rather than opened: this build would not know what the extra
    /// migrations mean, and writing to a schema it does not understand is how a
    /// newer database becomes an older one with its newer records still in it.
    #[error(
        "this database uses schema version {found}, and this build knows {supported}: it was written by a newer version of the program"
    )]
    SchemaTooNew { found: i64, supported: i64 },

    #[error("storage error: {0}")]
    Other(String),
}
