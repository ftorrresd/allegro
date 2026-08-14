//! The histogram error type.

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// An axis that cannot exist: no bins, a backwards range, unsorted edges.
    Axis(String),
    /// A record that is not the histogram its key claims.
    Decode(String),
    /// Reading or writing the file the histogram lives in.
    Io(root_io::Error),
}

impl Error {
    pub fn axis(msg: impl Into<String>) -> Self {
        Error::Axis(msg.into())
    }

    pub fn decode(msg: impl Into<String>) -> Self {
        Error::Decode(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Axis(m) => write!(f, "invalid axis: {m}"),
            Error::Decode(m) => write!(f, "histogram record: {m}"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            // `Display` forwards to the wrapped error, so the next link is
            // its cause rather than the error itself.
            Error::Io(e) => e.source(),
            Error::Axis(_) | Error::Decode(_) => None,
        }
    }
}

impl From<root_io::Error> for Error {
    fn from(e: root_io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;
