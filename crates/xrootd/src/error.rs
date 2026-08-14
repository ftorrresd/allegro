//! The XRootD client's error type.
//!
//! Failures are grouped by the layer that produced them so callers can react
//! to them (a `Server` error carrying `ENOENT` means "try the next replica",
//! an `Auth` error does not). Each variant keeps the underlying cause
//! reachable through [`std::error::Error::source`] so `main` can print a
//! chain instead of a single flattened string.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Socket or local file I/O.
    Io {
        context: String,
        source: std::io::Error,
    },
    /// The peer's bytes did not match the XRootD wire format.
    Protocol(String),
    /// A well-formed `kXR_error` response.
    Server { code: i32, message: String },
    /// The GSI handshake failed or was rejected.
    Auth(String),
    /// A cryptographic or X.509 operation failed.
    Crypto(String),
    /// TLS setup or handshake failure.
    Tls(String),
    /// Bad arguments or environment.
    Config(String),
}

impl Error {
    pub fn protocol(msg: impl Into<String>) -> Self {
        Error::Protocol(msg.into())
    }
    pub fn auth(msg: impl Into<String>) -> Self {
        Error::Auth(msg.into())
    }
    pub fn crypto(msg: impl Into<String>) -> Self {
        Error::Crypto(msg.into())
    }
    pub fn tls(msg: impl Into<String>) -> Self {
        Error::Tls(msg.into())
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
            Error::Protocol(m) => write!(f, "xrootd protocol error: {m}"),
            Error::Server { code, message } => {
                write!(f, "server returned error {code}: {message}")
            }
            Error::Auth(m) => write!(f, "gsi authentication failed: {m}"),
            Error::Crypto(m) => write!(f, "cryptographic error: {m}"),
            Error::Tls(m) => write!(f, "tls error: {m}"),
            Error::Config(m) => write!(f, "configuration error: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io {
            context: "i/o failed".into(),
            source: e,
        }
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

/// Lets the string-returning crypto helpers participate in `?`.
impl<T> Context<T> for std::result::Result<T, String> {
    fn context(self, msg: impl Into<String>) -> Result<T> {
        self.map_err(|e| Error::Crypto(format!("{}: {e}", msg.into())))
    }
}
