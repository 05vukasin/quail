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
    /// A source could not deliver one item; `locator` names it so the engine can still write
    /// a `Failed` record for that file.
    #[error("{locator}: {source}")]
    Source {
        /// The file that could not be delivered.
        locator: String,
        /// Why.
        source: Box<Error>,
    },
    /// Anything else.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Wraps an error with the locator of the item it belongs to.
    pub fn at(locator: impl Into<String>, source: Error) -> Self {
        Self::Source {
            locator: locator.into(),
            source: Box::new(source),
        }
    }

    /// The locator, if this error carries one.
    pub fn locator(&self) -> Option<&str> {
        match self {
            Self::Source { locator, .. } => Some(locator),
            _ => None,
        }
    }
}
