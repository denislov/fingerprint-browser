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
  `FingerprintGeneration::PIVOT_MAJOR` (144) into a legacy and a verified
  generation, and both carry the stable set (seed, brand, platform, both version
  switches, language, timezone, hardware concurrency and the WebRTC policy).
  The two switches that are not stable were measured on **two** builds and sit on
  opposite sides of the pivot: `--fingerprinting-canvas-image-data-noise` is
  honoured by 142 and 148 alike, while `--disable-spoofing` is honoured by 148
  and ignored by 142. See [the switch matrix](docs/fingerprint-matrix.md) for the
  readings and for which majors are still inherited rather than measured.
- Nothing is dropped in silence. A switch the core cannot honour is omitted at
  serialization time and reported by `runtime::compat::check`, which the
  supervisor turns into a `Warning` event; it lands in
  `RuntimeSnapshot::last_warning`, on the profile row and in Runtime Details.
  An undetected version (major `0`) is refused instead of being resolved to an
  assumed capability set. That includes `--disable-spoofing`, which used to be
  emitted for every generation and reported nowhere.
- A switch name is not evidence. `CdpSession` opens the browser's debugging
  endpoint on loopback and `FingerprintProbe` reads the fingerprint back out of
  a real page; `runtime::verify` compares that reading with the profile and
  reports every claim it does not support (a missing reading is itself a
  finding, with the probe's own error when it has one). The reading covers the
  canvas, the audio fingerprint, ICE candidates, the font enumeration and the
  glyphs they render, the platform, the user agent data brands and the locale.
  Measuring the verified generation this way found three switches this project
  was emitting that the engine silently ignores: `mac` instead of `macos`,
  `client-rects` instead of `clientrects`, and two brands no build honours. All
  three are fixed and covered by
  [the switch matrix](docs/fingerprint-matrix.md).
- Two of those surfaces can only be judged by comparing sessions, so `verify`
  states its limits instead of guessing: the audio and canvas fingerprints and
  the WebGL exclusion are exposed as signatures for comparison, while the leak
  check and the three font readings are settled by a single measurement. Those
  three are whether the font surface can be read at all (an enumeration and a
  width, and the finding says which half is missing), whether CJK has glyphs,
  and whether emoji do - the last one catches a host with no emoji font, which
  any page can see and no profile claim can explain. The WebRTC check is proven
  able to see a leak by a self-check that re-opens the ICE policy in a separate
  session.
- The font list is the one surface a platform spoof cannot render: the engine
  fabricates which fonts the page can enumerate, while the glyphs come from the
  host either way. Measuring that (Windows and macOS claims on a Linux host, with
  and without `--disable-spoofing=font`, identical widths in all six readings)
  is why this project does **not** add `font` to the exclusion when the profile
  claims another platform - it would replace a fabricated list with the host's
  own and admit the host. The exclusion remains a per-profile choice. See
  [the switch matrix](docs/fingerprint-matrix.md) for the readings.

```sh
# fingerprint surface, read back out of a real browser
CHROMIUM_BIN=/absolute/path/to/chrome cargo test -p runtime --test fingerprint_real -- --ignored
# process lifetime, proxies, cookie isolation
CHROMIUM_BIN=/absolute/path/to/chrome XRAY_BIN=/absolute/path/to/xray cargo test -p runtime --test chromium_real -- --ignored
```

Phase 5 also has its first slice: the GPUI window is wired to the services
instead of being a static mockup, and it can verify a running profile: the
Runtime Details panel reads the fingerprint back out of the live browser in a
tab of its own and reports every claim the browser did not reproduce, with a
per-row badge so the answer is visible without selecting anything. A reading
that fails is reported as unreadable, never as confirmed.

Settings has a page of its own, and it is mostly a report: every setting is
listed with the value in force and where that value came from - the environment,
the config file, a default, or another setting it is derived from. An
environment variable wins over the config file and the row says so, naming the
stored value it is overriding, because a setting that looks saved and does
nothing is worse than no setting.

Two settings can be changed from the window: the data directory and the Xray
executable. Both decide what the *next* start does, so they are stored in a
config file outside the data directory - keeping them inside would mean that
changing the data directory moves the database and loses the setting in the
move. A config file that cannot be read is reported rather than treated as
empty, and saving is refused until it is fixed, so a typo cannot cost the
settings it still holds.

Browser cores have a page of their own. A core is added by pointing at a
fingerprint-chromium binary; the version is read from it with `--version` rather
than typed, because the detected major is what decides which switches a profile
may claim, and each row says which generation it belongs to and whether the
noise switches were verified for it. A binary that does not report a version is
refused while the form is open, with the environment override named as the way
to record it anyway. Re-pointing a core at another binary, or pressing
Re-detect after replacing one in place, re-reads the version instead of keeping
the old major; a core whose version was never read has no capability table and
cannot be launched or verified with. Removing a core that a profile still uses
is refused by name.

Proxies have a page of their own. A proxy can be created, edited and deleted,
and assigned to a profile from the profile editor; the row says which profiles
use it. Only SOCKS5 and HTTP can be created, because those are the two outbounds
the config builder can turn into a working config - the other four protocols the
model can store are named in the form and refused, rather than stored and left to
fail at launch. A proxy that a profile still points at cannot be deleted: the
schema would null the assignment out and send that traffic direct, so the refusal
names the profiles that hold it.

Profiles can be edited, duplicated and deleted from the window. The editor is a
form over the profile's fingerprint and window: name, seed (with a re-roll),
brand and its version, platform and its version, language, accept language,
timezone, CPU cores, window size, the WebRTC policy and which spoofing features
to exclude. Saving runs the same domain rules storage enforces and keeps the
dialog open with the reason when one fails; fields the form does not edit (core,
proxy assignment, data directory, start target) are carried through untouched.
Deleting asks first and keeps the profile's browser data on disk.

Feedback is a toast, and what happened is a page. A success is a toast and
leaves the banner clear; a problem owns the banner until it is dismissed, and is
toasted while it happens, because the banner is the current problem rather than
every problem ever seen. Either way the line is kept: the Log page lists starts
and stops with the pids, ports and argument count they got, warnings, crashes,
refused commands and the outcome of every fingerprint reading, newest first,
attributed to the profile it is about or to `app` for the window itself. The
page has Copy, Clear and an All / Warnings / Errors filter, and says which kind
of empty it is showing when the filter hides everything. The history is capped at
500 lines so a long session does not grow without bound.

The same lines are also written to `logs/activity.log` under the data directory,
with an absolute UTC timestamp and the profile named rather than identified, so
the record outlives the window; when that file passes 512 KiB it is rotated to
`activity.log.1`, which the next rotation replaces, so there are at most two
files and nothing grows without bound. The page says which file it is writing to,
and a directory it cannot write to is reported at startup rather than at the
first line someone needed; the first write that fails is reported once, as a
toast, and never per line. Toasts are pushed from the background
tick into the component library's notification layer, not from the click
handler, so a warning that arrives on its own - a legacy core omitting a switch,
a browser that crashed - is shown the same way a button press is. The data
directory of the selected profile can be opened in the desktop's file manager
(`xdg-open`, `open` or `explorer`) from Runtime Details; a profile that has never
run has no directory yet, and the refusal names the path instead of creating an
empty folder that looks like state. A relative data directory is resolved
against the working directory first, the same way the Settings page shows it,
because the opener is a foreign process that may not share this one's. The opener
is run on a worker and its result is reported, because a spawn is not an open: an
exit of 0 means something took the request, a non-zero exit is reported with the
program that failed - a desktop with no handler is exactly that - and an opener
still running after five seconds is a file manager that stays in the foreground,
which is an open too.

- `crates/app/src/state.rs` holds the view-facing state (`AppState`). It owns no
  runtime state: every read goes through `RuntimeService::snapshot`, and a
  background tick drains `RuntimeEvent`s and reconciles snapshots every 200 ms
  (full reconcile every fifth tick) as the façade contract requires. The tick is
  also where drained events - which were previously only a reason to re-read the
  snapshots - are turned into log lines and toasts.
- The window renders the profile list with a state badge, Start/Stop/Restart,
  the Runtime Details panel (three views: identity and diagnostics, the launch
  line, and this profile's own log tail) and a fingerprint verification action.
  Create, start, stop and restart are covered by a headless GPUI test that clicks
  the real buttons; the toast layer is asserted through the window's own
  notification list rather than by waiting for the timer.
- On first start one browser core is registered by asking the discovered
  executable for `--version` and parsing its major. Discovery order is
  `FP_BROWSER_CHROMIUM_BIN`, then `bin/chromium`, `bin/chrome`, `chrome`, then
  `PATH`. Phase 4 owns moving real version detection into the runtime crate.
- Closing the last window and the Quit action both exit through `main`, which
  sends `ShutdownAll` and waits up to five seconds for the supervisor to reclaim
  every child process. `SIGINT`, `SIGTERM` and `SIGHUP` take the same exit: the
  handler writes one byte to a self-pipe and a thread of its own asks the
  supervisor for `ShutdownAll`, so a service manager stopping this process, a
  terminal going away and the Quit button all reclaim the browsers it started.
- Every running session writes a record next to its temporary Xray config
  (`data/runtime/<profile-id>/session.json`), naming the browser, the Xray, their
  ports, the arguments they were started with and the kernel start time of each.
  A run that is killed outright - `SIGKILL`, a crash, a window destroyed without
  the close protocol - leaves those browsers running and that record behind; the
  next start reads it back before the window opens and stops what it finds: a
  clean reclaim is a toast and a Log line, and a record that could not be
  honoured is a banner, which stays. Nothing is killed on a pid alone: the
  record is only acted on when the live process is still the instance it
  captured (same pid and
  kernel start time), or - where a platform cannot report a start time - when its
  command line still carries the recorded arguments as the tail of the argument
  vector, or as the end of the single string a process like Chromium leaves
  behind when it rewrites `/proc/self/cmdline`. A record that cannot be
  identified is reported and left alone, its process and its temporary config
  included. `RUST_LOG=info` is what shows the reclaim; the subscriber otherwise
  reports errors only.

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
  `xdotool windowclose`) may leave the process running, and it will not run the
  exit path a signal or the Quit action runs. This is upstream and measured: the
  X11 backend does not handle `DestroyNotify` for its own windows, and it keeps
  using the destroyed window - the process log fills with `X11 QueryPointer
  failed ... bad_value: <the gone window>` while nothing exits. A watchdog would
  mean a second X11 connection of our own and a new dependency, to save one
  process lifetime; what it would protect is already protected, because the
  session record and the next start's reclaim are what keep the browsers from
  being lost (and that reclaim is exercised by `chromium_real`). What it leaves
  behind is no longer lost, though: the next start finds the session records and
  reclaims it.
- Reclaiming trusts a Linux-only reader for process identity (`/proc/<pid>/stat`
  and `/proc/<pid>/cmdline`). On a platform that cannot be asked, records are
  reported as unreadable and nothing is killed, rather than guessed at.

## Validation and next steps

```sh
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Strict `-D warnings` is the gate. The workspace used to carry a
`.cargo/config.toml` that set `RUSTC_BOOTSTRAP` and injected
`feature(cold_path, atomic_try_update)` into every crate; both features have been
stable since Rust 1.95, no crate in the workspace uses them, and nothing in the
dependency graph needs a nightly compiler. The file is gone and the checks above
run clean on a stable toolchain (verified on rustc 1.96.0).

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
authenticated local proxy forwarding, crash recovery and shutdown, plus the two
paths that used to end with a browser nobody knew about:

```sh
# a start cancelled while a real browser is waiting for CDP readiness
# a real browser left running by a killed run, stopped from its session record
CHROMIUM_BIN=/absolute/path/to/chrome \
XRAY_BIN=/absolute/path/to/xray \
cargo test -p runtime --test chromium_real -- --ignored --test-threads=1
```

Both were also walked through the window by hand: start a profile, `kill -9` the
app, and the next start reclaims the browser and says so in a toast and in the
Log page; with a
browser running, `kill -TERM` the app and it exits with nothing left behind. See
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
into the GPUI UI is done: profiles, their fingerprint fields, proxies, browser
cores and the settings that need a restart are all managed from the window, with
the pages below describing what each one refuses to do.
Phase 4 then adds version detection and fingerprint capability compatibility.
