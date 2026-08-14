//! Writing a CMS NanoAOD analysis.
//!
//! This is the crate an analysis depends on. It brings together the layers
//! below it — [`lorentzvector`] for kinematics, [`root_io`] for the file
//! format and [`histogram`] for `TH1`/`TH2` — behind one [`prelude`], and adds
//! the part specific to NanoAOD: reading its collections as Rust structs.
//!
//! ```no_run
//! use nanoaod::prelude::*;
//!
//! nano_object! {
//!     /// A reconstructed muon.
//!     pub struct Muon in "Muon" {
//!         pt: f32,
//!         eta: f32,
//!         phi: f32,
//!         mass: f32,
//!         charge: i32,
//!         medium_id: bool = "mediumId",
//!     }
//! }
//! impl_four_momentum!(Muon);
//!
//! # fn main() -> Result<()> {
//! let mut events = Events::open("root://cms-xrd-global.cern.ch//store/mc/….root")?;
//!
//! // Columns are read once, up front; only these branches leave the server.
//! let muons = events.collection::<Muon>()?;
//!
//! let mut mass = Histogram::h1(
//!     "dimuon",
//!     "m_{#mu#mu}",
//!     Axis::uniform(120, 0.0, 120.0)?.titled("m [GeV]"),
//! );
//!
//! for i in 0..events.len() {
//!     let good: Vec<&Muon> = muons[i]
//!         .iter()
//!         .filter(|m| m.pt > 20.0 && m.eta.abs() < 2.4 && m.medium_id)
//!         .collect();
//!     for [a, b] in combinations::<_, 2>(&good) {
//!         if a.charge != b.charge {
//!             mass.fill(invariant_mass([*a, *b]));
//!         }
//!     }
//! }
//!
//! let mut out = RootWriter::create("dimuon.root");
//! out.write_histogram(&mass)?;
//! out.finish()?;
//! # Ok(())
//! # }
//! ```
//!
//! `docs/writing-an-analysis.md` in the repository is the longer guide, and
//! `scripts/new-analysis.sh` generates a crate that already runs.

pub mod error;
pub mod events;

pub use error::{Error, Result};
pub use events::{Events, Object};

/// Everything an analysis normally needs.
///
/// ```
/// use nanoaod::prelude::*;
/// ```
pub mod prelude {
    pub use crate::error::{Error, Result};
    pub use crate::events::{Events, Object};
    pub use crate::{impl_four_momentum, nano_object, nano_object_with_mass};
    pub use histogram::{Axis, Histogram, Precision, ReadHistogram, WriteHistogram};
    pub use lorentzvector::{
        combinations, delta_phi, invariant_mass, mass_of, FourMomentum, LorentzVector, MUON_MASS,
        Z_MASS,
    };
    pub use root_io::write::{tree::TreeWriter, RootWriter};
    pub use root_io::{Jagged, RootFile};
}

pub use lorentzvector::impl_four_momentum;

/// Re-exported so [`nano_object!`] expands in crates that depend on this one
/// alone, without their having to name the lower crates.
#[doc(hidden)]
pub mod __private {
    pub use lorentzvector;
    pub use root_io;
}
