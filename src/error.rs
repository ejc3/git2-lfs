//! Error types for git2-lfs operations.

use thiserror::Error;

/// Result type for git2-lfs operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur during LFS operations.
#[derive(Error, Debug)]
pub enum Error {
    /// Invalid LFS pointer format
    #[error("invalid LFS pointer: {0}")]
    InvalidPointer(String),

    /// OID parsing error
    #[error("invalid OID: {0}")]
    InvalidOid(String),

    /// HTTP request failed
    #[error("HTTP error: {0}")]
    Http(String),

    /// Server returned an error
    #[error("LFS server error: {message} (code: {code})")]
    ServerError { code: u16, message: String },

    /// Object not found on server
    #[error("object not found: {0}")]
    NotFound(String),

    /// Authentication required
    #[error("authentication required")]
    AuthRequired,

    /// Invalid URL
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization error
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// URL parsing error
    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    /// Git operation error
    #[cfg(feature = "git2-integration")]
    #[error("Git error: {0}")]
    Git(String),
}

impl From<ureq::Error> for Error {
    fn from(err: ureq::Error) -> Self {
        match err {
            ureq::Error::Status(401, _) | ureq::Error::Status(403, _) => Error::AuthRequired,
            ureq::Error::Status(404, _) => Error::NotFound("object not found".into()),
            ureq::Error::Status(code, response) => {
                let message = response
                    .into_string()
                    .unwrap_or_else(|_| "unknown error".into());
                Error::ServerError { code, message }
            }
            other => Error::Http(other.to_string()),
        }
    }
}
