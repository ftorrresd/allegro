#!/usr/bin/env bash
#
# Creates a CMS NanoAOD analysis as its own git repository, depending on this
# allegro checkout through a Cargo path dependency.
#
#   ./scripts/new-analysis.sh higgs-to-4l              # ../higgs-to-4l
#   ./scripts/new-analysis.sh higgs-to-4l ~/work/h4l   # somewhere else
#
# The two repositories stay independent — edit either, commit either, push
# either. Only the path in the analysis's Cargo.toml ties them together, so a
# change in allegro shows up in the next build of the analysis.
#
# `scripts/bootstrap.sh` is the curl-able front end that clones allegro first.

set -euo pipefail

die() { printf 'new-analysis: %s\n' "$1" >&2; exit 1; }

[ $# -ge 1 ] || die "usage: $(basename "$0") <analysis-name> [directory]

<analysis-name> becomes the crate name, so use lowercase words joined by
hyphens, e.g. higgs-to-4l, ttbar-dilepton, zprime-search."

NAME="$1"
case "$NAME" in
    [a-z]*[a-z0-9]|[a-z]) ;;
    *) die "'$NAME' must start with a lowercase letter and end with a letter or digit" ;;
esac
case "$NAME" in
    *[!a-z0-9-]*) die "'$NAME' may only contain lowercase letters, digits and hyphens" ;;
esac

ALLEGRO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[ -d "$ALLEGRO/crates/nanoaod" ] || die "$ALLEGRO does not look like an allegro checkout"

TARGET="${2:-$(dirname "$ALLEGRO")/$NAME}"
[ -e "$TARGET" ] && die "$TARGET already exists"

# Cargo turns hyphens into underscores for the Rust identifier.
IDENT="${NAME//-/_}"

# The path from the analysis to allegro, so the two repositories can be moved
# together without editing anything.
relpath() {
    local from="${1%/}" to="${2%/}" common up=""
    common="$from"
    while [ "${to#"$common"/}" = "$to" ] && [ "$to" != "$common" ]; do
        common="$(dirname "$common")"
        up="../$up"
        [ "$common" = "/" ] && break
    done
    if [ "$to" = "$common" ]; then
        printf '%s' "${up%/}"
    else
        printf '%s%s' "$up" "${to#"$common"/}"
    fi
}

mkdir -p "$TARGET/src"
TARGET="$(cd "$TARGET" && pwd)"
ALLEGRO_REL="$(relpath "$TARGET" "$ALLEGRO")"

cat > "$TARGET/Cargo.toml" <<EOF
[package]
name = "$NAME"
description = "A CMS NanoAOD analysis"
version = "0.1.0"
edition = "2021"
rust-version = "1.83"

[[bin]]
name = "$IDENT"
path = "src/main.rs"

[dependencies]
# allegro is a sibling checkout, not a published crate: edit it there and the
# next build of this analysis picks the change up.
nanoaod = { path = "$ALLEGRO_REL/crates/nanoaod" }

# This package is its own workspace root, so it is never pulled into
# allegro's workspace even if the two end up nested.
[workspace]

[profile.release]
opt-level = 3
EOF

cat > "$TARGET/.gitignore" <<'EOF'
/target/
**/*.rs.bk

# Analysis outputs
*.root
*.pdf
*.png
EOF

cat > "$TARGET/README.md" <<EOF
# $NAME

A CMS NanoAOD analysis built on [allegro](../allegro).

## Layout

This repository and allegro are two independent git repositories that sit
side by side:

\`\`\`
$(dirname "$TARGET" | sed "s|$HOME|~|")/
├── allegro/     the toolkit — clone it, edit it, commit to it
└── $NAME/     this analysis
\`\`\`

\`Cargo.toml\` points at \`$ALLEGRO_REL/crates/nanoaod\`, so there is no
submodule and nothing to sync: change allegro and the next \`cargo build\`
here uses it.

## Run

\`\`\`sh
cargo run --release -- <file.root|root://…>
\`\`\`

Remote files need a grid proxy:

\`\`\`sh
voms-proxy-init --voms cms --valid 192:00
\`\`\`

## What is in the file

\`\`\`sh
cargo run --release --manifest-path $ALLEGRO_REL/Cargo.toml \\
    --example inspect -- <file.root|root://…>
\`\`\`

## Guide

\`$ALLEGRO_REL/docs/writing-an-analysis.md\`
EOF

cat > "$TARGET/src/main.rs" <<'EOF'
//! ANALYSIS_TITLE
//!
//! ```sh
//! cargo run --release -- <file.root|root://…>
//! ```

use std::process::ExitCode;

use nanoaod::prelude::*;

// ---------------------------------------------------------------------------
// 1. Declare the collections you need.
//
//    The field name is the branch suffix, so `pt` reads `Muon_pt`. Give an
//    explicit name when NanoAOD's spelling is not an idiomatic Rust one.
//    Run the `inspect` example (see the README) to see what a file has.
// ---------------------------------------------------------------------------

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

// Gives Muon `p4()`, `delta_r()` and `delta_phi()`.
impl_four_momentum!(Muon);

impl Muon {
    /// The per-object cuts. Keeping them here keeps the event loop readable.
    fn is_good(&self) -> bool {
        self.pt > 10.0 && self.eta.abs() < 2.4 && self.medium_id && self.iso < 0.25
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(input) = args.get(1) else {
        eprintln!("usage: ANALYSIS_NAME <file.root|root://…> [out.root]");
        return ExitCode::FAILURE;
    };
    let output = args.get(2).map_or("ANALYSIS_NAME.root", String::as_str);

    if let Err(e) = run(input, output) {
        eprintln!("ANALYSIS_NAME: {e}");
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

    // -----------------------------------------------------------------------
    // 2. Read the columns you need, once.
    //
    //    Only these branches are fetched. Over XRootD that is the difference
    //    between a few megabytes and the whole file.
    // -----------------------------------------------------------------------
    let muons = events.collection::<Muon>()?;

    // -----------------------------------------------------------------------
    // 3. Book the histograms.
    // -----------------------------------------------------------------------
    let mut mass = Histogram::h1(
        "dimuonMass",
        "Opposite-sign dimuon mass",
        Axis::uniform(120, 0.0, 120.0)?.titled("m_{#mu#mu} [GeV]"),
    )
    .with_y_title("Pairs")
    .with_sumw2();

    // -----------------------------------------------------------------------
    // 4. Loop over events. `muons[i]` is this event's muons.
    // -----------------------------------------------------------------------
    let mut rows: Vec<(f32, f32)> = Vec::new();

    for i in 0..events.len() {
        let good: Vec<&Muon> = muons[i].iter().filter(|m| m.is_good()).collect();

        // `combinations::<_, N>` yields every N-object group, in index order.
        for [a, b] in combinations::<_, 2>(&good) {
            if a.charge == b.charge {
                continue;
            }
            let m = invariant_mass([*a, *b]);
            mass.fill(m);
            rows.push((m as f32, a.pt.max(b.pt)));
        }
    }

    println!("pairs selected: {}", rows.len());

    // -----------------------------------------------------------------------
    // 5. Write the output. Column types follow from the Rust types.
    // -----------------------------------------------------------------------
    let mut out = RootWriter::create(output);
    out.write_histogram(&mass)?;

    let mut tree = TreeWriter::new("Pairs", "one row per selected pair");
    tree.column_with("mass", &rows, |r| r.0)
        .column_with("leadingPt", &rows, |r| r.1);
    out.write_tree(tree)?;
    out.finish()?;

    println!("wrote {output}");
    Ok(())
}
EOF

# Fill in the placeholders the heredoc kept literal.
TITLE="$(printf '%s' "$NAME" | tr '-' ' ')"
sed -i.bak "s|ANALYSIS_TITLE|The ${TITLE} analysis.|; s|ANALYSIS_NAME|${NAME}|g" \
    "$TARGET/src/main.rs" && rm -f "$TARGET/src/main.rs.bak"

# --- working on both repositories at once ----------------------------------
cat > "$TARGET/both" <<EOF
#!/usr/bin/env bash
#
# Runs one git command in this analysis and in allegro.
#
#   ./both status      what is uncommitted, on both sides
#   ./both push        push each to its own remote
#   ./both pull
#   ./both log --oneline -5
#
# They are separate repositories with separate remotes and separate histories;
# this only saves running the same command twice.

set -uo pipefail

ANALYSIS="\$(cd "\$(dirname "\${BASH_SOURCE[0]}")" && pwd)"
ALLEGRO="\$ANALYSIS/$ALLEGRO_REL"

[ \$# -gt 0 ] || set -- status -sb

status=0
for repo in "\$ANALYSIS" "\$ALLEGRO"; do
    if [ ! -d "\$repo/.git" ]; then
        printf '\n=== %s (not a git repository, skipped) ===\n' "\$repo"
        continue
    fi
    printf '\n=== %s ===\n' "\$(basename "\$repo")"
    git -C "\$repo" "\$@" || status=1
done
exit \$status
EOF
chmod +x "$TARGET/both"

# --- its own repository ----------------------------------------------------
if command -v git >/dev/null 2>&1; then
    git -C "$TARGET" init -q -b main 2>/dev/null || git -C "$TARGET" init -q
    git -C "$TARGET" add -A
    # Uses whatever identity git is already configured with; if there is none,
    # the commit is left for the user to make.
    if ! git -C "$TARGET" commit -q -m "$NAME: start from the allegro skeleton" 2>/dev/null; then
        printf 'note: nothing committed — set user.name and user.email, then commit\n'
    fi
fi

cat <<EOF

Created $TARGET

  cd $TARGET
  cargo run --release -- <file.root|root://…>

Two repositories, side by side and independent:

  $ALLEGRO
  $TARGET   →  depends on $ALLEGRO_REL/crates/nanoaod

Work in the analysis; when something belongs in the toolkit, edit
$ALLEGRO_REL and the next build here picks it up. Each pushes to its own
remote, and \`./both\` runs one git command in both:

  cd $TARGET
  git remote add origin <your analysis remote>
  ./both status
  ./both push

The guide is $ALLEGRO_REL/docs/writing-an-analysis.md
EOF
