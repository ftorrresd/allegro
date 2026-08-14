//! A from-scratch XRootD client: framing, TLS, GSI authentication and ranged
//! reads.
//!
//! Nothing here links against XRootD or OpenSSL. The protocol, the GSI
//! handshake and every cryptographic primitive it needs are implemented in
//! this crate, on top of RustCrypto and rustls.
//!
//! ```no_run
//! use xrootd::XrdFile;
//!
//! let mut f = XrdFile::open("root://cms-xrd-global.cern.ch//store/mc/file.root")?;
//! println!("{} bytes", f.size());
//! // Only the bytes asked for cross the network.
//! let header = f.read_at(0, 100)?;
//! # Ok::<(), xrootd::Error>(())
//! ```
//!
//! [`XrdFile::open`] connects, negotiates TLS if the server demands it, logs
//! in, authenticates with the user's X.509 proxy and follows redirects from a
//! redirector to the data server holding the replica.

pub mod conn;
pub mod crypto;
pub mod error;
pub mod gsi;
pub mod proto;
pub mod session;
pub mod tls;

pub use error::{Context, Error, Result};
pub use session::{Session, XrdFile, XrdUrl};
