#!/bin/sh
# One section of CHANGELOG.md, for release notes.
#
#     scripts/changelog-section.sh 0.1.0        # the `## [0.1.0] ...` section
#     scripts/changelog-section.sh v0.1.0       # the same, tag spelling
#     scripts/changelog-section.sh Unreleased   # the section that is not a version
#
# Prints the body without its heading, and exits non-zero when there is no such
# section. That failure is the point: the changelog is where a user finds out what
# changed, so a release with nothing written about it is stopped here rather than
# published with empty notes.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
section=${1:-}
if [ -z "$section" ]; then
    echo "usage: scripts/changelog-section.sh <version|Unreleased>" >&2
    exit 2
fi
# Both spellings, because the tag is `v0.1.0` and the manifest says `0.1.0`.
section=${section#v}

awk -v want="$section" '
    function flush(   i, first, last) {
        first = 1
        while (first <= count && lines[first] ~ /^[[:space:]]*$/) first++
        last = count
        while (last >= first && lines[last] ~ /^[[:space:]]*$/) last--
        for (i = first; i <= last; i++) print lines[i]
    }

    /^## \[/ {
        if (printing) { flush(); exit }
        name = $0
        sub(/^## \[/, "", name)
        sub(/\].*$/, "", name)
        if (name == want) { printing = 1; found = 1 }
        next
    }
    printing { lines[++count] = $0 }

    END {
        if (printing) flush()
        if (!found) {
            print "CHANGELOG.md has no section for " want > "/dev/stderr"
            exit 1
        }
    }
' "$root/CHANGELOG.md"
