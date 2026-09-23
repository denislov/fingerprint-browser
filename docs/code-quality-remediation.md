# Code quality remediation

The September 2026 review is being addressed in independently validated commits.
Behavioral fixes precede structural cleanup.

## Stages

- [x] Protect browser-data sources and unowned recovery directories; separate copy tests.
- [x] Coordinate configuration restoration with configuration writes, starts and copies.
- [x] Keep queued starts protected until their own runtime acknowledgement.
- [x] Match asynchronous diagnostics to individual tasks.
- [x] Enforce diagnostic deadlines throughout partial reads and writes.
- [x] Preserve decoded proxy credentials.
- [x] Atomically replace configuration backups and settings files.
- [x] Align SQLite insert/update errors and share row decoding.
- [x] Extract large modules by responsibility and correct stale documentation.

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

Stage 3: start/restart requests carry monotonically increasing IDs. Snapshots
acknowledge handled and cancelled requests. The UI releases a queued-start lease
only for its own acknowledgement, never for an old failure or elapsed time.
An unresponsive supervisor therefore keeps data-changing operations blocked;
timeouts alone cannot prove a queued start will never execute.
Queued starts remain visible as Starting with an enabled Stop action, and
cannot be deleted before the runtime acknowledges completion or cancellation.

Stage 4: diagnostic results carry their originating task, and fingerprint
readings also match the runtime session. An edited proxy's obsolete result
cannot release a new start gate. Concurrent old/new proxy workers have separate
temporary engine directories. Restore invalidates all previous readings.

Stage 5: diagnostic socket operations recheck a single absolute deadline before
each read/write. Connection establishment consumes the same budget. A local
drip-feed peer regression verifies that partial progress cannot renew a timeout.

Stage 6: Trojan and Shadowsocks credentials retain decoded whitespace. Tests
cover percent-encoded Trojan passwords and both base64 Shadowsocks link forms.

Stage 7: backups and settings share a same-directory atomic file replacement
helper. The temporary file is private and synced before replacement. Tests
inject a mid-write failure and a failed rename; both preserve the existing
destination and remove the temporary file. App, application and storage tests pass.

Stage 8: SQLite profile writes share constraint classification; get/list share
one row decoder per entity. Both backend contract tests now check dangling-core
and dangling-proxy updates and verify the original row survives refusal.

Structural cleanup: runtime ownership, command handover, launch orchestration,
diagnostic transport and temporary engines are separate modules. URI parsing is
split by protocol and transport. AppState read models, activity and summaries,
UI worker dispatch and lifecycle, settings actions, settings persistence and
resolution, editor rendering, and text formatting have separate homes. Large
state/UI test suites are grouped by scenario with shared fixtures. Existing
public entry points are retained; obsolete foreign-key and timeout comments
were corrected. Runtime/domain tests and strict Clippy passed before the app
reorganization; the full workspace gate also passes after it.

Follow-up: the profile editor's initial seed and reroll now use the same OS
entropy source as application-created profiles, replacing the remaining clock
seed path found during the form split. Existing editor and seed-source tests
exercise both creation paths.

## Final validation

`CARGO_NET_OFFLINE=true ./scripts/check.sh` passed on Linux:

- Formatting and workspace compilation.
- 678 tests passed, zero failures, 24 environment-dependent tests ignored.
- Workspace/all-target Clippy with warnings denied.
- Packaging script syntax and changelog extraction checks.

The independent reproductions from the review were rerun against the final
code: overlapping backup paths return an error with the original Cookies file
intact; encoded password whitespace is retained; a 150 ms drip-feed diagnostic
returns Timeout at approximately 154 ms; dangling insert and update both return
`StorageError::Dangling`. Windows execution and opt-in browser/Xray tests have
not been run in this Linux environment.

## Implementation commits

| Commit | Stage |
| --- | --- |
| `4b44f2e` | Browser-data path checks and owned recovery |
| `5294372` | Restore installation lease and service coordination |
| `6662728` | Request-specific runtime acknowledgements |
| `ddb0934` | Diagnostic task identity and isolated engine directories |
| `2bed082` | Absolute diagnostic deadlines |
| `4f2698a` | Decoded password preservation |
| `a5d0f58` | Atomic backup/settings replacement |
| `db0fb91` | Repository errors and shared row decoding |
| `e0c341b` | Runtime and URI responsibility split |
| `d6279e7` | App presentation, worker and test organization |
| `a0f885b` | Queued-start cancellation and deletion protection |
| `c5ee291` | Editor seed entropy and adjacent documentation cleanup |

Real-browser, real-Xray and platform-specific tests require their corresponding
runtime environment; ordinary workspace tests do not replace those checks.
