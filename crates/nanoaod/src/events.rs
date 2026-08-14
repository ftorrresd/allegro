//! Writing a CMS NanoAOD analysis.
//!
//! A NanoAOD file is a flat tree of columns whose names follow one convention:
//! a scalar per event (`run`, `HLT_*`), or a collection where every field is
//! `Collection_field` and all fields of a collection share one length per
//! event (`nMuon`, `Muon_pt`, `Muon_eta`). [`Events`] reads either, and
//! [`nano_object!`] turns the naming convention into a struct.
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
//!         iso: f32 = "pfRelIso04_all",
//!     }
//! }
//! impl_four_momentum!(Muon);
//!
//! let mut events = Events::open("root://cms-xrd-global.cern.ch//store/...")?;
//!
//! // Columns are read once, up front; only these branches leave the server.
//! let muons = events.collection::<Muon>()?;
//! let trigger = events.scalar::<bool>("HLT_IsoMu24")?;
//!
//! for i in 0..events.len() {
//!     if !trigger[i] {
//!         continue;
//!     }
//!     let good: Vec<&Muon> = muons[i]
//!         .iter()
//!         .filter(|m| m.pt > 20.0 && m.eta.abs() < 2.4 && m.medium_id)
//!         .collect();
//!     if let [a, b, ..] = good[..] {
//!         println!("dimuon mass {}", invariant_mass([a, b]));
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Reading is columnar because the file is: asking for `Muon_pt` fetches that
//! branch's baskets and nothing else, so an analysis over eleven branches of a
//! 17 MB file transfers only those eleven.

use std::collections::BTreeSet;

use root_io::tree::Tree;
use root_io::{Element, Jagged, Result, RootFile};

/// A physics object read from a NanoAOD collection.
///
/// Implemented by [`nano_object!`]; implement it by hand only for a collection
/// whose fields do not follow the `Collection_field` convention.
pub trait Object: Sized {
    /// The branch-name prefix, e.g. `"Muon"` for `Muon_pt`.
    const COLLECTION: &'static str;

    /// Reads every field of the collection and zips them into one object per
    /// entry.
    fn read_from(events: &mut Events) -> Result<Jagged<Self>>;
}

/// An open tree of events.
///
/// [`Events::open`] takes a local path or a `root://` URL and finds the
/// `Events` tree; [`Events::open_tree`] picks a different one, such as `Runs`
/// or `LuminosityBlocks`.
pub struct Events {
    file: RootFile,
    tree: Tree,
}

impl Events {
    /// Opens the `Events` tree of a NanoAOD file.
    pub fn open(path: &str) -> Result<Self> {
        Self::open_tree(path, "Events")
    }

    /// Opens a named tree, for the files whose payload is not called `Events`.
    pub fn open_tree(path: &str, tree: &str) -> Result<Self> {
        let mut file = RootFile::open(path)?;
        let tree = file.tree(tree)?;
        Ok(Self { file, tree })
    }

    /// Number of events in the tree.
    pub fn len(&self) -> usize {
        self.tree.entries.max(0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Where the data is being read from.
    pub fn source(&self) -> String {
        self.file.describe()
    }

    /// The tree being read, for the branch list and entry count.
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// The file, for reading objects other than this tree.
    pub fn file(&mut self) -> &mut RootFile {
        &mut self.file
    }

    /// Whether the file has a branch, for the branches that exist in
    /// simulation but not in data.
    pub fn has(&self, branch: &str) -> bool {
        self.tree.branches.iter().any(|b| b.name == branch)
    }

    /// Every branch name, in the order the file stores them.
    pub fn branch_names(&self) -> impl Iterator<Item = &str> {
        self.tree.branches.iter().map(|b| b.name.as_str())
    }

    /// The collection prefixes present, e.g. `Muon`, `Jet`, `Electron`.
    ///
    /// A prefix counts as a collection when the file also has the `n<Prefix>`
    /// counter NanoAOD writes for it, which keeps `HLT_*` and `PV_*` out.
    pub fn collections(&self) -> Vec<&str> {
        let names: BTreeSet<&str> = self.branch_names().collect();
        names
            .iter()
            .filter_map(|n| n.strip_prefix('n'))
            .filter(|prefix| {
                names
                    .iter()
                    .any(|n| n.starts_with(prefix) && n[prefix.len()..].starts_with('_'))
            })
            .collect()
    }

    /// Reads a branch holding one value per event.
    ///
    /// The type must match how the branch is stored; the error names both if
    /// it does not.
    pub fn scalar<T: Element>(&mut self, branch: &str) -> Result<Vec<T>> {
        self.file.scalar(&self.tree, branch)
    }

    /// Reads a branch holding a variable-length list per event.
    pub fn jagged<T: Element>(&mut self, branch: &str) -> Result<Jagged<T>> {
        self.file.jagged(&self.tree, branch)
    }

    /// Reads a whole collection, one struct per object per event.
    ///
    /// ```no_run
    /// # use nanoaod::prelude::*;
    /// # nano_object! { pub struct Muon in "Muon" { pt: f32 } }
    /// # let mut events = Events::open("f.root")?;
    /// let muons = events.collection::<Muon>()?;
    /// let leading = muons[0].iter().max_by(|a, b| a.pt.total_cmp(&b.pt));
    /// # Ok::<(), root_io::Error>(())
    /// ```
    pub fn collection<C: Object>(&mut self) -> Result<Jagged<C>> {
        C::read_from(self)
    }

    /// Reads one field of a collection, given the prefix and the field name.
    ///
    /// [`nano_object!`] calls this; it is public so a hand-written [`Object`]
    /// can too.
    pub fn field<T: Element>(&mut self, collection: &str, field: &str) -> Result<Jagged<T>> {
        self.jagged(&format!("{collection}_{field}"))
    }
}

/// Declares a NanoAOD physics object and how to read it.
///
/// The struct field name is the branch suffix unless one is given explicitly,
/// which is how NanoAOD's `mediumId` becomes an idiomatic `medium_id`:
///
/// ```no_run
/// use nanoaod::prelude::*;
///
/// nano_object! {
///     /// A reconstructed muon.
///     pub struct Muon in "Muon" {
///         pt: f32,
///         eta: f32,
///         charge: i32,
///         medium_id: bool = "mediumId",
///     }
/// }
/// ```
///
/// The generated struct derives `Debug`, `Clone`, `Copy` and `PartialEq`, and
/// implements [`Object`] so [`Events::collection`] can read it. Reading checks
/// that every field really is one column of the same collection, so a typo in
/// a branch name fails at the read rather than silently misaligning objects.
#[macro_export]
macro_rules! nano_object {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident in $collection:literal {
            $(
                $(#[$field_meta:meta])*
                $field:ident : $ty:ty $(= $branch:literal)?
            ),* $(,)?
        }
    ) => {
        $crate::__nano_object! {
            $(#[$meta])*
            $vis struct $name in $collection
            fixed { }
            branches { $( $(#[$field_meta])* $field : $ty $(= $branch)? , )* }
        }
    };
}

/// Declares a NanoAOD object whose mass is a constant rather than a branch.
///
/// Some collections have no `mass` column, because the mass is not something
/// the reconstruction measures: an `FsrPhoton` is massless, and a `Tau` decay
/// product may be assigned a fixed hypothesis. That leaves them one field
/// short of what [`impl_four_momentum!`] needs.
///
/// This declares the same struct with `mass` filled in from a constant, so the
/// object can carry a four-momentum like any other:
///
/// ```no_run
/// use nanoaod::prelude::*;
///
/// nano_object_with_mass! {
///     /// A final-state-radiation photon. Photons are massless.
///     pub struct FsrPhoton in "FsrPhoton" mass = 0.0 {
///         pt: f32,
///         eta: f32,
///         phi: f32,
///         rel_iso03: f32 = "relIso03",
///     }
/// }
///
/// // Now possible, because the struct has all four fields.
/// impl_four_momentum!(FsrPhoton);
/// ```
///
/// The `mass` field is an ordinary `f32` field of the struct; it is simply not
/// read from the file. Every object of the collection gets the same value.
///
/// Use a hand-written [`FourMomentum`](lorentzvector::FourMomentum) impl
/// instead when the mass is not constant — when it depends on a decay
/// hypothesis the analysis chooses per object, say.
#[macro_export]
macro_rules! nano_object_with_mass {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident in $collection:literal mass = $mass:literal {
            $(
                $(#[$field_meta:meta])*
                $field:ident : $ty:ty $(= $branch:literal)?
            ),* $(,)?
        }
    ) => {
        $crate::__nano_object! {
            $(#[$meta])*
            $vis struct $name in $collection
            fixed {
                /// Not read from the file: this collection has no `mass`
                /// branch, so every object carries the declared constant.
                mass : f32 = $mass,
            }
            branches { $( $(#[$field_meta])* $field : $ty $(= $branch)? , )* }
        }
    };
}

/// The shared body of [`nano_object!`] and [`nano_object_with_mass!`].
///
/// `fixed` fields are filled from a constant; `branches` fields are read from
/// the file.
#[doc(hidden)]
#[macro_export]
macro_rules! __nano_object {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident in $collection:literal
        fixed {
            $( $(#[$fixed_meta:meta])* $fixed:ident : $fixed_ty:ty = $value:literal, )*
        }
        branches {
            $( $(#[$field_meta:meta])* $field:ident : $ty:ty $(= $branch:literal)? , )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq)]
        $vis struct $name {
            $(
                $(#[$field_meta])*
                pub $field: $ty,
            )*
            $(
                $(#[$fixed_meta])*
                pub $fixed: $fixed_ty,
            )*
        }

        impl $crate::events::Object for $name {
            const COLLECTION: &'static str = $collection;

            fn read_from(
                events: &mut $crate::events::Events,
            ) -> $crate::__private::root_io::Result<$crate::__private::root_io::Jagged<Self>> {
                $(
                    let $field = events.field::<$ty>(
                        $collection,
                        $crate::__nano_object!(@branch $field $(, $branch)?),
                    )?;
                )*

                // Every field of a collection is indexed by the same counter,
                // so their event boundaries must agree.
                let mut shape: Option<Vec<usize>> = None;
                $(
                    match &shape {
                        None => shape = Some($field.offsets().to_vec()),
                        Some(seen) if seen != $field.offsets() => {
                            return Err($crate::__private::root_io::Error::format(format!(
                                "{}_{} does not have the same per-event lengths as the \
                                 other fields of {}",
                                $collection,
                                $crate::__nano_object!(@branch $field $(, $branch)?),
                                $collection,
                            )));
                        }
                        Some(_) => {}
                    }
                )*

                let offsets = shape.ok_or_else(|| {
                    $crate::__private::root_io::Error::format(concat!(
                        "collection ", $collection,
                        " was declared with no fields read from the file"
                    ))
                })?;
                let total = offsets.last().copied().unwrap_or(0);
                let values = (0..total)
                    .map(|k| Self {
                        $( $field: $field.values()[k], )*
                        $( $fixed: $value, )*
                    })
                    .collect();
                Ok($crate::__private::root_io::Jagged::from_parts(values, offsets))
            }
        }
    };

    // The branch suffix: the given one, else the field's own name.
    (@branch $field:ident, $branch:literal) => { $branch };
    (@branch $field:ident) => { stringify!($field) };
}

#[cfg(test)]
mod tests {
    use root_io::Jagged;

    nano_object! {
        /// A muon, as the tests read it.
        pub struct Muon in "Muon" {
            pt: f32,
            eta: f32,
            charge: i32,
            medium_id: bool = "mediumId",
        }
    }

    nano_object_with_mass! {
        /// A massless object, whose mass no branch carries.
        pub struct FsrPhoton in "FsrPhoton" mass = 0.0 {
            pt: f32,
            eta: f32,
            phi: f32,
            muon_idx: i16 = "muonIdx",
        }
    }

    // The whole point: the generated struct has all four fields, so this
    // compiles. Without the constant mass it would not.
    crate::impl_four_momentum!(FsrPhoton);

    #[test]
    fn a_constant_mass_completes_the_four_momentum() {
        use lorentzvector::FourMomentum;

        let g = FsrPhoton {
            pt: 5.0,
            eta: 0.5,
            phi: 0.25,
            muon_idx: 3,
            mass: 0.0,
        };
        assert_eq!(g.mass, 0.0);
        assert_eq!(FourMomentum::mass(&g), 0.0);

        // A massless vector: E == |p| exactly, since that is how it was built.
        let p4 = g.p4();
        assert_eq!(p4.energy(), p4.momentum_squared().sqrt());
        assert!((p4.pt() - 5.0).abs() < 1e-6);
        // Recovering the mass back out of `E² - p²` cancels away most of the
        // significand, so it comes back near zero rather than at it — the same
        // way it does from ROOT's `TLorentzVector`.
        assert!(p4.mass().abs() < 1e-7 * p4.energy());

        // And it mixes with anything else carrying a four-momentum.
        let muon = lorentzvector::LorentzVector::from_pt_eta_phi_m(40.0, 0.5, 0.25, 0.106);
        assert!(g.delta_r(&muon) < 1e-6);
        assert!(lorentzvector::mass_of([g.p4(), muon]) > muon.mass());
    }

    #[test]
    fn the_macro_names_branches_after_the_fields() {
        use crate::events::Object;
        assert_eq!(Muon::COLLECTION, "Muon");
        assert_eq!(FsrPhoton::COLLECTION, "FsrPhoton");
        // The struct is a plain value type the analysis can copy around.
        let m = Muon {
            pt: 30.0,
            eta: 1.0,
            charge: -1,
            medium_id: true,
        };
        assert_eq!(m, m);
        assert!(format!("{m:?}").contains("medium_id"));
    }

    #[test]
    fn objects_group_by_event() {
        let muons: Jagged<Muon> = vec![
            vec![
                Muon {
                    pt: 30.0,
                    eta: 0.0,
                    charge: 1,
                    medium_id: true,
                },
                Muon {
                    pt: 10.0,
                    eta: 1.0,
                    charge: -1,
                    medium_id: false,
                },
            ],
            vec![],
        ]
        .into_iter()
        .collect();

        assert_eq!(muons.len(), 2);
        assert_eq!(muons[0].len(), 2);
        assert!(muons[1].is_empty());
        let leading = muons[0].iter().max_by(|a, b| a.pt.total_cmp(&b.pt));
        assert_eq!(leading.map(|m| m.pt), Some(30.0));
    }
}
