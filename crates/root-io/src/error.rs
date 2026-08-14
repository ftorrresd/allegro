//! The ROOT I/O error type.
//!
//! Decoding failures are separated from transport failures so a caller can
//! tell "this file is not what it claims to be" from "the server went away".
//! Every variant keeps its cause reachable through
//! [`std::error::Error::source`], so a program can print a chain rather than a
//! single flattened string.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Local file I/O.
    Io {
        context: String,
        source: std::io::Error,
    },
    /// The bytes did not decode as the ROOT structure they claimed to be.
    Format(String),
    /// Bad arguments: an impossible axis, a ragged tree, an unusable path.
    Config(String),
    /// The remote file could not be reached or read.
    Xrootd(xrootd::Error),
}

impl Error {
    pub fn format(msg: impl Into<String>) -> Self {
        Error::Format(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Error::Config(msg.into())
    }

    /// Attaches a description to an I/O failure.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            context: context.into(),
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { context, .. } => write!(f, "{context}"),
            Error::Format(m) => write!(f, "root i/o error: {m}"),
            Error::Config(m) => write!(f, "configuration error: {m}"),
            Error::Xrootd(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            // `Display` forwards to the wrapped error, so the next link in the
            // chain is its cause, not the error itself.
            Error::Xrootd(e) => e.source(),
            Error::Format(_) | Error::Config(_) => None,
        }
    }
}

impl From<xrootd::Error> for Error {
    fn from(e: xrootd::Error) -> Self {
        Error::Xrootd(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::io("i/o failed", e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Adds context to a `Result`, so failures read as a chain rather than a bare
/// `os error 2`.
pub trait Context<T> {
    fn context(self, msg: impl Into<String>) -> Result<T>;
}

impl<T> Context<T> for std::result::Result<T, std::io::Error> {
    fn context(self, msg: impl Into<String>) -> Result<T> {
        self.map_err(|e| Error::io(msg, e))
    }
}
