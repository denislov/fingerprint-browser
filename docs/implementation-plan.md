# Implementation plan

Updated 2026-09-20. This page describes current status and remaining work.
Original phased plans and batch notes are preserved in
[implementation-history.md](implementation-history.md); historical next-batch
statements there are not current TODOs.

## Implemented

| Area | Current state |
| --- | --- |
| Workspace/domain/storage | Five Rust crates, domain validation, SQLite repositories/migrations |
| Direct runtime | Independent profiles, CDP readiness, cancel/stop/restart, graceful close and crashes |
| Xray | Per-profile process, six outbound builders, transport/TLS settings, fail-closed cleanup |
| Fingerprints | Version/capability checks, warnings, live read-back; Linux 142/144/148 measured |
| Desktop UI | Profile forms, core/proxy management, link import, settings, runtime details and logs |
| Windows lifecycle | Atomic job assignment, kill-on-close containment, native identity, recovery with retained failure records |
| Validation | Linux/Windows CI configuration, native process tests, opt-in real Windows Chromium/Xray acceptance |

The desktop workflow is implemented. “Phase 5 has started” is outdated, and
“only two proxy protocols” describes the manual form, not the importer/runtime.

## Windows lifecycle acceptance

- [x] Chromium/Xray enter private jobs atomically at creation.
- [x] Stop/rollback cleans descendants even after the leader exits.
- [x] Killing the manager closes its jobs and preserves unrelated processes.
- [x] PID + native creation time identifies records; mismatches are refused.
- [x] Failed reclamation retains records; stale records are removed.
- [x] Real Chromium: simultaneous profiles, cookie isolation/persistence,
  cancellation, normal close, crashes, Xray failure, shutdown and manager crash.
- [ ] Reproduce remote upstream forwarding on Windows.
- [ ] Measure Windows fingerprint surfaces independently of Linux evidence.

Commands and limits: [windows-acceptance.md](windows-acceptance.md).

## Next bounded tasks

1. Proxy diagnostics: test through Xray and show connectivity, elapsed time and
   exit IP. Distinguish configuration, authentication and network errors. A local
   listening port alone is not proof of successful remote forwarding.
2. Profile management: search, selection and bounded batch start/stop/proxy
   assignment, with per-profile results and cancellation.
3. Backup/restore: define configuration-only versus browser-data backups,
   credential handling and ID/path conflict rules before adding export/import.
4. Distribution: repeatable Windows release build, first-run executable setup,
   diagnostics and packaging. Do not silently download binaries.

## Required gate

scripts/check.ps1 (Windows) and scripts/check.sh (Linux) run:

1. cargo fmt --all --check
2. cargo check --workspace
3. cargo test --workspace
4. cargo clippy --workspace --all-targets -- -D warnings

CI is configured for Windows and Ubuntu. Real browser/Xray acceptance remains
opt-in and is reported separately from the gate.
