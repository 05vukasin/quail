use crate::format::Format;

/// Result with quail's error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Why a step failed. Every variant ends one step for one file, never the process.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Reading failed.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes are not valid for the format.
    #[error("malformed {format:?}: {msg}")]
    Malformed {
        /// Format being parsed.
        format: Format,
        /// What is wrong.
        msg: String,
    },
    /// The file is encrypted and cannot be read without a password.
    #[error("encrypted")]
    Encrypted,
    /// A feature of the format that is not supported.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A [`Limits`](crate::Limits) value was hit.
    #[error("limit exceeded: {0}")]
    LimitExceeded(&'static str),
    /// The file's time budget ran out.
    #[error("timeout")]
    Timeout,
    /// A panic was caught; the message is kept for the record.
    #[error("panic: {0}")]
    Panic(String),
    /// A model or resource could not be loaded.
    #[error("resource: {0}")]
    Resource(String),
    /// Anything else.
    #[error("{0}")]
    Other(String),
}
