#!/usr/bin/env bash
#
# Starts a CMS NanoAOD analysis from nothing:
#
#   curl -sSL https://raw.githubusercontent.com/ftorrresd/allegro/main/scripts/bootstrap.sh | bash
#
# It asks for a name, clones allegro next to where you ran it, and creates the
# analysis as a separate git repository beside it:
#
#   .
#   ├── allegro/        the toolkit — its own repo, yours to edit
#   └── <name>/         your analysis — its own repo, depends on ../allegro
#
# Both are ordinary git repositories with no submodule or subtree between
# them: the analysis reaches allegro through a Cargo path dependency, so an
# edit on either side is picked up by the next build.
#
# Non-interactive:
#
#   curl -sSL …/bootstrap.sh | bash -s -- my-analysis
#
# Environment:
#   ALLEGRO_REPO    clone URL       (default: the GitHub repository)
#   ALLEGRO_REF     branch or tag   (default: the repository's default branch)

set -euo pipefail

REPO="${ALLEGRO_REPO:-https://github.com/ftorrresd/allegro.git}"
REF="${ALLEGRO_REF:-}"

say()  { printf '%s\n' "$*"; }
die()  { printf 'bootstrap: %s\n' "$1" >&2; exit 1; }

command -v git >/dev/null 2>&1 || die "git is required"
command -v cargo >/dev/null 2>&1 || say "warning: cargo not found — install Rust from https://rustup.rs"

# --- where to put things ---------------------------------------------------
#
# If we are standing in an allegro checkout, work beside it; otherwise use the
# current directory as the parent of both repositories.
if [ -f "Cargo.toml" ] && [ -d "crates/nanoaod" ]; then
    ALLEGRO="$(pwd)"
    BASE="$(dirname "$ALLEGRO")"
elif [ -d "allegro/crates/nanoaod" ]; then
    BASE="$(pwd)"
    ALLEGRO="$BASE/allegro"
else
    BASE="$(pwd)"
    ALLEGRO="$BASE/allegro"
fi

# --- the analysis name -----------------------------------------------------
NAME="${1:-}"

# Ask on the terminal rather than on stdin: under `curl | bash` stdin is this
# script, so a plain `read` would swallow the rest of it. `/dev/tty` can exist
# and still not be openable (cron, a container with no tty), which a `-r` test
# does not catch — so open it and see.
if [ -z "$NAME" ] && { exec 3<>/dev/tty; } 2>/dev/null; then
    while [ -z "$NAME" ]; do
        printf 'Analysis name (lowercase, hyphens, e.g. higgs-to-4l): ' >&3
        read -r NAME <&3 || break
    done
    exec 3>&-
fi

[ -n "$NAME" ] || die "no name given, and no terminal to ask on.
Pass it as an argument:
  curl -sSL …/bootstrap.sh | bash -s -- my-analysis"

case "$NAME" in
    [a-z]*[a-z0-9]|[a-z]) ;;
    *) die "'$NAME' must start with a lowercase letter and end with a letter or digit" ;;
esac
case "$NAME" in
    *[!a-z0-9-]*) die "'$NAME' may only contain lowercase letters, digits and hyphens" ;;
esac

TARGET="$BASE/$NAME"
[ -e "$TARGET" ] && die "$TARGET already exists"

# --- allegro ---------------------------------------------------------------
if [ -d "$ALLEGRO/crates/nanoaod" ]; then
    say "Using the allegro checkout at $ALLEGRO"
else
    say "Cloning allegro into $ALLEGRO"
    if [ -n "$REF" ]; then
        git clone --branch "$REF" "$REPO" "$ALLEGRO"
    else
        git clone "$REPO" "$ALLEGRO"
    fi
fi

[ -x "$ALLEGRO/scripts/new-analysis.sh" ] || die "$ALLEGRO/scripts/new-analysis.sh is missing"

# --- the analysis ----------------------------------------------------------
"$ALLEGRO/scripts/new-analysis.sh" "$NAME" "$TARGET"
