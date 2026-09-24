# Windows process lifecycle acceptance

Measured 2026-09-24 on Windows 11 Pro x64, build 10.0.26200, Rust 1.98.1 MSVC.
Browser: local fingerprint-chromium 148.0.7778.215 (`chrome.exe` PE metadata).
Proxy engine: Xray 26.2.6, commit 12ee51e, windows/amd64, Go 1.25.7.
No binaries are added to the repository.

Local Windows gate passed: `scripts/check.ps1` completed formatting, workspace
check, **669 passing tests** and strict Clippy (`-D warnings`). Real Windows
acceptance was run separately:
- `windows_real`: **4 passed** (isolation, cancellation, crash cleanup, subprocess fixture)
- `xray_real`: **3 passed** (links parsing, schema coverage, authenticated local forwarding)
- `fingerprint_real`: **11 passed** (canvas, audio, clientrects, font exclusions, WebRTC policy, capability table verification)

## Implementation

- Chromium and Xray each have an unnamed, non-inheritable Job Object configured
  with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
- `CreateProcessW` uses `PROC_THREAD_ATTRIBUTE_JOB_LIST`, so assignment happens
  as part of creation. There is no suspended child waiting for a later assignment.
  Failure to create/attach fails startup; there is no uncontained fallback.
- The handle inheritance list contains only NUL for standard streams. The job
  handle stays with the manager. No helper console windows are opened.
- Stop first uses the existing graceful CDP close. Forced cleanup terminates the
  job even when the initial browser/Xray process has already exited. Killing the
  manager also closes the job handles, letting Windows clean up their members.
- PID + `GetProcessTimes` creation time identifies records. A process handle is
  held during legacy `taskkill /T` recovery and identity is rechecked before
  termination. Errors are not silently treated as successful cleanup.
- A surviving or unidentifiable process retains its journal and temporary proxy
  configuration. Windows records written without creation time remain unresolved;
  they are never killed just because their PID exists.

The native launcher is internal to the runtime: executable, ordinary arguments,
current directory and environment overrides are supported; all streams go to NUL.
It does not expose shell commands, raw arguments or arbitrary Command extensions.
Windows 10+ is required by the creation-time job-list attribute.

API basis: [Microsoft Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
and [atomic process/job creation](https://devblogs.microsoft.com/oldnewthing/20230209-00/?p=107812).

## Controlled tests (ordinary gate)

```powershell
cargo test -p runtime process::windows
```

Six substantive cases plus one subprocess fixture:

1. Drop the job after its leader exits; its still-running descendant exits.
2. Kill a separate manager process; its parent/leaf tree exits and an unrelated
   process remains alive.
3. Creation time is available immediately and stable; an exited handle is absent,
   while an unqueryable identity is unknown.
4. Reclaim a matching session; reject a changed creation time and a legacy record
   without one. Repeat journal writes to exercise Windows replacement behavior.
5. Inject termination failure; retain the journal, then clear it after real exit.
6. Round-trip Unicode, spaces, empty arguments, quotes and trailing backslashes
   through the native process launcher into a real child.

## Real Chromium/Xray tests (opt-in)

```powershell
$env:CHROMIUM_BIN = 'C:\tools\fingerprint-chromium\chrome.exe'
$env:XRAY_BIN = 'C:\tools\xray\xray.exe'
cargo test -p runtime --test windows_real -- --ignored --test-threads=1
```

Passed: three substantive scenarios plus one internal subprocess fixture:

- Two profiles run with distinct browser PIDs, CDP ports and data directories.
  A persistent cookie belongs only to its profile and survives normal stop/start.
  Stopping one profile leaves the other running.
- Real Xray reaches readiness. Killing Xray closes its Chromium, reports the
  Xray failure and removes its temporary configuration. Normal browser close is
  reported as stopped without an error; forced browser termination records an
  error. Shutdown closes the remaining session.
- A real browser held in CDP readiness can be cancelled; its process and journal
  are removed. A separate manager process running real Chromium can be forcibly
  killed; its browser exits while another manager's browser survives. A subsequent
  reclaim clears the stale record.

The manager-crash test runs the production supervisor in a separate process;
it does not claim a manual GPUI window/Task Manager walkthrough.

## Scope and limits

- Controlled lifecycle tests and real tests verify descendant cleanup, session
  PIDs, state, configuration and cookie behavior. Real fingerprint measurements
  on Windows (11 tests in `fingerprint_real`) were also verified against local
  Chromium 148, confirming canvas, audio, clientrects, WebRTC and font exclusions.
- The Xray lifecycle case uses a local upstream placeholder and does not claim
  remote proxy forwarding. Remote upstream acceptance is still outstanding.
- Controlled tests verify descendant cleanup; real-browser tests additionally
  verify session PIDs, state, configuration and cookie behavior.
- Forced termination cannot guarantee persistence. Normal stop retains the CDP
  flush opportunity; crash/job cleanup prioritizes ending the session.
- CI is configured for Ubuntu and Windows, with controlled tests only. Hosted CI
  results must be obtained from a run; adding the matrix does not certify it.
