# Fingerprint Browser

<img src="assets/icon.png" alt="" width="96" align="right">

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
- Test a proxy with one real request and report the address it left from, or the
  class of failure: engine, configuration, authentication, name resolution,
  unreachable, timeout, TLS, HTTP status or unreadable answer. A proxy that is
  already carrying a profile's traffic is probed through that engine; otherwise a
  temporary engine is started for the test alone.
- Read fingerprints back from a running browser and report discrepancies, and read
  back the address that browser's own traffic leaves from.
  Linux builds 142, 144 and 148 were measured; 145–147 remain inferred.
  See the [fingerprint matrix](docs/fingerprint-matrix.md).
- Show runtime details, effective arguments, per-profile logs and notifications.
  Logs persist to logs/activity.log, rotating at 512 KiB to one backup. In-memory
  history is capped at 500 lines.
- Filter the profiles list by name, seed, brand, platform, core or proxy. The
  filter only narrows what is listed: a hidden profile keeps running and stays the
  one the details panel answers for. There is deliberately no multi-select or batch
  operation - see
  [why](docs/implementation-plan.md#why-profile-search-stays-a-single-profile-feature).
- Export the configuration - cores, proxies and profiles - to a JSON file, and
  import one back, for another machine or for keeping. Proxy credentials are left
  out unless asked for, the export says which file it wrote and whether it carried
  any, and the import never overwrites: taken identifiers are kept, profiles whose
  core is missing are skipped, and the report says which was which. Browser data
  is not included. See [backup and restore](docs/backup-and-restore.md).
- Restore makes the installation *be* the file instead of adding to it: an empty
  installation is restored at once, a populated one asks before replacing
  anything, and a running profile blocks it either way. Copy each profile's
  browser data out to a directory of your own and back in; only stopped profiles
  are copied, a profile that has never run is skipped and named, and the copy
  reports how many bytes it wrote.
- Configure the Xray path for the next start; Settings shows effective values,
  their sources and any environment overrides. One directory holds the whole
  installation - the config file lives in the data directory beside the database,
  the logs and the profiles - and the data directory itself is chosen by
  `FP_BROWSER_DATA_DIR` or the platform, not from inside the file it holds.
- Choose the window's appearance: **Dark**, the palette the program has always
  painted, or **Light**. The window repaints as soon as the choice is made - it is
  the one setting that does not wait for the next start - and the choice is kept
  in the config file.
- Choose the window's language: **English** or **简体中文**, labelled in their own
  language. Both switches change the running window at once, and both are kept in
  the config file. Identifiers the window is addressed by do not change with the
  language, and the faults reported by the engine's own layers stay in English;
  see [interface text](docs/i18n.md).
- Say what it is without opening a window: `--version` prints the version, the
  commit it was built from and the platform, `--help` prints the options in the
  language you chose, and `--diagnostics` writes one file about this installation
  that never contains proxy credentials or browser data. The same report is one
  button on the Settings page; see [diagnostics](docs/diagnostics.md).

## Getting started

This program does not download a browser or a proxy engine; you supply both. A
first run therefore has one step before anything can be launched: add a
fingerprint-chromium binary on the **Browser Cores** page, or start the program
with `FP_BROWSER_CHROMIUM_BIN` pointing at one. The empty profile list says so
and carries the button that leads there. Xray is only needed for a profile that
uses a proxy.

The whole walk - what the window says at each step, where the files go, and what
to look at when something is wrong - is in [first run](docs/first-run.md).

## Proxy testing

An open loopback port is not a working proxy. The launcher only ever learns that
the engine bound its local inbound; a proxy that refuses the upstream
credentials, cannot resolve the target name or simply carries nothing passes
that check. The profile then presents one fingerprint and is seen from another
address, which is the failure this project exists to prevent.

Testing a proxy sends one real HTTP request through the engine and reports the
address the endpoint saw. The endpoint is plain HTTP on purpose: over TLS a
certificate problem would be reported as a proxy problem, and the first question
is whether a byte left at all. An `https://` endpoint is refused rather than
downgraded. The endpoint is configurable because it is the one part of this that
sees the exit address.

| Result | What it establishes |
| --- | --- |
| An address | A request left through this engine and reached the endpoint, from that address |
| A fault | Where it stopped: engine, configuration, authentication, name resolution, unreachable, timeout, TLS, HTTP status, unreadable answer |
| Neither | That a browser was pointed at this engine, or that a profile's remaining traffic takes the same path. Reading a fingerprint back is the neighbouring check, not a substitute |

When a profile using the proxy is running, its engine is probed: that is the path
the traffic is actually taking, and it is never stopped or restarted by a test -
whoever started it stops it. Otherwise a temporary engine is started for the test
alone and removed when the test ends, including when it fails; its config, which
holds the upstream credentials, goes with it. The test runs only when the user
starts it, and never in the background.

### Where a browser's own traffic leaves from

Testing a proxy establishes what the *proxy* does. It cannot establish that a
*browser* was pointed at it, and the two come apart where it matters: a launch
flag that was ignored, a profile with no proxy, another proxy in between. So
verifying a profile that uses a proxy also asks the browser itself - in a tab of
its own, which is closed again - to fetch the endpoint and report the address it
saw.

Four outcomes are kept apart, because they are fixed in different places:

| Reading | What it means |
| --- | --- |
| An address | The browser reached the endpoint and left from there |
| Never reached | The page never committed to the endpoint: nothing left by this route |
| Not finished | The page was at the endpoint and had not loaded in time: slow, not blocked |
| No address in the answer | The endpoint answered with something that is not an address |

"Never reached" and "not finished" are read from the page rather than inferred
from a timeout, so a failed navigation is reported at once instead of after a
deadline. When the proxy has been tested, the reading is compared with the
address the proxy was measured at, and a difference is reported as a claim the
profile did not reproduce. Without a test there is nothing to disagree with, and
the address is reported rather than judged. Only a profile with a proxy is asked:
a direct profile claims nothing about an address, and the endpoint sees the
address it is asked from.

The address is recorded in the activity log. It is not claimed to be where pages
you open in that browser go: this reads one document, in one tab, once.

The read-back's own orchestration is covered in the gate without a browser: a stub
CDP endpoint speaks both of the protocols a browser does on one port, so opening
the page, polling the document, reading the answer and closing the page are all
tested. Only the real browser's behaviour is left to the opt-in run below.

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

The program itself is `fingerprint-browser`. It takes no argument to open the
window, and three options that answer a question and exit: `--version`, `--help`,
and `--diagnostics`, which writes one file about this installation - the build,
the effective settings and where each came from, the state and permissions of
every file it keeps, and the end of the activity log - with `--out <path>` to say
where. The report never opens the database and never reads browser data; see
[diagnostics](docs/diagnostics.md). The version is also in the window's title bar
and is the first line of the activity log.

| Variable | Purpose | Default |
| --- | --- | --- |
| FP_BROWSER_CHROMIUM_BIN | Initial core executable | Local bin candidates, then PATH |
| FP_BROWSER_CHROMIUM_MAJOR | Override when version detection fails | Detected |
| FP_BROWSER_CONFIG | Config file path, for a test or a script | `<data dir>/config.json` |
| FP_BROWSER_DATA_DIR | Database, profiles, logs, runtime files, config file and exports | Platform application-data directory |
| FP_BROWSER_ECHO_URL | Address endpoint a proxy test asks | http://api.ipify.org |
| FP_BROWSER_XRAY_BIN | Proxy executable | bin/xray.exe on Windows; bin/xray elsewhere |

Data defaults to %LOCALAPPDATA%\FpBrowser on Windows, $XDG_DATA_HOME/FpBrowser
or ~/.local/share/FpBrowser on Linux, and ~/Library/Application Support/FpBrowser
on macOS. One directory is the whole installation: the config file lives in the
data directory beside the database, the logs and the profiles, and the data
directory itself is chosen by `FP_BROWSER_DATA_DIR` or the platform. Legacy ./data
is detected and reported, not automatically moved; a config file left in the old
per-platform location is folded into the data directory once, at the next start.
Environment values override saved settings, and the Settings page says which
value won.

For a build to give to someone else, [releases](docs/release.md) has the version
policy, what `[profile.release]` does and why, how the icon is generated, and
what packaging still has to cover. There is no released artifact yet.

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
# Reaches a real endpoint through a real Xray; needs a working proxy and network.
XRAY_BIN=/path/xray PROXY_URI='socks5://host:port' \
  cargo test -p runtime --test proxy_diag_real -- --ignored
# Reads a real browser's own exit address back; needs a browser and network.
CHROMIUM_BIN=/path/chrome ECHO_URL=http://api.ipify.org \
  cargo test -p app real_chromium_reports_the_address -- --ignored
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
  Readiness is still only about the local port, so what a launch proves is unchanged:
  a proxy that carries nothing passes it, and testing the proxy is what tells the
  two apart. Testing the proxy does not prove the browser was pointed at it, which
  is why the exit address is read back out of the browser as well.
- Runtime events are bounded and best-effort; snapshots are authoritative and the
  UI reconciles periodically. CDP is loopback-only, not an automation API.

Next: Windows fingerprint acceptance, and distribution - a repeatable Windows
release build, first-run executable setup and packaging. The backup and restore
work is complete; see the [current plan](docs/implementation-plan.md). A profile
filter is in; batch operations and multi-select are not planned, for the reasons
recorded there.

## Design and evidence

- [Architecture](docs/architecture.md), [domain model](docs/domain-model.md),
  [service contracts](docs/service-contracts.md), [lifecycle](docs/lifecycle.md)
- [Linux acceptance](docs/chromium-acceptance.md), [Windows acceptance](docs/windows-acceptance.md),
  [fingerprint matrix](docs/fingerprint-matrix.md)
- [Backup and restore](docs/backup-and-restore.md) - export, import, restore and
  the browser-data copy are all in
- [Current plan](docs/implementation-plan.md), [historical batches](docs/implementation-history.md)
