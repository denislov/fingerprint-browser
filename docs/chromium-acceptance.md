# Linux Chromium runtime acceptance — 2026-09-19

## Environment

- User-supplied `ungoogled-chromium-148.0.7778.215-1-x86_64_linux.tar.xz`;
  executable reports `Chromium 148.0.7778.215`.
- A second build was accepted on 2026-09-20 to check the capability table
  against a legacy major: `ungoogled-chromium-142.0.7444.175-1-x86_64_linux.tar.xz`
  (`Chromium 142.0.7444.175`, the last release below the pivot). It runs the same
  `fingerprint_real` suite, which resolves the major from the binary itself; the
  readings are recorded in `docs/fingerprint-matrix.md`.
- Xray 26.2.6, copied from `Ant-Browser/bin/linux-amd64/xray`.
- Linux x86_64, headless Chromium with its sandbox enabled.
- Binaries are extracted/copied outside the repository and are not committed.

## Repeatable test

```sh
CHROMIUM_BIN=/absolute/path/to/chrome \
XRAY_BIN=/absolute/path/to/xray \
cargo test -p runtime --test chromium_real -- --ignored --nocapture
```

The test uses the real RuntimeSupervisor, launch planner, process controller,
CDP probe and Xray builder. A planner adapter adds headless test flags. Temporary
profiles are removed after shutdown. The event receiver is intentionally absent,
so lifecycle assertions use runtime snapshots.

## Verified

- Two profiles run simultaneously with distinct PIDs, data directories and CDP
  ports. Their effective arguments contain distinct seeds.
- CDP and the local SOCKS inbound listen on IPv4 loopback.
- A persistent cookie written through CDP in profile A is absent in profile B.
- Stopping A leaves B running; restarting A restores the cookie.
- Chromium loads a page through its per-profile Xray and a local HTTP CONNECT
  upstream requiring authentication. The fixture verifies authentication and
  the request path; CDP verifies the returned `OK` text rendered in the page.
- Killing Xray stops its browser, records the crash and removes temporary
  credentials. No live process remains in the browser process group.
- Unexpected browser termination is detected and its group is reclaimed.
- ShutdownAll closes the remaining browser and reclaims its process group.

### Added 2026-09-20: cancellation and reclaim

- A start cancelled while a real browser is waiting for CDP readiness leaves
  nothing running and no session record behind. The test moves the debugging
  endpoint off the port the supervisor probes, so the browser is up and the start
  is provably still inside the readiness wait when the stop arrives.
- A browser left running by a run that was killed is stopped from its session
  record alone, and the record and temporary config are removed with it. The test
  spawns a real Chromium the way the launch planner would, writes the record the
  way the supervisor does, and drops the handle: nothing in the test process is
  holding the browser when the reclaim runs.
- Walked through the window as well, on the same build: a profile was started,
  the app was killed with `SIGKILL`, and the next start reclaimed the browser
  (`INFO app: a previous run left 1 browser session running; Profile 1 (browser
  pid 496461)` and the same sentence in the banner). With a browser running,
  `SIGTERM` on the app made it exit through `ShutdownAll`, leaving no browser and
  no record behind.

### Defects found by that acceptance

- Chromium overwrites `/proc/self/cmdline` with a single string holding the whole
  command line, so the arguments a browser was started with are not separate
  fields any more. Identifying a recorded process by comparing its argument
  vector tail refused to reclaim a browser that was plainly ours. Identity is now
  the kernel start time of the same pid, with the command line as the fallback
  when a start time cannot be read - in both shapes.
- The acceptance probe looked for a profile directory as a bare word, but a
  browser holds it as `--user-data-dir=<dir>`. "No browser is left" was passing
  without looking at anything; it now matches the flag.

## Defect found and fixed

The initial run lost a freshly set persistent cookie after Stop/Start. Stop used
to kill Chromium immediately, before its cookie store flushed. Normal stop,
restart and shutdown now request `Browser.close` over loopback CDP, allow up to
two seconds for exit after the close request, then force-clean remaining group
members. If CDP is unavailable, cleanup falls back to force termination. Crash
and startup rollback paths continue to force termination without waiting for a
graceful close. Cookie persistence cannot be guaranteed after a crash or fallback
kill.

## Limits

This certifies the tested Linux runtime path with **ungoogled-chromium**, not
fingerprint spoofing: passing a `--fingerprint` argument does not prove that this
browser implements it. What the fingerprint layer claims is certified separately,
by reading the surface back out of the fingerprint-chromium builds (see
`docs/fingerprint-matrix.md`), not by this report.

Remote proxy endpoints, Windows/macOS process-tree cleanup and visible GPUI
operation remain separate acceptance work. Process identity for reclaim is read
from `/proc`, so that path is Linux-only today: on a platform that cannot be
asked, records are reported as unreadable and nothing is killed.
