//! Error handling and JSON error output.
//!
//! # Error Variant Usage Patterns
//!
//! - **`Message`**: Use for errors with no underlying cause, typically validation
//!   failures or missing data. Example: `UpkeepError::message(ErrorCode::InvalidData, "no root package found")`
//!
//! - **`Context`**: Use when wrapping another error with additional context.
//!   Example: `UpkeepError::context(ErrorCode::Metadata, "failed to load cargo metadata", err)`
//!
//! - **Auto-converted variants** (`Io`, `Json`, `Metadata`, etc.): Used via `?` operator
//!   for ergonomic error propagation. These lose context - prefer `Context` when you need
//!   to add meaningful context about what operation failed.
//!
//! # Clone Strategy
//!
//! `UpkeepError` intentionally does not implement `Clone` because:
//! - The `Context` variant contains a `Box<dyn StdError>` which is not `Clone`
//! - Auto-converted variants wrap error types that may not be `Clone`
//! - Cloning errors is rarely needed; pass by reference instead
//!
//! If you need to preserve an error while also returning it, consider:
//! - Using `error.to_string()` to capture the message
//! - Using `ErrorResponse::from(&error)` to create a serializable snapshot

use serde::Serialize;
use std::error::Error as StdError;
use std::fmt;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, UpkeepError>;

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Io,
    Json,
    Metadata,
    Http,
    Rustsec,
    Semver,
    Utf8,
    ExternalCommand,
    MissingTool,
    InvalidData,
    TaskFailed,
    Config,
    Concurrency,
    /// Reserved for unexpected internal errors. Currently unused but kept
    /// for future error handling needs.
    #[allow(dead_code)]
    Internal,
}

#[derive(Debug, Error)]
pub enum UpkeepError {
    #[error("{message}")]
    Message { code: ErrorCode, message: String },
    #[error("{message}")]
    Context {
        code: ErrorCode,
        message: String,
        #[source]
        source: Box<dyn StdError + Send + Sync>,
    },
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("cargo metadata error: {0}")]
    Metadata(#[from] cargo_metadata::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("RustSec error: {0}")]
    Rustsec(#[from] rustsec::Error),
    #[error("semver error: {0}")]
    Semver(#[from] semver::Error),
    #[error("UTF-8 error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("tokio semaphore error: {0}")]
    Acquire(#[from] tokio::sync::AcquireError),
}

impl UpkeepError {
    pub fn code(&self) -> ErrorCode {
        match self {
            UpkeepError::Message { code, .. } => *code,
            UpkeepError::Context { code, .. } => *code,
            UpkeepError::Io(_) => ErrorCode::Io,
            UpkeepError::Json(_) => ErrorCode::Json,
            UpkeepError::Metadata(_) => ErrorCode::Metadata,
            UpkeepError::Http(_) => ErrorCode::Http,
            UpkeepError::Rustsec(_) => ErrorCode::Rustsec,
            UpkeepError::Semver(_) => ErrorCode::Semver,
            UpkeepError::Utf8(_) => ErrorCode::Utf8,
            UpkeepError::Acquire(_) => ErrorCode::Concurrency,
        }
    }

    pub fn message(code: ErrorCode, message: impl Into<String>) -> Self {
        UpkeepError::Message {
            code,
            message: message.into(),
        }
    }

    pub fn context<E>(code: ErrorCode, message: impl Into<String>, source: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        UpkeepError::Context {
            code,
            message: message.into(),
            source: Box::new(source),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub code: ErrorCode,
    pub message: String,
    /// The chain of underlying causes, if any.
    /// Each entry represents one level of the error chain.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub causes: Vec<String>,
}

impl From<&UpkeepError> for ErrorResponse {
    fn from(error: &UpkeepError) -> Self {
        let mut causes = Vec::new();
        let mut current: Option<&(dyn StdError + 'static)> = error.source();
        while let Some(cause) = current {
            causes.push(cause.to_string());
            current = cause.source();
        }

        Self {
            code: error.code(),
            message: error.to_string(),
            causes,
        }
    }
}

/// Prints an error as JSON to stderr.
///
/// This function writes to stderr (not stdout) because error output should be
/// separate from normal command output, allowing proper stream separation in
/// shell pipelines.
pub fn eprint_error_json(error: &UpkeepError) {
    let response = ErrorResponse::from(error);
    match serde_json::to_string_pretty(&response) {
        Ok(payload) => eprintln!("{payload}"),
        Err(err) => eprintln!(
            "{}: {} (serialization error: {err})",
            response.code, response.message
        ),
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            ErrorCode::Io => "io",
            ErrorCode::Json => "json",
            ErrorCode::Metadata => "metadata",
            ErrorCode::Http => "http",
            ErrorCode::Rustsec => "rustsec",
            ErrorCode::Semver => "semver",
            ErrorCode::Utf8 => "utf8",
            ErrorCode::ExternalCommand => "external_command",
            ErrorCode::MissingTool => "missing_tool",
            ErrorCode::InvalidData => "invalid_data",
            ErrorCode::TaskFailed => "task_failed",
            ErrorCode::Config => "config",
            ErrorCode::Concurrency => "concurrency",
            ErrorCode::Internal => "internal",
        };
        write!(f, "{label}")
    }
}
