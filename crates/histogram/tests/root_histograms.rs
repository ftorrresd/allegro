//! Histograms across the file boundary, in both directions.
//!
//! One half reads `tests/data/histograms.root`, which ROOT itself wrote, and
//! checks every flavour against the numbers ROOT reports for it in
//! `tests/data/histograms.txt`. The other half writes histograms with
//! [`RootWriter`] and reads them back, which is the path the analysis output
//! takes. Both fixtures come from `tests/data/make_histograms.C`.

use std::collections::HashMap;

use histogram::{Axis, Histogram, Precision, ReadHistogram, WriteHistogram};
use root_io::write::RootWriter;
use root_io::RootFile;

const FIXTURE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/histograms.root");
const FIXTURE_TXT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/histograms.txt");

/// One histogram as ROOT describes it.
#[derive(Default, Debug)]
struct Expected {
    class: String,
    dim: usize,
    title: String,
    x_title: String,
    y_title: String,
    /// Per axis: bins, low, high, whether the binning is variable, and the
    /// edges.
    axes: Vec<(usize, f64, f64, bool, Vec<f64>)>,
    entries: f64,
    integral: f64,
    mean: f64,
    std_dev: f64,
    sumw2: bool,
    /// Global bin, content, error.
    cells: Vec<(usize, f64, f64)>,
}

fn read_expectations() -> Vec<(String, Expected)> {
    let text = std::fs::read_to_string(FIXTURE_TXT).expect("fixture description");
    let mut out: Vec<(String, Expected)> = Vec::new();
    let mut name = String::new();
    let mut cur = Expected::default();

    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let (key, rest) = line.split_once(' ').unwrap_or((line, ""));
        let fields: Vec<&str> = rest.split_whitespace().collect();
        let num = |i: usize| fields[i].parse::<f64>().expect("number");
        match key {
            "histogram" => {
                cur = Expected {
                    class: fields[1].to_string(),
                    dim: fields[2].parse().unwrap(),
                    ..Default::default()
                };
                name = fields[0].to_string();
            }
            "title" => cur.title = rest.to_string(),
            "xtitle" => cur.x_title = rest.to_string(),
            "ytitle" => cur.y_title = rest.to_string(),
            "axis" => cur.axes.push((
                fields[1].parse().unwrap(),
                num(2),
                num(3),
                fields[4] == "1",
                fields[5..].iter().map(|f| f.parse().unwrap()).collect(),
            )),
            "stats" => {
                cur.entries = num(0);
                cur.integral = num(1);
                cur.mean = num(2);
                cur.std_dev = num(3);
            }
            "sumw2" => cur.sumw2 = fields[0] == "1",
            "cell" => cur.cells.push((fields[0].parse().unwrap(), num(1), num(2))),
            "end" => out.push((std::mem::take(&mut name), std::mem::take(&mut cur))),
            other => panic!("unexpected key {other} in {FIXTURE_TXT}"),
        }
    }
    assert!(!out.is_empty(), "no histograms described in {FIXTURE_TXT}");
    out
}

fn check_against_root(name: &str, h: &Histogram, want: &Expected) {
    assert_eq!(h.name, name);
    assert_eq!(h.class_name(), want.class, "{name}: class");
    assert_eq!(h.dim(), want.dim, "{name}: dimension");
    assert_eq!(h.title, want.title, "{name}: title");
    assert_eq!(h.x_axis().title, want.x_title, "{name}: x title");
    assert_eq!(h.y_axis().title, want.y_title, "{name}: y title");

    for (i, (nbins, min, max, variable, edges)) in want.axes.iter().enumerate() {
        let axis = if i == 0 { h.x_axis() } else { h.y_axis() };
        let which = if i == 0 { 'x' } else { 'y' };
        assert_eq!(axis.nbins(), *nbins, "{name}: {which} bins");
        assert_eq!(axis.min(), *min, "{name}: {which} low edge");
        assert_eq!(axis.max(), *max, "{name}: {which} high edge");
        assert_eq!(axis.is_uniform(), !variable, "{name}: {which} uniformity");
        assert_eq!(&axis.edges(), edges, "{name}: {which} edges");
    }

    assert_eq!(h.entries(), want.entries, "{name}: entries");
    assert_eq!(h.integral(), want.integral, "{name}: integral");
    assert_eq!(h.mean(), want.mean, "{name}: mean");
    assert_eq!(h.std_dev(), want.std_dev, "{name}: standard deviation");
    assert_eq!(h.has_sumw2(), want.sumw2, "{name}: fSumw2 presence");

    assert_eq!(h.ncells(), want.cells.len(), "{name}: cell count");
    for (bin, content, error) in &want.cells {
        assert_eq!(h.bin_content(*bin), *content, "{name}: content of bin {bin}");
        assert_eq!(h.bin_error(*bin), *error, "{name}: error of bin {bin}");
    }
}

/// Every flavour in the fixture, as ROOT reports it.
#[test]
fn reads_the_histograms_root_wrote() {
    let expectations = read_expectations();
    let mut f = RootFile::open(FIXTURE_ROOT).expect("open fixture");

    let classes: Vec<&str> = f.keys().iter().map(|k| k.class_name.as_str()).collect();
    for want in [
        "TH1D", "TH1F", "TH1C", "TH1S", "TH1I", "TH1L", "TH2D", "TH2F", "TH2I",
    ] {
        assert!(classes.contains(&want), "fixture is missing a {want}");
    }

    for (name, want) in &expectations {
        let h = f
            .histogram(name)
            .unwrap_or_else(|e| panic!("reading {name}: {e}"));
        check_against_root(name, &h, want);
    }
}

/// Reading a ROOT histogram and writing it straight back reproduces the same
/// histogram, so nothing this crate models is lost in between.
#[test]
fn a_histogram_from_root_survives_being_written_again() {
    let expectations = read_expectations();
    let mut f = RootFile::open(FIXTURE_ROOT).expect("open fixture");

    let out = std::env::temp_dir().join("allegro-rewritten-histograms.root");
    let mut w = RootWriter::create(&out);
    let originals: Vec<Histogram> = expectations
        .iter()
        .map(|(name, _)| f.histogram(name).expect("read"))
        .collect();
    for h in &originals {
        w.write_histogram(h).expect("write");
    }
    w.finish().expect("finish");

    let mut back = RootFile::open(out.to_str().unwrap()).expect("reopen");
    for ((name, want), original) in expectations.iter().zip(&originals) {
        let h = back.histogram(name).expect("read back");
        assert_eq!(&h, original, "{name} changed on the way through the file");
        check_against_root(name, &h, want);
    }
    std::fs::remove_file(out).ok();
}

/// The writer stores each histogram under the class its precision implies, so
/// a reader that goes by the key alone still gets the right type.
#[test]
fn keys_name_the_class_the_precision_implies() {
    let out = std::env::temp_dir().join("allegro-histogram-classes.root");
    let mut w = RootWriter::create(&out);

    let precisions = [
        Precision::I8,
        Precision::I16,
        Precision::I32,
        Precision::I64,
        Precision::F32,
        Precision::F64,
    ];
    let mut expected: HashMap<String, &'static str> = HashMap::new();
    for (i, precision) in precisions.into_iter().enumerate() {
        let uniform = Axis::uniform(4, 0.0, 4.0).unwrap();
        let variable = Axis::variable(&[0.0, 1.0, 3.0, 7.0]).unwrap();

        let mut h1 = Histogram::h1(&format!("h1_{i}"), "one", variable)
            .with_precision(precision)
            .with_sumw2();
        h1.fill(2.0);
        h1.fill(6.5);
        h1.fill(-1.0);
        expected.insert(h1.name.clone(), h1.class_name());
        w.write_histogram(&h1).expect("write");

        let mut h2 = Histogram::h2(&format!("h2_{i}"), "two", uniform.clone(), uniform)
            .with_precision(precision);
        h2.fill_xy(1.5, 2.5);
        h2.fill_xy(9.0, 2.5);
        expected.insert(h2.name.clone(), h2.class_name());
        w.write_histogram(&h2).expect("write");
    }
    w.finish().expect("finish");

    let mut back = RootFile::open(out.to_str().unwrap()).expect("reopen");
    let written: HashMap<String, String> = back
        .keys()
        .iter()
        .map(|k| (k.name.clone(), k.class_name.clone()))
        .collect();
    assert_eq!(written.len(), expected.len());
    for (name, class) in &expected {
        assert_eq!(written[name], *class, "key class of {name}");
        let h = back.histogram(name).expect("read back");
        assert_eq!(h.class_name(), *class);
    }
    std::fs::remove_file(out).ok();
}

/// Contents, errors and statistics all come back unchanged from a file this
/// crate wrote, including the under- and overflow cells.
#[test]
fn a_written_histogram_reads_back_identical() {
    let out = std::env::temp_dir().join("allegro-histogram-round-trip.root");
    let mut w = RootWriter::create(&out);

    let mut h1 = Histogram::h1(
        "mass",
        "H #rightarrow ZZ^{*} #rightarrow 4#mu",
        Axis::uniform(110, 70.0, 180.0).unwrap().titled("m [GeV]"),
    )
    .with_y_title("Events")
    .with_sumw2();
    for i in 0..500 {
        h1.fill_weighted(60.0 + 0.5 * i as f64, 1.0 + (i % 3) as f64);
    }

    let mut h2 = Histogram::h2(
        "plane",
        "eta against p_{T}",
        Axis::variable(&[0.0, 10.0, 20.0, 50.0, 200.0])
            .unwrap()
            .titled("p_{T} [GeV]"),
        Axis::uniform(10, -2.5, 2.5).unwrap().titled("#eta"),
    )
    .with_precision(Precision::F32)
    .with_z_title("Events");
    for i in 0..200 {
        h2.fill_xy(i as f64, -3.0 + 0.03 * i as f64);
    }

    w.write_named("provenance", "allegro integration test").unwrap();
    w.write_histogram(&h1).unwrap();
    w.write_histogram(&h2).unwrap();
    w.finish().expect("finish");

    let mut back = RootFile::open(out.to_str().unwrap()).expect("reopen");
    let r1 = back.histogram("mass").expect("read mass");
    let r2 = back.histogram("plane").expect("read plane");
    assert_eq!(r1, h1);
    assert_eq!(r2, h2);

    // Spelled out rather than left to the derived comparison, so a change in
    // what round-trips is visible here.
    assert_eq!(r1.class_name(), "TH1D");
    assert_eq!(r1.entries(), 500.0);
    assert_eq!(r1.integral(), h1.integral());
    assert_eq!(r1.mean(), h1.mean());
    assert_eq!(r1.std_dev(), h1.std_dev());
    assert!(r1.has_sumw2());
    for bin in 0..r1.ncells() {
        assert_eq!(r1.bin_content(bin), h1.bin_content(bin), "bin {bin}");
        assert_eq!(r1.bin_error(bin), h1.bin_error(bin), "error of bin {bin}");
    }
    assert_eq!(r1.x_axis().title, "m [GeV]");
    assert_eq!(r1.y_axis().title, "Events");

    assert_eq!(r2.class_name(), "TH2F");
    assert!(!r2.x_axis().is_uniform());
    assert_eq!(r2.x_axis().edges(), vec![0.0, 10.0, 20.0, 50.0, 200.0]);
    assert_eq!(r2.y_axis().nbins(), 10);
    assert_eq!(r2.ncells(), 6 * 12);
    assert_eq!(r2.entries(), 200.0);
    for bin in 0..r2.ncells() {
        assert_eq!(r2.bin_content(bin), h2.bin_content(bin), "bin {bin}");
    }

    // The other objects in the file are unaffected by the histograms.
    assert!(back.keys().iter().any(|k| k.class_name == "TNamed"));
    std::fs::remove_file(out).ok();
}

/// Asking for something that is not a histogram fails with a clear message
/// rather than returning nonsense.
#[test]
fn reading_a_non_histogram_key_is_an_error() {
    let out = std::env::temp_dir().join("allegro-histogram-wrong-key.root");
    let mut w = RootWriter::create(&out);
    w.write_named("metadata", "not a histogram").unwrap();
    w.finish().expect("finish");

    let mut back = RootFile::open(out.to_str().unwrap()).expect("reopen");
    let err = back.histogram("metadata").unwrap_err().to_string();
    assert!(err.contains("TNamed"), "unhelpful message: {err}");
    assert!(back.histogram("absent").is_err());
    std::fs::remove_file(out).ok();
}
