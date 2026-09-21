#!/bin/sh
# The workspace gate. These four commands are the ones README.md names, and
# .github/workflows/ci.yml runs this script unchanged, so "the gate passes" has
# exactly one definition instead of a paragraph that can drift from the commands.
#
# Deliberately absent: the opt-in suites that need a real browser or a real Xray
# (`--ignored`, see README). They are acceptance runs, not the gate. Also absent:
# building a release artifact - that is what the release workflow is for, and it
# takes minutes this gate does not have.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root"

cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings

# The packaging and release scripts are shell, and the gate can only check that
# they parse: running one means building a release binary, and the install-and-
# verify step belongs to the release workflow. A syntax error here would
# otherwise surface on the day of a release, which is the worst day for it.
for script in scripts/version.sh scripts/changelog-section.sh \
    packaging/linux/package.sh packaging/linux/install.sh; do
    sh -n "$script"
done
# The changelog reader is asked for the one section that always exists, so a
# broken pattern is caught here rather than by a release with no notes.
scripts/changelog-section.sh Unreleased > /dev/null

echo "gate passed: fmt, check, test, clippy -D warnings, packaging scripts parse"
