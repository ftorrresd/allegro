//! The analysis error type.
//!
//! An analysis touches every layer below it, so this is mostly a join of their
//! errors: `?` lifts a file, histogram or protocol failure without the caller
//! spelling out a conversion, and the chain stays printable through
//! [`std::error::Error::source`].

use std::fmt;

#[derive(Debug)]
pub enum Error {
    /// Reading or writing a ROOT file.
    RootIo(root_io::Error),
    /// Filling or decoding a histogram.
    Histogram(histogram::Error),
    /// The analysis itself: a branch that is not what the selection expects,
    /// a missing collection, an impossible cut.
    Analysis(String),
}

impl Error {
    pub fn analysis(msg: impl Into<String>) -> Self {
        Error::Analysis(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::RootIo(e) => write!(f, "{e}"),
            Error::Histogram(e) => write!(f, "{e}"),
            Error::Analysis(m) => write!(f, "analysis error: {m}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            // Both wrappers forward their `Display`, so the next link in the
            // chain is the wrapped error's cause, not the wrapper's contents.
            Error::RootIo(e) => e.source(),
            Error::Histogram(e) => e.source(),
            Error::Analysis(_) => None,
        }
    }
}

impl From<root_io::Error> for Error {
    fn from(e: root_io::Error) -> Self {
        Error::RootIo(e)
    }
}

impl From<histogram::Error> for Error {
    fn from(e: histogram::Error) -> Self {
        match e {
            // A file failure that reached us through a histogram call is still
            // a file failure; unwrapping it keeps the printed chain short.
            histogram::Error::Io(e) => Error::RootIo(e),
            other => Error::Histogram(other),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
