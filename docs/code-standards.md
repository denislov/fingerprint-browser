# Code standards

These rules come from what PR reviews keep asking for. A reviewer checks each one, and a PR that doesn't meet one gets a comment saying which. How to describe and review a change to a subsystem is in the [subsystem references](subsystems/README.md).

## What a change contains

| Rule | What a reviewer looks at |
| --- | --- |
| A change does only what its goal needs. | Every change in behavior has a reason. Unrelated refactors, extra features and policy changes go in a PR of their own. |
| A fix covers exactly the cases it means to. | Which cases change, and which neighboring ones stay the same. Different errors are not handled as one. |
| Checks cover the edges the change touches. | For numbers, check the boundaries. For paths, check each platform a release targets (Windows, Linux); the CI matrix and the two packaging scripts cover them. For concurrency, check races and stale state. Pick checks by risk. |

## Tests

| Rule | What a reviewer looks at |
| --- | --- |
| A fix comes with a test that fails without it. | Put the old code back and run the new test; it must fail with a real `FAIL`, not a compile error or a skip. The change says which test failed and what it printed. |
| The gate never reaches the network. | `cargo test --workspace` sends nothing beyond loopback. Anything that dials GitHub, downloads a core or an engine, or launches a real browser is `#[ignore]`d with its requirement in the reason, and runs under [Acceptance criteria](#acceptance-criteria), not in the gate. |
| A test that needs a tool the gate cannot assume says so. | The runtime fixtures drive the supervisor against a `python3` stub playing the engine and a Rust `CdpProbe` double; the single-instance test prints `python3 is not installed; skipping` where python3 is missing (README). The opt-in suites name their binary in the `#[ignore]` reason and their `*_BIN` variable. |
| UI tests render headless. | The window's tests drive the real view through GPUI test contexts (`TestAppContext`); CI installs no display and needs none, so a change to the view is checked there, not by running the window. |
| Whatever a test starts ends when it does. | The supervisor fixtures call `cleanup_all()` and remove their temporary directory in `Drop`; the single-instance test kills and reaps its process. Nothing a test started may write into the next test. |
| Tests leave the real installation alone. | Every fixture works in a temporary directory of its own under the system temp dir (`fp-runtime-<id>`, `fp-app-acceptance-<id>`); nothing reads or writes the user's data directory. |
| A test that waits for state waits on a deadline. | Poll against a bounded deadline and fail naming the state last seen (`wait_for` in the app harness), instead of sleeping a fixed time. |
| A test that fails on `main` is a bug to fix now. | Rerun it there (`cargo test --workspace`); if it fails, fix it at its cause or file it, in a commit of its own, before shipping on top of it. |

## Acceptance criteria

A change is merged or released only when it shows each of the following.
Say which ones were run and what they printed. A check that wasn't run is
listed as not run, never left out.

1. **The gate, named.** Run `./scripts/check.sh` on Linux or `./scripts/check.ps1` on Windows: `cargo fmt --all --check`, `cargo check --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` (the shell gate also parses the packaging scripts and reads a changelog section). Say which one ran and what it printed; the pass line is the one starting `gate passed:`. CI runs the same scripts on every push and pull request, on `ubuntu-latest` and `windows-latest`, with the compiler `rust-toolchain.toml` pins. A green run on one platform is not a check of the other.
2. **A test that fails without the change.** Put the old code back and run the new test. It must fail with a real `FAIL` that names the behavior - a compile error or a skipped test doesn't count. Say which test failed and what it failed with, then restore the change and check it passes.
3. **The opt-in suites, when the change touches the path they cover.** They are `#[ignore]`d, never part of the gate, and each needs its binaries, its upstream and the network. Run the ones the change makes relevant; every suite not run is named with its reason (no browser, no engine, no upstream, no machine):
   - `CHROMIUM_BIN=... XRAY_BIN=... cargo test -p runtime --test chromium_real -- --ignored --test-threads=1`
   - `CHROMIUM_BIN=... cargo test -p runtime --test fingerprint_real -- --ignored`
   - `XRAY_BIN=... cargo test -p runtime --test xray_real -- --ignored`
   - `XRAY_BIN=... DIAG_SOCKS5='host:port[:user:password]' DIAG_ECHO_URL=http://... cargo test -p runtime --test proxy_diag_real -- --ignored` (`PROXY_URI` takes a share link - `ss://`, `vmess://`, `vless://`, `trojan://`; a plain SOCKS5 upstream goes in `DIAG_SOCKS5`)
   - `CHROMIUM_BIN=... ECHO_URL=http://api.ipify.org cargo test -p app real_chromium_reports_the_address -- --ignored`
   - Windows lifecycle: `CHROMIUM_BIN=... XRAY_BIN=... cargo test -p runtime --test windows_real -- --ignored --test-threads=1`
   - The ignored tests in `core_releases` and `xray_releases` dial the GitHub API and download a real core or engine over the network.
4. **Packaging and workflow changes: the dry run.** Run `Release` from the Actions tab (`workflow_dispatch`): it runs `manifest`, `linux` and `windows` and skips `publish`. The `linux` job unpacks the archive, installs it into a scratch home, runs `--version` from the installed path, verifies every `.sha256` and installs the `.deb` with `dpkg --install`; the `windows` job builds the Inno Setup installer after the packaging script's resource-section check. Say which run proved it.
5. **Real use where possible.** A change to a launch, a proxy test or a download is also checked against the real thing in an opt-in suite above, not only against its stub. What was not tried is said plainly: nothing has run the installer, and the program has never been started on Windows, so the tray icon and the single-instance lock are compiled and tested without a desktop but not seen working (see [releases](release.md)).
6. **A release counts only when all of these hold.** The gate passed on the commit that will be tagged (`scripts/check.sh` / `check.ps1`); the tag is `v<version>` and matches `scripts/version.sh` and the changelog's newest section; the changelog has a `## [<version>]` section for `scripts/changelog-section.sh <version>`; `Release` ran end to end - `linux` and `windows` built and `publish` attached both artifacts with their checksums - and the release notes are that changelog section. The steps are in [releases](release.md).
