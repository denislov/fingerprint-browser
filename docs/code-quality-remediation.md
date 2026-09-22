# Code quality remediation

The September 2026 review is being addressed in independently validated commits.
Behavioral fixes precede structural cleanup.

## Stages

- [x] Protect browser-data sources and unowned recovery directories; separate copy tests.
- [x] Coordinate configuration restoration with configuration writes, starts and copies.
- [ ] Keep queued starts protected until their own runtime acknowledgement.
- [ ] Match asynchronous diagnostics to individual tasks.
- [ ] Enforce diagnostic deadlines throughout partial reads and writes.
- [ ] Preserve decoded proxy credentials.
- [ ] Atomically replace configuration backups and settings files.
- [ ] Align SQLite insert/update errors and share row decoding.
- [ ] Extract large modules by responsibility and correct stale documentation.

## Validation

Stage 1: 18 browser-data tests pass, including source/sibling collisions,
unowned directory preservation and recovery across overlapping profiles.
Recovery now requires a matching operation ownership record. Unmarked `.old`
and `.partial` directories from older versions are preserved and reported for
manual recovery instead of being deleted or adopted by their suffix alone.

Stage 2: application and app tests pass. A restore job owns an exclusive
installation lease. Service mutations and browser-data workers share the same
coordinator; conflicting actions are rejected without blocking the UI. Tests
cover direct service mutation, queued starts, copies and lease release on unwind.

Real-browser, real-Xray and platform-specific tests require their corresponding
runtime environment; ordinary workspace tests do not replace those checks.
