//! The four-muon example analysis.
//!
//! A worked example of an analysis written against [`nanoaod`]: a collection
//! declared with [`nano_object!`](nanoaod::nano_object), a per-event selection
//! over [`combinations`](lorentzvector::combinations), and its candidates
//! written out as a tree by the `allegro` binary in `src/main.rs`.
//!
//! It is a direct port of `fourMuonMass.C`, and reproduces its numbers
//! exactly. Start a new analysis from `scripts/new-analysis.sh` rather than
//! from this crate.

pub mod four_muon;
