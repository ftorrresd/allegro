#![forbid(unsafe_code)]

//! `allegro` — the `fourMuonMass.C` analysis as a self-contained Rust program.
//!
//! Reads a CMS NanoAOD file (locally or over XRootD, using this crate's own
//! implementation of that protocol), selects H → ZZ* → 4μ candidates and
//! writes the histogram and candidate tree to a ROOT file. Nothing here links
//! against ROOT, XRootD or OpenSSL.

use std::process::ExitCode;

use example_analysis::four_muon::{select_event, Candidate, Muon};
use nanoaod::prelude::*;

const DEFAULT_INPUT: &str = "root://cms-xrd-global.cern.ch//store/mc/RunIII2024Summer24NanoAODv15/\
GluGluHto2Zto4Mu-newPS_Bin-M4L-70_Fil-4L_Par-g1-g1prime2-M-125-Ga-SM_TuneCP5_13p6TeV_mcfm-pythia8/\
NANOAODSIM/150X_mcRun3_2024_realistic_v2-v2/100000/f58c7d36-3436-4ae3-b867-eb19b609280d.root";

const DEFAULT_OUTPUT: &str = "fourMuonMass.root";
const TRIGGER: &str = "HLT_Mu17_TrkIsoVVL_Mu8_TrkIsoVVL_DZ_Mass3p8";

fn main() -> ExitCode {
    if let Err(e) = run() {
        eprintln!("allegro: {e}");
        let mut source = std::error::Error::source(&e);
        while let Some(s) = source {
            eprintln!("  caused by: {s}");
            source = s.source();
        }
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("usage: allegro [INPUT] [OUTPUT]");
        println!("  INPUT   local path or root:// URL  (default: the CMS NanoAOD sample)");
        println!("  OUTPUT  ROOT file to write         (default: {DEFAULT_OUTPUT})");
        return Ok(());
    }
    let input = args.get(1).map_or(DEFAULT_INPUT, String::as_str);
    let output = args.get(2).map_or(DEFAULT_OUTPUT, String::as_str);

    let candidates = analyse(input)?;

    let mut histogram = Histogram::h1(
        "hFourMuonMass",
        "H #rightarrow ZZ^{*} #rightarrow 4#mu",
        Axis::uniform(110, 70.0, 180.0)?.titled("m_{4#mu} [GeV]"),
    )
    .with_y_title("Events")
    .with_sumw2();
    for c in &candidates {
        histogram.fill(f64::from(c.four_muon_mass));
    }

    write_output(output, input, &histogram, &candidates)?;

    println!("Selected four-muon candidates: {}", candidates.len());
    println!("Wrote {output}");
    Ok(())
}

fn analyse(input: &str) -> Result<Vec<Candidate>> {
    let mut events = Events::open(input)?;

    // Only these branches are fetched; the rest of the file is never read.
    let muons = events.collection::<Muon>()?;
    let trigger = events.scalar::<bool>(TRIGGER)?;
    let run = events.scalar::<u32>("run")?;
    let lumi = events.scalar::<u32>("luminosityBlock")?;
    let event = events.scalar::<u64>("event")?;

    let mut triggered = 0usize;
    let mut candidates = Vec::new();
    for i in 0..events.len() {
        if !trigger[i] {
            continue;
        }
        triggered += 1;
        if let Some(c) = select_event(&muons[i]) {
            candidates.push(Candidate {
                run: run[i],
                luminosity_block: lumi[i],
                event: event[i],
                ..c
            });
        }
    }

    println!("Input events: {}", events.len());
    println!("Triggered events: {triggered}");
    Ok(candidates)
}

fn write_output(
    path: &str,
    input: &str,
    histogram: &Histogram,
    candidates: &[Candidate],
) -> Result<()> {
    let mut out = RootWriter::create(path);
    out.write_named("inputXRootD", input)?;
    out.write_named("trigger", TRIGGER)?;
    out.write_histogram(histogram)?;

    let mut tree = TreeWriter::new("FourMuonTree", "Selected four-muon candidates");
    tree.column_with("run", candidates, |c| c.run)
        .column_with("luminosityBlock", candidates, |c| c.luminosity_block)
        .column_with("event", candidates, |c| c.event)
        .column_with("fourMuonMass", candidates, |c| c.four_muon_mass)
        .column_with("z1Mass", candidates, |c| c.z1_mass)
        .column_with("z2Mass", candidates, |c| c.z2_mass)
        .column_with("muonPt", candidates, Candidate::pt)
        .column_with("muonEta", candidates, Candidate::eta)
        .column_with("muonPhi", candidates, Candidate::phi)
        .column_with("muonCharge", candidates, Candidate::charge);
    out.write_tree(tree)?;

    out.finish()?;
    Ok(())
}
