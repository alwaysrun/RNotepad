//! Error types shared by the Win32 layer.

use thiserror::Error;

/// Error raised by a Win32 wrapper or a view.
#[derive(Debug, Error)]
pub enum WinError {
    #[error("{api} failed: {code}")]
    Windows { api: &'static str, code: String },
    #[error("{0}")]
    Message(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Scintilla is unavailable: {0}")]
    Scintilla(String),
    #[error("document error: {0}")]
    Document(#[from] rnotepad_core::document::DocumentError),
    #[error("encoding error")]
    Encoding,
}

impl WinError {
    /// Creates a message error.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// Wraps a `windows` crate error and records the API name.
    #[must_use]
    pub fn from_windows(api: &'static str, err: windows::core::Error) -> Self {
        Self::Windows {
            api,
            code: format!("{err}"),
        }
    }
}

/// Convenience result alias.
pub type WinResult<T> = Result<T, WinError>;
