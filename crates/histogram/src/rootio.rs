//! Moving histograms in and out of ROOT files.
//!
//! These are extension traits rather than inherent methods because the file
//! types live in `root-io`, which knows the ROOT container format but nothing
//! about histograms.

use root_io::write::RootWriter;
use root_io::RootFile;

use crate::error::Result;
use crate::Histogram;

/// Reading a histogram from an open ROOT file.
pub trait ReadHistogram {
    /// Reads the named histogram: any `TH1` or `TH2`, in any of ROOT's storage
    /// precisions.
    fn histogram(&mut self, name: &str) -> Result<Histogram>;
}

impl ReadHistogram for RootFile {
    fn histogram(&mut self, name: &str) -> Result<Histogram> {
        let (key, payload) = self.object(name)?;
        Histogram::parse(&payload, &key.class_name)
    }
}

/// Writing a histogram into a ROOT file being built.
pub trait WriteHistogram {
    /// Writes a histogram under the class its dimension and precision imply,
    /// `TH1D` through `TH2L`.
    fn write_histogram(&mut self, h: &Histogram) -> Result<()>;
}

impl WriteHistogram for RootWriter {
    fn write_histogram(&mut self, h: &Histogram) -> Result<()> {
        let payload = h.serialize();
        self.write_object(h.class_name(), &h.name, &h.title, &payload)?;
        Ok(())
    }
}
