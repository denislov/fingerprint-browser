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
fingerprint spoofing. Passing a `--fingerprint` argument does not prove that this
browser implements it. Fingerprint-chromium capability detection/certification,
remote proxy endpoints, visible GPUI operation, Windows/macOS cleanup and real
browser startup cancellation remain separate acceptance work. Startup cancellation
is currently covered by the controlled-child regression suite.
