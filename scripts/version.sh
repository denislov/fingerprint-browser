#!/bin/sh
# The workspace version, read from the one place it is written down.
#
# `--version` prints the same number because it reads `CARGO_PKG_VERSION`, which
# cargo fills from this file. Everything else that needs the version asks this
# script rather than growing a second parser for it: the two packaging scripts
# name their artifacts with it, and the release workflow compares it against the
# tag, because a release whose tag and manifest disagree cannot be rebuilt.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

awk '
    /^\[workspace\.package\]/ { section = 1; next }
    /^\[/ { section = 0 }
    section && /^version[[:space:]]*=/ {
        gsub(/[",]/, "", $3)
        print $3
        found = 1
        exit
    }
    END {
        if (!found) {
            print "no [workspace.package] version in Cargo.toml" > "/dev/stderr"
            exit 1
        }
    }
' "$root/Cargo.toml"
