# allegro

A toolkit for writing CMS NanoAOD analyses in pure Rust — and, as its worked
example, a reimplementation of `fourMuonMass.C` that reads a NanoAOD file
straight from the grid, selects H → ZZ* → 4μ candidates, and writes the
histogram and candidate tree to a ROOT file.

Nothing here links against ROOT, XRootD or OpenSSL. The XRootD protocol, the
GSI authentication handshake and the ROOT file format are all implemented in
this repository, the whole dependency tree is Rust, and every crate is
`#![forbid(unsafe_code)]`.

```
root://cms-xrd-global.cern.ch//store/mc/.../f58c7d36-....root   (17.1 MB NanoAOD)
        │
        │  XRootD: TCP → kXR_protocol → TLS → kXR_login → GSI (X.509 proxy)
        │          → redirect chain → kXR_open → ranged kXR_read
        ▼
   ROOT reader: TFile header, keys, TTree/TBranch/TLeaf, LZMA baskets
        │
        ▼
   selection: muon quality cuts, ZZ pairing, best-candidate scoring
        │
        ▼
   ROOT writer: TNamed ×2, TH1D (with Sumw2), TTree with 10 branches
        ▼
   fourMuonMass.root
```

Only the eleven branches the analysis needs are fetched, as byte ranges — the
17 MB file is never downloaded whole.

---

## Contents

1. [Requirements](#1-requirements)
2. [Starting an analysis](#2-starting-an-analysis)
3. [The two-repository layout](#3-the-two-repository-layout)
4. [Writing the analysis](#4-writing-the-analysis)
5. [Running it](#5-running-it)
6. [Looking inside a file](#6-looking-inside-a-file)
7. [Working on allegro itself](#7-working-on-allegro-itself)
8. [Troubleshooting](#8-troubleshooting)
9. [Crate layout](#9-crate-layout)
10. [Verification](#10-verification)
11. [Runtime](#11-runtime)
12. [Scope and limitations](#12-scope-and-limitations)

---

## 1. Requirements

### Rust

1.83 or newer. Install from [rustup.rs](https://rustup.rs):

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Use your own `rustup` toolchain, **not** the one in an LCG view — an LCG Rust
is usually older and its `cargo` may not see your registry:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
cargo --version        # expect 1.83 or newer
```

### A grid proxy (only for `root://` input)

Reading a local file needs nothing. Reading from the grid needs a valid VOMS
proxy:

```sh
voms-proxy-init --voms cms --valid 192:00
voms-proxy-info                        # check what you have
```

The proxy is found through `$X509_USER_PROXY`, falling back to
`/tmp/x509up_u$(id -u)`. `timeleft: 00:00:00` means it has expired — run
`voms-proxy-init` again.

allegro reads the proxy file directly and does the GSI handshake itself; you do
not need the XRootD client or a CMSSW environment.

---

## 2. Starting an analysis

One command, from an empty directory where you want your work to live:

```sh
mkdir -p ~/work && cd ~/work
curl -sSL https://raw.githubusercontent.com/ftorresd/allegro/main/scripts/bootstrap.sh | bash
```

It asks for a name:

```
Analysis name (lowercase, hyphens, e.g. higgs-to-4l): higgs-to-4l
Cloning allegro into /home/you/work/allegro
Created /home/you/work/higgs-to-4l
```

To skip the prompt, pass the name through:

```sh
curl -sSL https://raw.githubusercontent.com/ftorresd/allegro/main/scripts/bootstrap.sh | bash -s -- higgs-to-4l
```

If you already have allegro checked out, skip the clone:

```sh
cd allegro
./scripts/new-analysis.sh higgs-to-4l          # creates ../higgs-to-4l
./scripts/new-analysis.sh higgs-to-4l ~/other  # or somewhere specific
```

Then build and run what it made — it is a complete analysis already:

```sh
cd ~/work/higgs-to-4l
cargo run --release -- /path/to/file.root
```

Options: `ALLEGRO_REPO` overrides the clone URL, `ALLEGRO_REF` picks a branch
or tag.

---

## 3. The two-repository layout

The bootstrap leaves you with **two independent git repositories** side by
side:

```
~/work/
├── allegro/          the toolkit — its own repo, its own remote
│   ├── crates/
│   ├── docs/
│   └── scripts/
└── higgs-to-4l/      your analysis — its own repo, its own remote
    ├── Cargo.toml    nanoaod = { path = "../allegro/crates/nanoaod" }
    ├── both          run one git command in both repositories
    ├── src/main.rs
    └── README.md
```

There is **no submodule and nothing to sync**. The analysis reaches allegro
through a Cargo path dependency, so:

* work in the analysis for anything analysis-specific;
* when a change belongs in the toolkit, edit `../allegro` directly — the next
  `cargo build` in the analysis compiles it in, with no publish, vendor or
  update step;
* each repository has its own history and its own remote.

### Setting up your remote

The analysis is created with an initial commit but no remote. Add yours:

```sh
cd ~/work/higgs-to-4l
git remote add origin git@github.com:you/higgs-to-4l.git
git push -u origin main
```

If you also want to push toolkit changes, make sure `allegro`'s remote is one
you can write to — fork it and repoint, or add a second remote:

```sh
cd ~/work/allegro
git remote set-url origin git@github.com:you/allegro.git   # your fork
# or keep upstream and add your own:
git remote add mine git@github.com:you/allegro.git
```

### Day-to-day

`./both`, generated inside the analysis, runs one git command in each
repository — they stay separate, this just saves typing it twice:

```sh
cd ~/work/higgs-to-4l

./both status                # what is uncommitted, on both sides
./both log --oneline -5
./both pull
./both push                  # each to its own remote
```

A typical session where a change turns out to belong in the toolkit:

```sh
# 1. edit the analysis
vim src/main.rs

# 2. it needs something from allegro — a new field on a collection, say
vim ../allegro/crates/nanoaod/src/events.rs

# 3. one build compiles both
cargo run --release -- root://cms-xrd-global.cern.ch//store/mc/....root

# 4. commit each where it belongs
git -C ../allegro commit -am "nanoaod: expose the counter branch"
git commit -am "higgs-to-4l: cut on the new field"
./both push
```

### One caveat

Because the link is a path dependency, a collaborator who clones **only** your
analysis cannot build it — they need `allegro` checked out next to it. Point
them at the bootstrap command, which sets up exactly that layout, or tell them
to clone both as siblings.

---

## 4. Writing the analysis

**[docs/writing-an-analysis.md](docs/writing-an-analysis.md) is the full
guide.** The shape of it: declare the collections, read the columns, loop.

```rust
use nanoaod::prelude::*;

nano_object! {
    /// A reconstructed muon.
    pub struct Muon in "Muon" {
        pt: f32,
        eta: f32,
        phi: f32,
        mass: f32,
        charge: i32,
        medium_id: bool = "mediumId",
    }
}
impl_four_momentum!(Muon);

let mut events = Events::open("root://cms-xrd-global.cern.ch//store/mc/file.root")?;
let muons = events.collection::<Muon>()?;

let mut mass = Histogram::h1(
    "dimuon",
    "m_{#mu#mu}",
    Axis::uniform(120, 0.0, 120.0)?.titled("m [GeV]"),
).with_sumw2();

for i in 0..events.len() {
    let good: Vec<&Muon> = muons[i].iter().filter(|m| m.pt > 20.0 && m.medium_id).collect();
    for [a, b] in combinations::<_, 2>(&good) {
        if a.charge != b.charge {
            mass.fill(invariant_mass([*a, *b]));
        }
    }
}

let mut out = RootWriter::create("dimuon.root");
out.write_histogram(&mass)?;
out.finish()?;
```

Points worth knowing up front:

* The field name is the branch suffix, so `pt` reads `Muon_pt`. Write
  `medium_id: bool = "mediumId"` when NanoAOD's spelling is not idiomatic Rust.
* The declared type must match how the branch is stored, and the error says so
  if it does not: `branch Muon_pt holds f32, which cannot be read as i32`.
  Cross-collection indices are `Short_t`, so they read as `i16`.
* Reading is columnar: only the branches you name leave the server.
* `muons[i]` is that event's `&[Muon]`; the column is a `Jagged<Muon>`.
* `nano_object_with_mass!` declares a collection with no `mass` branch (an FSR
  photon), which is what lets `impl_four_momentum!` apply to it.
* `invariant_mass` takes objects of one type; `mass_of` combines different
  kinds through `p4()`.

### Worked examples

| File | Shows |
| --- | --- |
| `crates/example-analysis/src/four_muon.rs` | The full H → ZZ* → 4μ selection: quality cuts, four-object combinatorics, Z pairing, best-candidate scoring |
| `crates/example-analysis/examples/dimuon_spectrum.rs` | A log-spaced axis, a second collection, `delta_r` overlap counting |
| `crates/example-analysis/examples/dimuon_fsr.rs` | **Mixing two object kinds**: `mass_of` across types, a constant-mass collection, following index links between collections |

Run them from the allegro checkout:

```sh
cd ~/work/allegro
cargo run --release --example dimuon_spectrum -- /path/to/file.root
cargo run --release --example dimuon_fsr -- /path/to/file.root
```

---

## 5. Running it

Your analysis takes an input and an optional output:

```sh
cd ~/work/higgs-to-4l

# a local file
cargo run --release -- /path/to/file.root out.root

# straight from the grid
cargo run --release -- root://cms-xrd-global.cern.ch//store/mc/....root out.root

# the built binary, without cargo in the way
./target/release/higgs_to_4l /path/to/file.root
```

Always build with `--release` for real running: the debug build is roughly two
orders of magnitude slower on the decoding paths.

### The bundled four-muon program

The allegro checkout also builds `allegro`, the `fourMuonMass.C` port:

```sh
cd ~/work/allegro
cargo build --release

./target/release/allegro                                   # defaults: the CMS sample → fourMuonMass.root
./target/release/allegro /path/to/input.root out.root
./target/release/allegro root://host//store/mc/file.root out.root
./target/release/allegro --help
```

---

## 6. Looking inside a file

Before writing against a file, see what it actually holds:

```sh
cd ~/work/allegro
cargo run --release --example inspect -- /path/to/file.root
cargo run --release --example inspect -- root://host//store/mc/file.root Events
```

It prints every object in the file, and for each tree every branch with its
type, its shape and a checksum of its values:

```
tree Events — 6857 entries
  nFsrPhoton           i32     n=6857 sum=620.000000
  FsrPhoton_muonIdx    i16[]   n=620 sum=989.000000
  Muon_pt              f32[]   n=28937 sum=1031748.025840
  Muon_mediumId        bool[]  n=28937 true=26333
```

That tells you the branch name, the Rust type to declare it as, and whether it
is a scalar (`f32`), a per-event list (`f32[]`) or a fixed array (`f32[4]`).
The checksum is how you compare two files without diffing megabytes.

From inside your analysis directory, point cargo at allegro's manifest:

```sh
cargo run --release --manifest-path ../allegro/Cargo.toml \
    --example inspect -- /path/to/file.root
```

At runtime, `Events` can also tell you:

```rust
println!("{}", events.collections().join(", "));  // Muon, Jet, Electron, …
println!("{}", events.has("nJet"));               // for optional collections
```

---

## 7. Working on allegro itself

```sh
cd ~/work/allegro

cargo build --workspace
cargo test --workspace              # 120 tests
cargo clippy --workspace --all-targets
cargo doc --workspace --open
```

The test suite includes checks of the OpenSSL-compatible name hashes against
the live proxy, which are skipped when no proxy is present.

To regenerate the ROOT fixtures the histogram tests compare against (needs
ROOT, and only when you change what is covered):

```sh
root -l -b -q crates/histogram/tests/data/make_histograms.C
```

---

## 8. Troubleshooting

| Message | What it means |
| --- | --- |
| `gsi authentication failed: server does not offer gsi (token: &P=unix&P=sss,…)` | The redirector sent you to a replica that does not accept GSI. Not your fault and not deterministic — **run it again** and you will usually land on another server. |
| `gsi authentication failed: …` with a valid-looking proxy | Check `voms-proxy-info`; if `timeleft` is `00:00:00`, run `voms-proxy-init --voms cms` again. |
| `cannot resolve <host>:1094` | DNS could not find the redirect target. Check the network, and that you are not behind a firewall blocking 1094. |
| `opening /path/to/file.root` / `No such file or directory` | A local path that does not exist. Remote files must start with `root://`. |
| `no object named Events in <file>` | The file has no `Events` tree — it may not be NanoAOD. Run `inspect` to see what is in it, and `Events::open_tree(path, "Runs")` for another tree. |
| `branch Muon_pt holds f32, which cannot be read as i32` | The declared type does not match the file. `inspect` prints the right one. |
| `no branch named Muon_foo in tree Events` | A typo, or a field this NanoAOD version does not have. |
| `Muon_x does not have the same per-event lengths as the other fields of Muon` | A field that is not really part of that collection. |
| `error: failed to load manifest … ../allegro/crates/nanoaod` | allegro is not where the analysis expects it. Keep the two directories as siblings, or fix the path in `Cargo.toml`. |
| Remote reads are slow (10–15 s) | Normal: redirect and authentication round trips dominate. Local files take ~0.09 s. |

---

## 9. Crate layout

Six crates that stack in one direction, so each is usable on its own:

| Crate | What it is | Depends on |
| --- | --- | --- |
| `lorentzvector` | Four-vectors, the `FourMomentum` trait, `combinations` | — |
| `xrootd` | The XRootD protocol: framing, TLS, GSI, its own crypto | — |
| `root-io` | The ROOT format: `TFile`, `TKey`, `TTree`, typed columns, the writer | `xrootd` |
| `histogram` | `TH1`/`TH2` in all six precisions, and their ROOT records | `root-io` |
| `nanoaod` | The analysis front end: `Events`, `nano_object!`, the prelude | the three above |
| `example-analysis` | The four-muon analysis, as a worked example | `nanoaod` |

An analysis depends only on `nanoaod`, which re-exports the rest through one
prelude. `example-analysis` is there to read, not to depend on.

---

## 10. Verification

The output is compared against the file produced by the original
`fourMuonMass.C` under ROOT:

* histogram: name, title, axis titles, binning, `fEntries`, mean, RMS, all 112
  bin contents **and** their `Sumw2` errors, and all four statistics sums;
* tree: 4139 rows × 10 branches, every value;
* both `TNamed` metadata objects.

```
=== C++ reference vs Rust (read over XRootD) ===
  tree rows: 4139 compared, 0 differing
  histogram: 112 bins + errors + stats, diffs=[]
  RESULT: IDENTICAL
```

Reproducing the numbers exactly required following the original's arithmetic
precisely — muon kinematics are `Float_t` promoted to `double` for the Lorentz
algebra, while the Z candidate masses are stored back into `float` before the
window cuts and the scoring, and that rounding changes which pairing wins.

The written file is also read back by a **compiled** C++ program using
`TTreeReader`, the same API the original uses:

```
TTreeReader read 4139 entries, sum(m4mu)=518331.7553
OK: compiled C++ read the Rust-written tree
```

Histograms are checked against ROOT in both directions. `TH1C`, `TH1S`,
`TH1I`, `TH1L`, `TH1F`, `TH1D` and the `TH2` counterparts, with uniform and
with variable bins, were written by `allegro` and read back with ROOT: 18
histograms, 294 cells, no difference in any class name, axis, title, entry
count, integral, mean, RMS, bin content or bin error. The other direction is a
test: `crates/histogram/tests/data/histograms.root` is a file ROOT wrote, and
`crates/histogram/tests/root_histograms.rs` compares what `allegro` reads from
it against the numbers ROOT reports for the same histograms in the
accompanying `histograms.txt`.

```sh
cargo test --workspace --release
```

---

## 11. Runtime

Wall clock on this machine, 6857 input events:

| Input | allegro | ROOT / C++ |
| --- | --- | --- |
| local file (17.1 MB) | **0.089 s** | 3.6 s |
| over XRootD from CERN | 10–15 s | 8.5–9.7 s |

ROOT interpreter startup accounts for only ~0.37 s of its local figure. The
local gap is mostly that `allegro` touches just the eleven branches it needs,
while remote timings are dominated by redirect and authentication round trips
and vary with which replica the redirector picks.

---

## 12. Scope and limitations

* **Authentication** is GSI with an X.509 proxy. The `ztn` (bearer token)
  mechanism the servers also advertise is not implemented.
* **TLS certificate verification** is delegated to the GSI layer, which
  validates the server's certificate and identity; the TLS wrapper itself
  checks handshake signatures but does not independently validate the chain
  against the IGTF store.
* **The reader** handles flat trees of fixed-width and counter-driven branches
  (`TBranch` with `TLeaf*`). Split objects (`TBranchElement`) are rejected with
  a clear error, as are unexpected class versions.
* **Histograms** cover `TH1` and `TH2` in all six storage precisions, with
  uniform or variable bins. `TH3`, `TProfile` and bin labels are not modelled,
  and neither is the drawing state a histogram carries around — contour
  levels, the draw option, the fill buffer and the attached function list are
  read as their defaults, so a histogram that had them loses them on the way
  through. Everything that decides the numbers — binning, contents, errors and
  the statistics sums — survives unchanged.
* **The writer** emits no `StreamerInfo` record (`fSeekInfo = 0`). ROOT and
  uproot read the output using their built-in class definitions; a file with
  streamer info would be needed only for classes the reader does not know.
* **The plots** are not reproduced. `fourMuonMass.C` also writes
  `fourMuonMass.pdf` and `.png` via `TCanvas`; matching ROOT's renderer
  pixel-for-pixel is not attainable, so `allegro` produces the `.root` file
  only.
