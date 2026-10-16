<!-- reviewed-through: 061b2ee (2026-10-16) -->
# Lessons from merged work

fp-browser's code is written, reviewed, merged and released by agents. What we learned is kept here. Read this before changing code. Each lesson is a rule, then why, then the evidence. "Seen" counts the days a review found it. A lesson seen on 3 or more separate days moves into [docs/code-standards.md](docs/code-standards.md).

After each release, and whenever unreleased work accumulates, the commits since the reviewed-through sha above are read for what had to be fixed again, what shouldn't have merged and what was verified too thinly. The pass updates the lessons below and moves the marker to the commit it read.

## Replacing and destroying data

**A replace writes beside the target and swaps; the old copy survives until the new one has taken over.**
- Deleting first and writing after leaves a window where neither exists, and a failure in that window costs the user what was there.
- Browser-data recovery used to delete the destination and then copy into it: a source that could not be read, a full disk or a crash left an incomplete directory and the cookies were gone ([September review](docs/code-quality-review-2026-09-22.md), P1 #2). `crates/application/src/browser_data.rs` now builds the copy beside its destination and commits with two renames, so the destination is absent only between them, and `recover` picks up a run killed in between (4b44f2e, 2026-09-23).
- Settings and configuration backups use the same shape: `crates/storage/src/atomic_file.rs` writes a private temporary file beside the destination, syncs it, and renames it over; tests inject a mid-write failure and a failed rename and assert the previous file survived (a5d0f58, 2026-09-23).
- Seen 1× (2026-09-22).

**A destructive write validates everything it will write before its first remove.**
- A write that validates record by record has already destroyed the old state when it reaches the first record it must refuse; a full disk or a crash ends with neither the old nor the new.
- Restore removed every stored core, proxy and profile and only then wrote the file's records one repository call at a time: an empty core name in the file left zero cores and zero profiles ([September review](docs/code-quality-review-2026-09-22.md), P1 #3, reproduced). It now validates the whole file first and replaces everything inside one SQLite transaction, `ConfigurationRepository::replace`, so a refusal changes nothing (b78c5cf, 2026-09-22).
- Seen 1× (2026-09-22).

## Values another program consumes

**What this program turns into another program's argument is validated where the argument is built, and everywhere the value can enter.**
- A value that reaches a command line can become a switch, or a different instruction than the form that captured it ever intended.
- A profile's start page was an unvalidated `String` the planner appended to Chromium's command line as an argument of its own; a page of `--no-proxy-server` was a profile with one more switch, and the proxy its fingerprint is built on was off ([September review](docs/code-quality-review-2026-09-22.md), P1 #5, reproduced). The rule now lives on the type (`StartTarget::validate`), is asked for by everything that stores a profile (`validate_profile`) and by the planner as the last place before the value becomes an argument (6a0692c, 2026-09-22).
- Seen 1× (2026-09-22).

## Verification

**A check named for a guarantee is proved by breaking that guarantee by hand and watching it go red.**
- A check that has never been seen red is a check of nothing: the failure it exists to catch can pass through it silently while the suite stays green.
- The orphan-reclaim acceptance probe looked for the profile directory as a bare word, while the browser holds it as `--user-data-dir=<dir>`; "no browser left behind" was green while looking at nothing, and only asserted after the probe was rewritten to match the flag (2026-09-20).
- The column-alignment test compared column boundaries but never whether a control was inside the window, so a proxies page that overflowed the window at 960 width stayed green until a size matrix test was written (2026-09-23).
- Seen 2× (2026-09-20, 2026-09-23).

## Operations that must not run twice

**A task that must not run twice at once owns a lease from the moment it is accepted, through the queue, until its own acknowledgement releases it.**
- A request that is queued but not yet visible in the snapshot looks like a request that is not running, so a second start or restore can act on state the first one still holds.
- A restore job holds an exclusive installation lease for the whole restore, shared with the other data-changing operations (5294372, 2026-09-23). Start and restart requests carry increasing ids and keep their lease until the runtime acknowledges that exact request - never until an old failure or a timeout, because time alone cannot prove a queued start will not execute (6662728, a0f885b, 2026-09-23).
- Seen 1× (2026-09-22).

## Moved to the code standards

These were seen on three or more days. The rules and their evidence are in [docs/code-standards.md](docs/code-standards.md); a review still checks each one. Nothing has moved yet.
