//! Histograms, independent of the file they come from or go to.
//!
//! [`Histogram`] models ROOT's `TH1` and `TH2` in all six storage precisions
//! (`TH1C`, `TH1S`, `TH1I`, `TH1L`, `TH1F`, `TH1D` and their two-dimensional
//! counterparts), with uniform or variable bin edges.
//!
//! ```
//! use histogram::{Axis, Histogram, Precision};
//!
//! let mut h = Histogram::h1("mass", "m_{4#mu}", Axis::uniform(110, 70.0, 180.0)?)
//!     .with_precision(Precision::F32)
//!     .with_sumw2();
//! h.fill(125.1);
//! assert_eq!(h.entries(), 1.0);
//! assert_eq!(h.class_name(), "TH1F");
//! # Ok::<(), histogram::Error>(())
//! ```
//!
//! Filling reproduces `TH1::Fill` and `TH2::Fill`: the entry counter always
//! advances, the bin and its squared-error companion are incremented, and the
//! statistics sums are updated only for values inside the axis range (ROOT's
//! default `fStatOverflows` behaviour). Contents are accumulated *in the
//! stored precision*, so a `TH1F` rounds where ROOT's `TH1F` rounds and the
//! integer flavours truncate and saturate where ROOT's do.
//!
//! # Through a ROOT file
//!
//! [`ReadHistogram`] and [`WriteHistogram`] extend `root-io`'s file types, so
//! a histogram round-trips without either side knowing about the other:
//!
//! ```no_run
//! use histogram::{Axis, Histogram, ReadHistogram, WriteHistogram};
//! use root_io::{write::RootWriter, RootFile};
//!
//! let mut out = RootWriter::create("mass.root");
//! out.write_histogram(&Histogram::h1("m", "", Axis::uniform(10, 0.0, 1.0)?))?;
//! out.finish()?;
//!
//! let mut back = RootFile::open("mass.root")?;
//! let h = back.histogram("m")?;
//! # Ok::<(), histogram::Error>(())
//! ```

pub mod error;
mod hist;
mod rootio;

pub use error::{Error, Result};
pub use hist::{Axis, Histogram, Precision};
pub use rootio::{ReadHistogram, WriteHistogram};
