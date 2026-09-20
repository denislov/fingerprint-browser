#!/bin/sh
# The workspace gate. These four commands are the ones README.md names, and
# .github/workflows/ci.yml runs this script unchanged, so "the gate passes" has
# exactly one definition instead of a paragraph that can drift from the commands.
#
# Deliberately absent: the opt-in suites that need a real browser or a real Xray
# (`--ignored`, see README). They are acceptance runs, not the gate.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

echo "gate passed: fmt, check, test, clippy -D warnings"
