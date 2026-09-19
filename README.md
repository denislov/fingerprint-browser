# Fingerprint Browser v1 Design Baseline

This package is the first architecture baseline for a personal Rust fingerprint-browser manager using GPUI Kit, fingerprint-chromium and Xray.

## Files

- `architecture.md` — system boundaries, workspace, dependency direction and runtime architecture.
- `domain-model.md` — core Rust domain types and persistent/ephemeral state separation.
- `service-contracts.md` — repository, planner, supervisor and runtime interfaces.
- `lifecycle.md` — browser/Xray startup, rollback, stop and crash state machine.
- `implementation-plan.md` — phased implementation order and first test matrix.
- `Cargo.workspace.example.toml` — workspace skeleton only; dependency versions are resolved during implementation.

## Decisions fixed for v1

1. Rust desktop application with GPUI Kit.
2. Existing fingerprint-chromium is the browser engine; no Chromium fork in v1.
3. SQLite local storage.
4. One Xray process per running proxied profile.
5. Stable seed + explicit persona fields; avoid a large legacy fingerprint field matrix.
6. UI never owns child processes.
7. RuntimeSupervisor is the single owner of Chromium/Xray process handles.
8. Profile configuration and RuntimeSession are separate models.
9. CDP is loopback-only and used for readiness/basic control, not an automation platform.
10. Xray failure closes the corresponding Chromium session (fail closed).

## Implementation status

Phases 0–2 have landed: the Cargo workspace, domain models, SQLite repositories,
application services and direct Chromium supervisor. Phase 3 now has its first
runtime implementation for SOCKS5/HTTP upstream proxies:

- A per-profile Xray process starts before Chromium, after writing a loopback-only
  SOCKS configuration and checking local TCP readiness.
- Stop, startup rollback and detected component crashes reclaim both processes
  and remove the temporary proxy configuration. Xray crashes close Chromium.
- Runtime snapshots include the Xray PID and clear process/port fields on stop.
- Other outbound variants remain stored domain types but are rejected at launch
  until their transport/TLS configuration is implemented and tested.

Configure `SupervisorComponents.xray_executable`, `runtime_dir` and
`xray_ready_timeout` when constructing the supervisor. Defaults are `bin/xray`
(`bin/xray.exe` on Windows), `data/runtime` and five seconds. Temporary configs
live at `<runtime_dir>/<profile-id>/xray.json`; Unix files use mode `0600`.

## Validation and next steps

```sh
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings -A stable-features
```

The existing `.cargo/config.toml` injects two feature attributes that are already
stable on the installed compiler; `-A stable-features` bypasses only those legacy
warnings. Strict `-D warnings` alone currently fails on that existing configuration.

Runtime lifecycle tests on Unix require Python 3 and permission to bind loopback
ports. They use controlled child processes rather than a real browser/Xray pair.
Real Xray 26.2.6 has also passed an opt-in integration test against local
authenticated SOCKS5 and HTTP CONNECT upstream fixtures, including bidirectional
payload forwarding. Run it with:

```sh
XRAY_BIN=/absolute/path/to/xray cargo test -p runtime --test xray_real -- --ignored
```

The binary is not bundled. Linux acceptance with real ungoogled-chromium
148.0.7778.215 and Xray 26.2.6 now covers profile isolation, cookie persistence,
authenticated local proxy forwarding, crash recovery and shutdown. See
[the acceptance report](docs/chromium-acceptance.md) for reproduction and limits.
Actual remote upstreams and Windows process-tree cleanup still need acceptance.

Startup readiness now polls existing sessions during Xray waits and between
bounded CDP requests. CDP bypasses environment proxies and requires browser and
WebSocket metadata. Unix children run in independent process groups, allowing
cleanup of descendants even after their leader exits (children that deliberately
leave the group require stronger OS containment).

Stop and shutdown commands now interrupt Xray/CDP readiness waits. Cancelling a
start reclaims its children and configuration before publishing `Stopped`;
restart queues a fresh launch after cleanup. Other starts remain deferred, while
stops for running profiles are handled during the wait. Channel disconnection
also cancels startup and shuts down the supervisor. Responsiveness is bounded by
the current readiness probe (normally at most 100 ms), plus process cleanup;
filesystem operations and process creation are still synchronous.

CDP and SOCKS TCP ports are reserved by live loopback listeners during planning.
Each reservation is released immediately before its child is spawned, and all
error/cancellation paths release reservations automatically. External programs
bind their own sockets, so a small release-to-bind race remains; readiness and
child-exit checks handle detected startup failures rather than claiming atomic
port handoff.

Events are bounded, best-effort notifications sent without blocking. UI clients
must reconcile from `RuntimeFacade::snapshot` on notifications, periodically and
on reconnect, rather than replaying events as authoritative state. Snapshots retain
effective arguments, the latest error/warning and a cumulative `dropped_events`
counter, including when the receiver is full or disconnected. Diagnostics reset
on a new start; stop preserves them. This is in-memory recovery, not a durable
event history.

Normal stop first requests CDP `Browser.close` so Chromium can flush persistent
state, waits up to two seconds for exit, and force-cleans remaining processes.
Unavailable CDP falls back to forced cleanup; crash/rollback paths skip graceful
close. Persistence is not guaranteed on a forced exit.

Before declaring Phase 3 fully accepted for the fingerprint browser product,
repeat acceptance with fingerprint-chromium and wire runtime settings and services
into the GPUI UI.
Phase 4 then adds version detection and fingerprint capability compatibility.
