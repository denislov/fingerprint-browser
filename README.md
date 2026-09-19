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

Phase 4 has started: version detection, the capability table, the
compatibility report, and read-back verification of the fingerprint itself.

- `BrowserCore.major` now comes from the binary. `runtime::version::VersionReport`
  runs `<executable> --version` under a five second deadline and parses the first
  digit run (`Chromium 148.0.7778.215` -> 148); `FP_BROWSER_CHROMIUM_MAJOR`
  overrides it for binaries that do not answer usefully.
- The catalogue is reconciled on every start: a replaced binary is re-detected
  and its stored major and auto-generated name are refreshed, a custom name is
  kept, and a missing executable is reported. A core whose version cannot be
  re-read keeps its stored major rather than being downgraded to "unknown".
- `CoreCapabilities::for_major` is a table instead of a stub. Majors split at
  `FingerprintGeneration::PIVOT_MAJOR` (144, the first generation whose switch
  set was verified against a real engine): both generations carry the stable set
  (seed, brand, platform, both version switches, language, timezone, hardware
  concurrency, WebRTC policy, `--disable-spoofing`), and only the verified
  generation carries the canvas and client-rects noise switches. See
  [the switch matrix](docs/fingerprint-matrix.md) for the evidence and the gaps.
- Nothing is dropped in silence. A switch the core cannot honour is omitted at
  serialization time and reported by `runtime::compat::check`, which the
  supervisor turns into a `Warning` event; it lands in
  `RuntimeSnapshot::last_warning`, on the profile row and in Runtime Details.
  An undetected version (major `0`) is refused instead of being resolved to an
  assumed capability set.
- A switch name is not evidence. `CdpSession` opens the browser's debugging
  endpoint on loopback and `FingerprintProbe` reads the fingerprint back out of
  a real page; `runtime::verify` compares that reading with the profile and
  reports every claim it does not support (a missing reading is itself a
  finding). Measuring the verified generation this way found three switches
  this project was emitting that the engine silently ignores: `mac` instead of
  `macos`, `client-rects` instead of `clientrects`, and two brands no build
  honours. All three are fixed and covered by
  [the switch matrix](docs/fingerprint-matrix.md).

```sh
# fingerprint surface, read back out of a real browser
CHROMIUM_BIN=/absolute/path/to/chrome cargo test -p runtime --test fingerprint_real -- --ignored
# process lifetime, proxies, cookie isolation
CHROMIUM_BIN=/absolute/path/to/chrome XRAY_BIN=/absolute/path/to/xray cargo test -p runtime --test chromium_real -- --ignored
```

Phase 5 also has its first slice: the GPUI window is wired to the services
instead of being a static mockup.

- `crates/app/src/state.rs` holds the view-facing state (`AppState`). It owns no
  runtime state: every read goes through `RuntimeService::snapshot`, and a
  background tick drains `RuntimeEvent`s and reconciles snapshots every 200 ms
  (full reconcile every fifth tick) as the façade contract requires.
- The window renders the profile list with a state badge, Start/Stop/Restart,
  the Runtime Details panel (PIDs, ports, effective args, last error/warning,
  dropped events) and a copy-args action. Create, start, stop and restart are
  covered by a headless GPUI test that clicks the real buttons.
- On first start one browser core is registered by asking the discovered
  executable for `--version` and parsing its major. Discovery order is
  `FP_BROWSER_CHROMIUM_BIN`, then `bin/chromium`, `bin/chrome`, `chrome`, then
  `PATH`. Phase 4 owns moving real version detection into the runtime crate.
- Closing the last window and the Quit action both exit through `main`, which
  sends `ShutdownAll` and waits up to five seconds for the supervisor to reclaim
  every child process.

Runtime configuration for the window:

| Variable | Purpose | Default |
| --- | --- | --- |
| `FP_BROWSER_CHROMIUM_BIN` | Browser core executable | discovered |
| `FP_BROWSER_CHROMIUM_MAJOR` | Major override when `--version` is unusable | detected |
| `FP_BROWSER_DATA_DIR` | SQLite database, profiles and runtime configs | `data` |
| `FP_BROWSER_XRAY_BIN` | Per-profile proxy binary | `bin/xray` |

```sh
FP_BROWSER_CHROMIUM_BIN=/absolute/path/to/chrome cargo run -p app
```

Known limitations:

- A window destroyed by another X client without the close protocol (for example
  `xdotool windowclose`) may leave the process running with its browsers. The
  supported exits are the window manager's close button and the in-window Quit
  action.
- Terminating the process directly (`SIGTERM`, `SIGKILL`) skips the reclaim path
  for the same reason: only the graceful exit asks the supervisor for
  `ShutdownAll`. Signal handling is not implemented yet.

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
repeat acceptance with fingerprint-chromium. Wiring runtime settings and services
into the GPUI UI has started (see the Phase 5 slice below) but the Proxies,
Browser Cores and Settings pages do not exist yet, so proxy assignment and core
management still happen outside the window.
Phase 4 then adds version detection and fingerprint capability compatibility.
