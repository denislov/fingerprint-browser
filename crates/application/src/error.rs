use domain::ValidationError;
use runtime::RuntimeCommandError;
use storage::StorageError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Validation(#[from] ValidationError),

    #[error(transparent)]
    Runtime(#[from] RuntimeCommandError),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("application error: {0}")]
    Other(String),
}
