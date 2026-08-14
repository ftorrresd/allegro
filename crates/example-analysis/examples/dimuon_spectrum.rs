//! The dimuon mass spectrum: a second, unrelated analysis over the same file.
//!
//! It exists to show what writing a new CMS analysis costs — declare the
//! collections, read the columns, loop. The J/ψ, Υ and Z peaks all land in one
//! histogram.
//!
//! ```sh
//! cargo run --release --example dimuon_spectrum -- <file.root|root://…>
//! ```

use std::process::ExitCode;

use nanoaod::prelude::*;

nano_object! {
    /// A reconstructed muon.
    #[derive(Default)]
    pub struct Muon in "Muon" {
        pt: f32,
        eta: f32,
        phi: f32,
        mass: f32,
        charge: i32,
        /// The loose identification working point.
        loose_id: bool = "looseId",
        /// Particle-flow relative isolation in a cone of 0.4.
        iso: f32 = "pfRelIso04_all",
    }
}

impl_four_momentum!(Muon);

nano_object! {
    /// A reconstructed jet, read only to show a second collection costs
    /// nothing but its declaration.
    pub struct Jet in "Jet" {
        pt: f32,
        eta: f32,
        phi: f32,
        mass: f32,
    }
}

impl_four_momentum!(Jet);

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(input) = args.get(1) else {
        eprintln!("usage: dimuon_spectrum <file.root|root://…> [out.root]");
        return ExitCode::FAILURE;
    };
    let output = args.get(2).map_or("dimuon.root", String::as_str);

    if let Err(e) = run(input, output) {
        eprintln!("dimuon_spectrum: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run(input: &str, output: &str) -> Result<()> {
    let mut events = Events::open(input)?;
    println!("{} events in {}", events.len(), events.source());
    println!("collections: {}", events.collections().join(", "));

    let muons = events.collection::<Muon>()?;
    // A collection that may be absent in some samples costs one check.
    let jets = events
        .has("nJet")
        .then(|| events.collection::<Jet>())
        .transpose()?;

    // A log-spaced axis puts the J/psi, the Upsilon and the Z in one plot.
    let edges: Vec<f64> = (0..=120)
        .map(|i| 10f64.powf(-0.4 + 2.7 * f64::from(i) / 120.0))
        .collect();
    let mut spectrum = Histogram::h1(
        "dimuonSpectrum",
        "Opposite-sign dimuon mass",
        Axis::variable(&edges)?.titled("m_{#mu#mu} [GeV]"),
    )
    .with_y_title("Pairs")
    .with_sumw2();

    let mut leading_pt = Histogram::h1(
        "leadingMuonPt",
        "Leading muon p_{T}",
        Axis::uniform(100, 0.0, 200.0)?.titled("p_{T} [GeV]"),
    )
    .with_y_title("Events");

    let mut pairs = 0usize;
    let mut jet_muon_overlap = 0usize;

    for i in 0..events.len() {
        let good: Vec<&Muon> = muons[i].iter().filter(|m| selected(m)).collect();

        if let Some(leading) = good.iter().copied().max_by(|a, b| a.pt.total_cmp(&b.pt)) {
            leading_pt.fill(f64::from(leading.pt));
        }

        for [a, b] in combinations::<_, 2>(&good) {
            if a.charge == b.charge {
                continue;
            }
            spectrum.fill(invariant_mass([*a, *b]));
            pairs += 1;
        }

        // delta_r comes from the FourMomentum trait, so it works between any
        // two objects without either knowing about the other.
        if let Some(jets) = &jets {
            jet_muon_overlap += jets[i]
                .iter()
                .filter(|j| good.iter().any(|m| j.delta_r(*m) < 0.4))
                .count();
        }
    }

    println!("opposite-sign pairs: {pairs}");
    println!("jets overlapping a selected muon: {jet_muon_overlap}");
    println!(
        "Z window [80, 100]: {} pairs",
        (1..=spectrum.x_axis().nbins())
            .filter(|&b| (80.0..=100.0).contains(&spectrum.x_axis().bin_center(b)))
            .map(|b| spectrum.bin_content(b))
            .sum::<f64>()
    );

    let mut out = RootWriter::create(output);
    out.write_histogram(&spectrum)?;
    out.write_histogram(&leading_pt)?;
    out.finish()?;
    println!("wrote {output}");
    Ok(())
}

/// The per-muon quality cuts this analysis uses.
fn selected(m: &Muon) -> bool {
    m.pt > 3.0 && m.eta.abs() < 2.4 && m.loose_id && m.iso < 0.25
}
