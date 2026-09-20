# Fingerprint Browser

Personal Rust desktop browser-profile manager built with GPUI Kit, SQLite,
fingerprint-chromium and Xray. Chromium and Xray are external executables;
this repository does not bundle or fork them.

## Current capabilities

- Create, edit, duplicate and delete profiles with independent browser data,
  stable seeds, fingerprint settings, browser cores and proxies. Deleting a
  profile keeps its browser data on disk.
- Start, stop and restart profiles independently; cancel startup during readiness.
  Normal stop requests CDP Browser.close before forcing cleanup, allowing cookies
  and persistent state to be flushed.
- Manage cores and re-detect versions. Unknown majors are refused; unsupported
  fingerprint claims produce warnings. Windows probes manifests and PE metadata.
- Configure SOCKS5/HTTP in a form; import Shadowsocks, VMess, VLESS and Trojan
  links with supported transport/TLS settings. Unsupported link fields are refused.
  Referenced proxies/cores cannot be deleted.
- Run one Xray per proxied profile. Xray must be ready before Chromium starts;
  an Xray crash closes its browser instead of silently allowing direct traffic.
- Read fingerprints back from a running browser and report discrepancies.
  Linux builds 142, 144 and 148 were measured; 145–147 remain inferred.
  See the [fingerprint matrix](docs/fingerprint-matrix.md).
- Show runtime details, effective arguments, per-profile logs and notifications.
  Logs persist to logs/activity.log, rotating at 512 KiB to one backup. In-memory
  history is capped at 500 lines.
- Configure data/Xray paths for the next start; Settings shows effective values,
  their sources and any environment overrides.

## Windows lifecycle

Windows 10 or newer is required for the runtime launcher. Chromium and Xray are
created inside separate Job Objects using PROC_THREAD_ATTRIBUTE_JOB_LIST.
Assignment is atomic with creation; there is no create-then-assign gap.
Only the supervisor owns the non-inherited job handles. Closing them, including
when the manager is forcibly terminated, kills their process trees. Descendants
are also cleaned up after their original parent exits.

Session journals record PID and native creation time. Windows reads GetProcessTimes
to verify identity and distinguishes exited processes from unreadable ones.
Matching older processes can be reclaimed; mismatched PIDs, unreadable processes
and Windows records without creation times are left untouched and reported.
Cleanup failures retain journals/configuration for retry. After a manager crash,
Windows jobs normally have already stopped the children, so the next start removes
stale journals and temporary Xray configuration.

Unix children use independent process groups. Linux recovery uses /proc; other
non-Windows platforms still lack a native process-identity reader.
See [Windows acceptance](docs/windows-acceptance.md) and [lifecycle](docs/lifecycle.md).

## Run

Use stable Rust. Linux needs libfontconfig1-dev and libfreetype-dev. Windows needs
the MSVC Rust toolchain and C++ build tools. Supply your own binary paths:

```powershell
$env:FP_BROWSER_CHROMIUM_BIN = 'C:\tools\fingerprint-chromium\chrome.exe'
$env:FP_BROWSER_XRAY_BIN = 'C:\tools\xray\xray.exe'
cargo run -p app
```

```sh
FP_BROWSER_CHROMIUM_BIN=/path/chrome FP_BROWSER_XRAY_BIN=/path/xray cargo run -p app
```

| Variable | Purpose | Default |
| --- | --- | --- |
| FP_BROWSER_CHROMIUM_BIN | Initial core executable | Local bin candidates, then PATH |
| FP_BROWSER_CHROMIUM_MAJOR | Override when version detection fails | Detected |
| FP_BROWSER_DATA_DIR | Database, profiles, logs and runtime files | Platform application-data directory |
| FP_BROWSER_XRAY_BIN | Proxy executable | bin/xray.exe on Windows; bin/xray elsewhere |

Data defaults to %LOCALAPPDATA%\FpBrowser on Windows, $XDG_DATA_HOME/FpBrowser
or ~/.local/share/FpBrowser on Linux, and ~/Library/Application Support/FpBrowser
on macOS. Legacy ./data is detected and reported, not automatically moved.
Settings are stored outside the data directory so changing it cannot lose the
setting that chose it. Environment values override saved settings.

## Validation

```powershell
# Windows: fmt, check, tests and strict clippy
./scripts/check.ps1
```

```sh
# Linux: the same four checks
./scripts/check.sh
```

CI is configured to run the gate on both Ubuntu and Windows. Windows controlled
process tests require no browser/Xray download. Linux lifecycle fixtures require
Python 3 and loopback sockets. Real-engine acceptance is opt-in, separate from the gate:

```powershell
$env:CHROMIUM_BIN = 'C:\tools\fingerprint-chromium\chrome.exe'
$env:XRAY_BIN = 'C:\tools\xray\xray.exe'
cargo test -p runtime --test windows_real -- --ignored --test-threads=1
```

```sh
CHROMIUM_BIN=/path/chrome XRAY_BIN=/path/xray \
  cargo test -p runtime --test chromium_real -- --ignored --test-threads=1
CHROMIUM_BIN=/path/chrome \
  cargo test -p runtime --test fingerprint_real -- --ignored
XRAY_BIN=/path/xray cargo test -p runtime --test xray_real -- --ignored
```

## Limits and next steps

- Forced termination cannot guarantee cookie/state flushing. CDP close remains
  the normal stop path; job cleanup protects crash and forced-stop paths.
- Windows real tests cover profile isolation, cookies, cancellation, normal close,
  browser/Xray crashes, manager termination and stale-journal cleanup. Remote
  upstream forwarding and Windows fingerprint measurements remain separate work.
- Four proxy protocols enter through links rather than complete form editors.
- The X11 backend may keep the manager alive if another client destroys its window
  without the close protocol. Linux recovery handles leftovers on the next start;
  there is no X11 watchdog.
- CDP/SOCKS ports are reserved until spawn, but external executables bind their own
  sockets. Release-to-bind is not atomic; readiness detects startup failure.
- Runtime events are bounded and best-effort; snapshots are authoritative and the
  UI reconciles periodically. CDP is loopback-only, not an automation API.

Next: proxy diagnostics, Windows fingerprint acceptance, profile search/batch
operations, backup/restore and packaging. See the [current plan](docs/implementation-plan.md).

## Design and evidence

- [Architecture](docs/architecture.md), [domain model](docs/domain-model.md),
  [service contracts](docs/service-contracts.md), [lifecycle](docs/lifecycle.md)
- [Linux acceptance](docs/chromium-acceptance.md), [Windows acceptance](docs/windows-acceptance.md),
  [fingerprint matrix](docs/fingerprint-matrix.md)
- [Current plan](docs/implementation-plan.md), [historical batches](docs/implementation-history.md)
