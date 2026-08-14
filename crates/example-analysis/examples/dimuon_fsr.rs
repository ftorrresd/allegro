//! Dimuon + FSR photon: recovering the Z peak from final-state radiation.
//!
//! This is the example for **mixing two kinds of object**. A muon that
//! radiates a photon carries less momentum than the one the Z decayed to, so
//! `m(μμ)` sits below the Z peak; adding the photon back puts it there. Doing
//! that means combining a `Muon` with an `FsrPhoton`, which is where the three
//! things worth knowing come up:
//!
//! 1. **Objects of different types do not add directly.** [`invariant_mass`]
//!    takes objects that are all one type. To mix kinds, convert each with
//!    `p4()` and use [`mass_of`] — `LorentzVector` is the common currency.
//!
//! 2. **A collection missing `mass` still gets a four-momentum.** `FsrPhoton`
//!    has no `mass` branch, because photons are massless, so it is declared
//!    with [`nano_object_with_mass!`] — which supplies the field from a
//!    constant and makes [`impl_four_momentum!`] work.
//!
//! 3. **Collections are linked by index, and the indices are not safe.**
//!    NanoAOD relates objects with `Muon_fsrPhotonIdx` and
//!    `FsrPhoton_muonIdx`, which are `-1` when there is no partner and are
//!    indices into a *different* collection. See [`linked`].
//!
//! ```sh
//! cargo run --release --example dimuon_fsr -- <file.root|root://…>
//! ```

use std::process::ExitCode;

use nanoaod::prelude::*;

// --- the two object kinds --------------------------------------------------

nano_object! {
    /// A reconstructed muon.
    pub struct Muon in "Muon" {
        pt: f32,
        eta: f32,
        phi: f32,
        mass: f32,
        charge: i32,
        medium_id: bool = "mediumId",
        iso: f32 = "pfRelIso04_all",
        /// Index into the `FsrPhoton` collection, or `-1`. Note the width:
        /// NanoAOD stores these as `Short_t`, so this must be `i16`.
        fsr_photon_idx: i16 = "fsrPhotonIdx",
    }
}

// Muon has pt, eta, phi and mass, so the macro writes the impl.
impl_four_momentum!(Muon);

// `FsrPhoton` has no `mass` branch — photons are massless — which leaves it a
// field short of what `impl_four_momentum!` needs. `nano_object_with_mass!`
// fills the field in from a constant instead of from the file.
nano_object_with_mass! {
    /// A final-state-radiation photon.
    pub struct FsrPhoton in "FsrPhoton" mass = 0.0 {
        pt: f32,
        eta: f32,
        phi: f32,
        /// Isolation in a cone of 0.3.
        rel_iso03: f32 = "relIso03",
        /// `ΔR(μ, γ) / E_T²`, the FSR-likeness of the association.
        dr_over_et2: f32 = "dROverEt2",
        /// Index into the `Muon` collection, or `-1`.
        muon_idx: i16 = "muonIdx",
    }
}

// Now possible, and everything taking a `FourMomentum` takes a photon too.
impl_four_momentum!(FsrPhoton);

// --- selection -------------------------------------------------------------

impl Muon {
    fn is_good(&self) -> bool {
        self.pt > 5.0 && self.eta.abs() < 2.4 && self.medium_id && self.iso < 0.35
    }
}

impl FsrPhoton {
    /// The CMS FSR working point: soft, central, isolated and close to the
    /// muon relative to its energy.
    fn is_good(&self) -> bool {
        self.pt > 2.0
            && self.eta.abs() < 2.4
            && self.rel_iso03 < 1.8
            && self.dr_over_et2 < 0.012
    }
}

/// Resolves a NanoAOD cross-collection index.
///
/// These are `-1` when there is no partner, and nothing guarantees they are in
/// range, so both cases have to be handled. `usize::try_from` rejects the
/// negative ones and `get` rejects the rest, which is why this returns an
/// `Option` rather than indexing.
fn linked<T>(index: i16, objects: &[T]) -> Option<&T> {
    usize::try_from(index).ok().and_then(|i| objects.get(i))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(input) = args.get(1) else {
        eprintln!("usage: dimuon_fsr <file.root|root://…> [out.root]");
        return ExitCode::FAILURE;
    };
    let output = args.get(2).map_or("dimuon_fsr.root", String::as_str);

    if let Err(e) = run(input, output) {
        eprintln!("dimuon_fsr: {e}");
        let mut source = std::error::Error::source(&e);
        while let Some(s) = source {
            eprintln!("  caused by: {s}");
            source = s.source();
        }
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run(input: &str, output: &str) -> Result<()> {
    let mut events = Events::open(input)?;
    println!("{} events in {}", events.len(), events.source());

    // Two collections, read the same way. Each costs only its own branches.
    let muons = events.collection::<Muon>()?;
    let photons = events.collection::<FsrPhoton>()?;

    let z_window = || {
        Axis::uniform(80, 40.0, 120.0)
            .map(|a| a.titled("m [GeV]"))
            .expect("a fixed, valid axis")
    };
    let mut bare = Histogram::h1("mumu", "m(#mu#mu), no FSR recovery", z_window())
        .with_y_title("Pairs")
        .with_sumw2();
    let mut recovered = Histogram::h1("mumugamma", "m(#mu#mu#gamma)", z_window())
        .with_y_title("Pairs")
        .with_sumw2();
    let mut shift = Histogram::h1(
        "massShift",
        "m(#mu#mu#gamma) - m(#mu#mu)",
        Axis::uniform(60, 0.0, 30.0)?.titled("#Delta m [GeV]"),
    )
    .with_y_title("Pairs");

    let mut rows: Vec<Row> = Vec::new();
    let mut pairs = 0usize;
    let mut with_fsr = 0usize;

    for i in 0..events.len() {
        // The two collections of *this* event. Indices below are into these.
        let event_muons = &muons[i];
        let event_photons = &photons[i];

        let good: Vec<&Muon> = event_muons.iter().filter(|m| m.is_good()).collect();

        for [a, b] in combinations::<_, 2>(&good) {
            if a.charge == b.charge {
                continue;
            }
            pairs += 1;

            // Same-type objects: `invariant_mass` takes them directly.
            let m_bare = invariant_mass([*a, *b]);

            // Mixing types: each object becomes a LorentzVector first, and
            // `mass_of` sums whatever it is handed. Starting from the two
            // muons and pushing any photons keeps one code path for the
            // zero-, one- and two-photon cases.
            let mut parts = vec![a.p4(), b.p4()];
            for muon in [a, b] {
                let Some(photon) = linked(muon.fsr_photon_idx, event_photons) else {
                    continue;
                };
                if !photon.is_good() {
                    continue;
                }
                // The link is only trustworthy if it points back at us, and
                // the photon has to be close to the muon that radiated it.
                let points_back = linked(photon.muon_idx, event_muons)
                    .is_some_and(|owner| std::ptr::eq(owner, *muon));
                if points_back && photon.delta_r(*muon) < 0.5 {
                    parts.push(photon.p4());
                }
            }

            let m_recovered = mass_of(parts.iter().copied());
            let had_fsr = parts.len() > 2;

            bare.fill(m_bare);
            recovered.fill(m_recovered);
            if had_fsr {
                with_fsr += 1;
                shift.fill(m_recovered - m_bare);
            }

            rows.push(Row {
                mass: m_bare as f32,
                mass_fsr: m_recovered as f32,
                photons: (parts.len() - 2) as i32,
            });
        }
    }

    println!("opposite-sign pairs: {pairs}");
    println!("pairs with a recovered FSR photon: {with_fsr}");
    println!(
        "mean m(mumu) = {:.3}, mean m(mumugamma) = {:.3}",
        bare.mean(),
        recovered.mean()
    );

    let mut out = RootWriter::create(output);
    out.write_histogram(&bare)?;
    out.write_histogram(&recovered)?;
    out.write_histogram(&shift)?;

    let mut tree = TreeWriter::new("Pairs", "one row per opposite-sign muon pair");
    tree.column_with("mass", &rows, |r| r.mass)
        .column_with("massFsr", &rows, |r| r.mass_fsr)
        .column_with("nFsrPhotons", &rows, |r| r.photons);
    out.write_tree(tree)?;
    out.finish()?;

    println!("wrote {output}");
    Ok(())
}

/// One output row.
struct Row {
    mass: f32,
    mass_fsr: f32,
    photons: i32,
}
