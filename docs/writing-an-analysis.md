# Writing an analysis

This is the guide to going from a NanoAOD file to a histogram and an output
tree. It assumes you can write Rust, but not that you know this codebase.

## Start from the generator

```sh
curl -sSL https://raw.githubusercontent.com/ftorrresd/allegro/main/scripts/bootstrap.sh | bash
```

It asks for a name, clones allegro, and creates the analysis as a **separate
git repository** beside it:

```
work/
├── allegro/        the toolkit — its own repo and remote
└── higgs-to-4l/    your analysis — its own repo and remote
```

If allegro is already checked out, `./scripts/new-analysis.sh higgs-to-4l`
skips the clone.

What you get is a complete analysis that already runs: it reads a muon
collection, fills a dimuon mass histogram and writes it out. Edit it from
there.

```sh
cd higgs-to-4l
cargo run --release -- /path/to/file.root
```

### Changing allegro too

The analysis depends on allegro through a Cargo path dependency
(`nanoaod = { path = "../allegro/crates/nanoaod" }`), not a submodule or a
published version. So when something belongs in the toolkit rather than in the
analysis, edit `../allegro` directly — the next build of the analysis compiles
it in. Nothing to publish, vendor or sync.

The two repositories keep separate histories and separate remotes. `./both`,
generated in the analysis, runs one git command in each:

```sh
./both status          # what is uncommitted, on both sides
./both push            # push each to its own remote
./both log --oneline -5
```

The one cost of a path dependency: a collaborator who clones only the analysis
cannot build it. They need allegro beside it, which the bootstrap script sets
up.

The generated crate depends on `nanoaod`, which re-exports everything through
one prelude:

```rust
use nanoaod::prelude::*;
```

## Look at the file first

Before writing against a file, see what it holds:

```sh
cargo run --release --example inspect -- /path/to/file.root
```

It prints the objects in the file, and for each tree every branch with its
type, its shape and a checksum of its values. That last column is how you
check two files agree without diffing megabytes.

`Events` can also tell you at runtime:

```rust
let events = Events::open(input)?;
println!("{}", events.collections().join(", "));   // Muon, Jet, Electron, …
println!("{}", events.has("nJet"));                // for optional collections
```

## The four steps

### 1. Declare the collections

NanoAOD stores a collection as one branch per field, all sharing a per-event
length: `nMuon`, `Muon_pt`, `Muon_eta`. `nano_object!` turns that convention
into a struct:

```rust
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
    }
}
```

The field name is the branch suffix, so `pt` reads `Muon_pt`. Give an explicit
name — `medium_id: bool = "mediumId"` — when NanoAOD's spelling is not an
idiomatic Rust one.

The type must match how the branch is stored. If it does not, the read fails
with a message naming both types rather than producing wrong numbers:

```
branch Muon_pt holds f32, which cannot be read as i32
```

Add `impl_four_momentum!(Muon)` when the struct has `pt`, `eta`, `phi` and
`mass`. That gives it `p4()`, `delta_r()` and `delta_phi()` against any other
object with a four-momentum.

Put the per-object cuts on the struct, so the event loop stays about events:

```rust
impl Muon {
    fn is_good(&self) -> bool {
        self.pt > 10.0 && self.eta.abs() < 2.4 && self.medium_id && self.iso < 0.25
    }
}
```

### 2. Read the columns

```rust
let mut events = Events::open("root://cms-xrd-global.cern.ch//store/…")?;

let muons = events.collection::<Muon>()?;
let trigger = events.scalar::<bool>("HLT_IsoMu24")?;
let run = events.scalar::<u32>("run")?;
```

Reading is columnar because the file is. Each call fetches that branch's
baskets and nothing else — over XRootD an analysis over eleven branches of a
17 MB file transfers only those eleven, as ranged requests.

`Events::open` takes a local path or a `root://` URL and behaves the same
either way. Remote files need a grid proxy (see the README).

A collection whose fields disagree on their per-event lengths is rejected at
the read, so a typo in a branch name cannot silently misalign objects.

### 3. Loop over events

```rust
for i in 0..events.len() {
    if !trigger[i] {
        continue;
    }
    let good: Vec<&Muon> = muons[i].iter().filter(|m| m.is_good()).collect();

    for [a, b] in combinations::<_, 2>(&good) {
        if a.charge != b.charge {
            mass.fill(invariant_mass([*a, *b]));
        }
    }
}
```

`muons[i]` is this event's `&[Muon]`. `combinations::<_, N>` yields every
`N`-object group in index order — pairs, triplets, the four-lepton
quadruplets. `invariant_mass` takes anything with a four-momentum.

### 4. Write the output

```rust
let mut out = RootWriter::create("higgs.root");
out.write_named("provenance", "produced by higgs-to-4l")?;
out.write_histogram(&mass)?;

let mut tree = TreeWriter::new("Pairs", "one row per selected pair");
tree.column("mass", &masses)                        // &[f32]
    .column("muonPt", &muon_pts)                    // &[[f32; 4]]
    .column_with("run", &rows, |r| r.run);          // computed per row
out.write_tree(tree)?;

out.finish()?;
```

The branch type follows from the Rust type: `&[f32]` becomes `mass/F` on a
`TLeafF`, `&[[f32; 4]]` becomes `muonPt[4]/F`, `&[u64]` becomes `event/l`.
Columns that disagree on their row count are rejected before anything is
written.

The result is a file ROOT itself opens.

## Mixing two kinds of object

`invariant_mass` takes objects that are all the same type, so a muon and a
photon will not go in together. Convert each with `p4()` and use `mass_of` —
`LorentzVector` is the common currency every object converts to:

```rust
let m_mumu  = invariant_mass([a, b]);              // same type
let m_mumug = mass_of([a.p4(), b.p4(), g.p4()]);   // mixed types
```

### Collections with no `mass` branch

`impl_four_momentum!` reads `pt`, `eta`, `phi` and `mass` off the struct, so a
collection missing one of them is a field short. An FSR photon has no `mass`
branch, because photons are massless. Declare it with `nano_object_with_mass!`,
which supplies the field from a constant instead of from the file:

```rust
nano_object_with_mass! {
    /// A final-state-radiation photon.
    pub struct FsrPhoton in "FsrPhoton" mass = 0.0 {
        pt: f32,
        eta: f32,
        phi: f32,
        rel_iso03: f32 = "relIso03",
    }
}

impl_four_momentum!(FsrPhoton);   // now possible
```

`mass` becomes an ordinary `f32` field carrying the same value on every
object, and everything taking a `FourMomentum` — `p4`, `delta_r`, `delta_phi`
— takes an `FsrPhoton` too.

Write the trait out by hand when the mass is *not* constant, e.g. when it
depends on a decay hypothesis the analysis picks per object:

```rust
impl FourMomentum for Candidate {
    fn pt(&self) -> f64 { f64::from(self.pt) }
    fn eta(&self) -> f64 { f64::from(self.eta) }
    fn phi(&self) -> f64 { f64::from(self.phi) }
    fn mass(&self) -> f64 { self.hypothesis.mass() }
}
```

### Following index links

NanoAOD relates collections by index: `Muon_fsrPhotonIdx` points into
`FsrPhoton`, `FsrPhoton_muonIdx` back into `Muon`. They are `-1` when there is
no partner, and nothing guarantees they are in range, so resolve rather than
index:

```rust
fn linked<T>(index: i16, objects: &[T]) -> Option<&T> {
    usize::try_from(index).ok().and_then(|i| objects.get(i))
}
```

`try_from` rejects the `-1`s and `get` rejects anything out of range. Check
the width when you declare the field — these indices are `Short_t`, so they
must be read as `i16`:

```rust
fsr_photon_idx: i16 = "fsrPhotonIdx",
```

Indices are relative to the *same event*, so resolve them against
`photons[i]`, not the whole column.

`crates/example-analysis/examples/dimuon_fsr.rs` does all three.

## Histograms

```rust
let mut h = Histogram::h1(
    "mass",
    "H #rightarrow ZZ^{*} #rightarrow 4#mu",
    Axis::uniform(110, 70.0, 180.0)?.titled("m_{4#mu} [GeV]"),
)
.with_y_title("Events")
.with_sumw2();

h.fill(125.1);
h.fill_weighted(125.1, 0.9);
```

`Axis::variable(&edges)` gives non-uniform bins. `Histogram::h2` gives a `TH2`,
filled with `fill_xy`. `with_precision(Precision::F32)` picks the storage type
and with it the class written (`TH1F` rather than `TH1D`).

Filling reproduces `TH1::Fill` exactly, including which fills reach the
statistics sums and how the integer flavours saturate — so mean, RMS and bin
contents agree with ROOT bin for bin.

## Where things live

| Crate | What it is |
| --- | --- |
| `lorentzvector` | Four-vectors, `FourMomentum`, `combinations`. No dependencies. |
| `xrootd` | The XRootD protocol, its own TLS and GSI authentication. |
| `root-io` | The ROOT file format: `TFile`, `TTree`, typed columns, the writer. |
| `histogram` | `TH1`/`TH2`, and their ROOT records. |
| `nanoaod` | The analysis front end: `Events`, `nano_object!`, the prelude. |
| `example-analysis` | The four-muon analysis, as a worked example. |

An analysis normally needs only `nanoaod`, which re-exports the rest.
`example-analysis` is a worked example to read, not something to depend on.

## Worked examples

- `crates/example-analysis/src/four_muon.rs` — the H → ZZ* → 4μ selection:
  quality cuts, four-muon combinatorics, Z pairing and best-candidate scoring,
  with `src/main.rs` driving it and writing the output tree.
- `crates/example-analysis/examples/dimuon_spectrum.rs` — the dimuon mass
  spectrum over a log-spaced axis, plus a second collection and a `delta_r`
  overlap count.
- `crates/example-analysis/examples/dimuon_fsr.rs` — recovering the Z peak
  from final-state radiation, and the example for **mixing two object kinds**:
  `mass_of` across types, `FourMomentum` by hand for a massless photon, and
  index links between collections.

Run either with:

```sh
cargo run --release --example dimuon_spectrum -- /path/to/file.root
cargo run --release --example dimuon_fsr -- /path/to/file.root
```

## Notes

**The event loop is a plain loop.** There is no framework driving your code,
no scheduler and no callbacks. If you want to parallelise it, the columns are
plain `Vec`s and `Jagged`s.

**Columns are read whole.** `events.collection::<Muon>()` reads every entry of
every field it names. That is what makes the loop cheap, and it is why you
should declare only the fields you use.

**Prefer borrowing in the loop.** `muons[i]` hands out a slice; filtering into
a `Vec<&Muon>` copies pointers, not muons.

**`f32` in, `f64` out.** NanoAOD stores kinematics as `Float_t`. The
`FourMomentum` trait promotes to `f64` for the Lorentz algebra, which is what
ROOT's `TLorentzVector` does when you hand it floats.
