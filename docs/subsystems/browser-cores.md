# Browser cores

A browser core is a fingerprint-chromium build this program launches. This
subsystem owns the record that names one, the major its own binary reports, the
switch table that major selects, and the two GitHub downloads that put a binary
on disk - the published core builds and the Xray engine - neither of which runs
without a click. Where the launch path goes from there (ports, processes, the
supervisor) is [runtime lifecycle](../lifecycle.md); the types are the [domain
model](../domain-model.md)'s; which switches a major may be asked to spoof is
[fingerprint-matrix.md](../fingerprint-matrix.md)'s.

## Responsibilities and boundaries

- **Discovery and startup maintenance** - `crates/app/src/core_detect.rs`
  (`discover`, `maintain`, `maintain_with`, `refresh`): where to look for a
  binary and how the stored catalogue is reconciled with what is on disk.
- **The core record's lifecycle** - `crates/application/src/core_service.rs`
  (`CoreService`, `DefaultCoreService`): add, update, redetect, insert (import),
  delete, usage. Nothing probed here accepts a version the binary did not
  report - `add` and `redetect` always probe, `update` re-probes when the
  executable changed, and `insert` (import) is the deliberate exception,
  storing the file's recorded version verbatim.
- **Version detection and capabilities per generation** -
  `crates/runtime/src/version.rs` (`VersionReport::probe`, `parse_major`), which
  lives in `runtime` because `crates/runtime/src/capability.rs`
  (`DefaultCapabilityResolver`) and `crates/domain/src/core.rs`
  (`CoreCapabilities`, `FingerprintGeneration`) consume it.
- **The downloads** - `crates/app/src/core_releases.rs` and
  `crates/app/src/xray_releases.rs` (catalogues and downloaders),
  `crates/app/src/state/releases.rs` and `state/xray_releases.rs` (window
  state), the dialogs `crates/app/src/ui/cores.rs` and `ui/xray.rs`.
- **Shared release machinery** - `crates/app/src/releases.rs`, called by both
  downloads rather than copied.

Outside this page: the service interfaces are sketched in
[service contracts](../service-contracts.md), and the packaging policy - what a
release contains, and that neither engine ships in one - is
[release.md](../release.md)'s.

## Source of truth

- The stored `BrowserCore` row (`domain::core`, persisted through
  `storage::CoreRepository`) is what a profile launches with. `major` is the
  input capability resolution reads, and `version` is the banner the binary
  printed, kept verbatim.
- A release tag is never a major. A downloaded browser is registered by probing
  the unpacked binary through the same `application::CoreService` the Add Core
  form uses, deliberately not by the tag on the release that carried the file
  (`crates/app/src/core_releases.rs` and `state/releases.rs` module docs; test
  `a_downloaded_browser_is_registered_with_the_version_it_reports`).
- The published lists are read live, not cached: `ReleaseStatus::Unasked` means
  no request has been made, and the request happens when the dialog first opens
  or Refresh is pressed (`AppView::on_open_core_downloads`).
- For the engine there is no database row: the source of truth is the path in
  the configuration (`SettingKey::XrayExecutable`). Which release is on disk is
  read back out of that path (`AppState::downloaded_xray_tag`), and what a
  downloaded binary is is read from `PROVENANCE.txt` beside it (`engine_section`
  in `crates/app/src/diagnostics.rs`; see [diagnostics](../diagnostics.md)).

## Main runtime paths

### Startup: discovery and refresh

`main` (`crates/app/src/main.rs`) calls `core_detect::maintain(core_repo, t)`
after storage opens and before the window exists, so the catalogue is
reconciled before any profile can launch. With an empty catalogue it registers
one core:

- `discover()` looks at `FP_BROWSER_CHROMIUM_BIN` first and treats it as
  authoritative: set but not a file, and discovery stops rather than silently
  picking another browser.
- Otherwise: `bin/chromium`, `bin/chrome` and `chrome` relative to the working
  directory (Windows adds `bin/chrome.exe`, `chrome.exe`), then the platform's
  browser names on `PATH`.
- `register` probes that binary once; the major is the one it reported or
  `FP_BROWSER_CHROMIUM_MAJOR`; the version is the banner or `unknown`; the name
  is `domain::suggested_name`. Major `0` is stored and reported as a warning
  naming the variable, not refused.
- No discovered binary is an error notice naming `FP_BROWSER_CHROMIUM_BIN`, and
  the catalogue stays empty.

With a non-empty catalogue, `refresh` re-probes every registered core instead of
registering anything. The probes run at once, one thread per core inside
`std::thread::scope`, and are all joined before the window opens - a catalogue
used to cost one probe after another. A core whose executable is not a file is
collected and reported as an error naming it; a probe that answers nothing keeps
the stored major and version; a changed major or banner rewrites both, and an
auto-generated name follows the major while a chosen one never does
(`refreshed_name`). The one notice the function returns goes onto the banner.

### Reading a core's major

`VersionReport::probe` reads the binary's own folder before it runs anything,
and on Windows avoids starting the binary where it can (`--version` on a
GUI-subsystem Chromium would open a blank window). `read_from_manifest_or_directory`
looks for a `manifest.json` with a `version`, a `*.manifest` whose stem starts
with a digit, or a version-shaped subdirectory of the executable's folder;
`read_pe_version_windows` (Windows) reads `ProductVersion` through PowerShell,
bounded by `min(timeout, 3 s)`; otherwise the binary itself is run with
`--version`, bounded by `DEFAULT_TIMEOUT` (5 s) and killed at the deadline, so a
hung or wrong binary reads as "no version". The banner is trimmed and kept verbatim; `parse_major`
takes its first run of ASCII digits (`Chromium 148.0.7778.215` is 148).

For a form save or a re-detect the same probe runs on a worker
(`crates/app/src/maintenance.rs`, `AddJob`, `UpdateJob`, `RedetectJob`), so a
slow or hanging binary never holds the window.

### Adding a core

The Add Core form hands its path to `AppView::start_core_save`, which runs
`maintenance::run_add` -> `DefaultCoreService::add` off the drawing thread. The
service refuses, in order: an executable already registered; a path that is not
a file (inside `read_version`, before any probe); and a probe result without a
positive major, naming the path, `--version` and `FP_BROWSER_CHROMIUM_MAJOR`.
The banner is stored verbatim, the name is the typed one or `suggested_name`,
and `domain::validate_core` is applied before the row is saved. `update`
re-probes only when the executable changed, and a refusal changes nothing;
`redetect` re-reads the stored binary and keeps a chosen name while an
auto-generated one follows the major; a missing binary is refused and the record
stays. `delete` is refused while any profile points at the core, and says which.
`insert` is the import path: an upsert that stores a core exactly as the file
gives it, without probing; import itself is
[backup and restore](../backup-and-restore.md)'s.

Capabilities hang off that stored major: `FingerprintGeneration::for_major`
splits at `PIVOT_MAJOR` (144) into `Legacy` and `Chrome144Plus`, and
`CoreCapabilities::for_major` is the one table the serializer and the
compatibility layer read. At launch the supervisor resolves it before building
the plan and reports every switch the core cannot honour
(`crates/runtime/src/supervisor/start.rs`, `crates/runtime/src/compat.rs`); the
measurements behind the table are the [fingerprint matrix](../fingerprint-matrix.md)'s.
Major `0` has no table, so resolution refuses it rather than launching with an
assumed set.

### Downloading a published core

The Download Core dialog asks `GithubReleaseCatalog` for the list on first open
and on Refresh (`AppView::on_fetch_releases`); `REPOSITORY` is
`adryfish/fingerprint-chromium`, and one release (one version) is one row, with
one button per platform the release offers. Clicking a button calls
`AppView::on_download_release`: `AppState::begin_core_download` starts the one
download in flight, and a `DownloadJob` is built with the directory
`AppState::release_dir(tag)` - `<data dir>/cores/<safe tag>`
(`crates/app/src/paths.rs`, `CORES_DIR`), so two versions never share a tree.
`HttpCoreDownloader::download` then runs on a worker:

- `releases::fetch_asset` streams the asset into the release directory under a
  `.part` name and renames it at the end. A core is fetched without a digest
  (`hash = false`) because it has nowhere to record one.
- A portable archive is unpacked into `unpacked/` (`UNPACK_DIR`) by `unpack`
  (zip through the `zip` crate, tar.xz through `tar` + `xz2`), then
  `find_browser` picks the shallowest match (`chrome`/`chromium`,
  `chrome.exe`/`chromium.exe`, or any file under a macOS `Contents/MacOS`
  directory), depth-bounded by `SEARCH_DEPTH`, and `make_runnable` sets the
  executable bit.
- An AppImage is itself the browser once it is runnable; an installer or a disk
  image is downloaded and left as a file, with `Downloaded.executable` `None`.

The answer arrives through the worker channel and `drain_releases`
(`crates/app/src/ui/workers.rs`) applies it: `AppState::finish_core_download`
returns an `AddJob` when there is a browser to register - probed on another
worker, with `CoreSaveOrigin::Download` so a refusal names the file - and
otherwise reports where the file or the unpacked tree is. A download that
failed registers nothing.

### Downloading the Xray engine

The engine download is a separate pair of traits over the same shared machinery,
because an engine differs in three ways (`crates/app/src/xray_releases.rs`
module docs): the architecture is in the file name (`Xray-linux-arm64-v8a.zip`),
the archive carries geo data this program does not use, and what arrives is a
configuration setting rather than a catalogue row. `REPOSITORY` is
`XTLS/Xray-core`; `classify` reads the platform prefix and compares the
remainder to an exact architecture token, so `Xray-linux-mips32.zip` is not
misread as the 32-bit x86 build. `Release::for_host` offers only the build this
machine would run (ranked among the three 32-bit ARM names, equal everywhere
else), and `XrayAsset::digest_url` points at upstream's `.dgst` beside the
archive. The dialog shows one row per version with one button - named
`platform · architecture` - or a "no build for this system" note.

`HttpXrayDownloader::download` fetches with a digest requested, then `extract`
writes the entries `keeps` names (`xray`, `xray.exe`, `LICENSE`, matched by base
name) into the release directory itself, counting everything else as skipped;
every output path is built from that base name, so an archive entry called
`../../xray` lands as `xray`. No engine in the archive is an error naming the
release and the directory. The engine gets the executable bit, the licence is
cleared of it, and `write_provenance` records repository, release, asset, URL,
the SHA-256 of what arrived and where to check it in `PROVENANCE_FILE`. A
finished download becomes `SettingKey::XrayExecutable`, logs the provenance and
reports the kept and skipped sizes; when `FP_BROWSER_XRAY_BIN` supplies the path
in force, the banner says the setting is shadowed instead of reporting success,
and a failed download leaves the setting alone.

### The shared release machinery

`crates/app/src/releases.rs` is the half both downloads call rather than copy:

- `fetch_release_list` - one request to `api.github.com` with the releases
  media type, `RELEASES_PER_PAGE` (30) per page and a `User-Agent` of the
  program's name and version.
- `read_releases` - the API document read into `Release` values: drafts are
  left out, a pre-release is kept and marked, `published_at` is cut to
  `YYYY-MM-DD`, each remaining asset goes through the feature's own
  `Asset::read`, and a release left with no usable asset is dropped entirely.
- `fetch_asset`, `safe_file_name`, `partial_path`, `make_runnable`,
  `size_label` - the `.part` write and rename, the name sanitiser (anything
  that is not alphanumeric or `. _ - +` becomes `_`, and an empty, `.` or `..`
  name becomes `core`), the runnable bit, and the sizes the dialog shows.
- `ReleaseEvent` (Listed / Progress / Downloaded), `ReleaseStatus` and
  `Download` - the shapes the window's side uses for both features, with the
  release type (and, for `ReleaseEvent`, the downloaded payload type) differing;
  `Download` itself is shared unchanged.

## State and transitions

A core is registered once - by the bootstrap when the catalogue is empty, by the
Add Core form, or by a finished download through the same service. After that it
changes only by being updated (a rename is one write; a re-pointed core re-reads
its version), redetected, refreshed at startup, or deleted when no profile uses
it; the page reloads from storage (`AppState::load_rows`).

`ReleaseStatus` moves `Unasked -> Loading -> Listed | Failed`:
`AppState::begin_release_fetch` / `begin_xray_release_fetch` set `Loading`, and
`releases::ReleaseStatus::finish_fetch` sets `Listed` or `Failed`. Starting a
fetch replaces the previous list with `Loading` rather than keeping it behind a
spinner: a refresh that failed must not leave the reader choosing from stale
rows. A failure is written by the feature's own words (`core_download_failed`,
`xray_download_failed`), stored as `Failed`, and also pushed to the banner -
which is what the reader sees if the dialog was closed while the request was out.

`Download` (shared; `ReleaseDownload`/`XrayDownload` are aliases) holds the key
`<release>/<asset>` and the bytes. `begin` refuses a second download and names
the asset that is running rather than queueing it - a core is a third of a
gigabyte and a row has one progress bar; `update` drops a report whose key is no
longer the one in flight, and only accepts a total greater than zero (a server
that gives none leaves the asset list's own size in place), so `fraction`
returns `None` rather than inventing a bar. `finish_*` clears the slot; on the
dialog the running row shows the progress bar in place of its buttons. The two
features have one download each: the core and the engine are
independent slots.

## Constraints and failure behavior

- **The probe is bounded.** `DEFAULT_TIMEOUT` is 5 s and the child is killed at
  the deadline, so a hung or wrong binary reads as "no version" instead of
  holding anything up; on Windows the manifest and PE reads come first because
  running a GUI-subsystem Chromium with `--version` would open a blank window.
- **An unknown major is refused where it would become a claim.** The bootstrap
  may store major `0` with a warning naming `FP_BROWSER_CHROMIUM_MAJOR`, but
  `CoreService::add` refuses it, `BrowserCore::capabilities` returns `None`, and
  `DefaultCapabilityResolver::resolve` rejects it at launch with
  `CapabilityError::UnsupportedMajor`: a fingerprint browser that cannot
  classify its engine must not launch with unverified spoofing switches.
- **`FP_BROWSER_CHROMIUM_MAJOR` covers registration only** (read by
  `core_detect::env_major` when a discovered binary is registered); the startup
  refresh and `CoreService` do not consult it. `FP_BROWSER_CHROMIUM_BIN` is
  authoritative for discovery, and stops it when set to a path that is not a
  file.
- **One row per version, and a version is a directory.** A row is one GitHub
  release, one button per platform the release offers - the best asset for that
  platform, Portable first - and a release with no asset this program can run is
  not a row - an empty list and a failed fetch are different states in the
  dialog. Each version is downloaded under `<data dir>/cores/<tag>`
  or `<data dir>/xray-releases/<tag>`, and a repeated download replaces the file
  (the old final name is removed before the `.part` rename). The policy side -
  no download without a click, neither engine in a release - is
  [release.md](../release.md)'s.
- **Names out of a JSON document never become paths.** Tags and asset names go
  through `safe_file_name`; the core unpackers validate entry paths, and the
  Xray extractor writes only from a matched base name over a fixed directory.
- **Network bounds.** A catalog request has one 30-second whole-request
  deadline; a download has a 30-second connect timeout, 60 seconds for the
  response headers and a 30-minute body budget. Progress is reported every MiB
  and once more at the end, whatever the size, so a finished bar reaches its end.
- **Failures say what the reader can do.** A fetch failure is shown in the
  dialog and the banner; a failed download registers no core and leaves the
  engine setting untouched; a downloaded file that is not a core is reported
  with the path it is at; an Xray archive with no engine is an error naming the
  release and directory.

## Verification

The gate runs all of these as part of `cargo test --workspace`
(`scripts/check.sh`). The per-area commands:

```sh
cargo test -p app core_detect::tests            # discovery, registration, startup refresh
cargo test -p app core_releases::tests          # catalogue, classification, unpacking
cargo test -p app xray_releases::tests          # engine catalogue and extractor
cargo test -p app state::tests::releases        # fetch, download, registration
cargo test -p app state::tests::xray_releases   # the engine setting
cargo test -p app ui::tests::cores              # the cores page and its dialog
cargo test -p app ui::tests::xray               # the engine card and its dialog
cargo test -p application core_service::tests   # the stored row: add/update/redetect/delete
cargo test -p runtime version::tests            # the probe, parse_major
cargo test -p runtime capability::tests         # major 0 is refused
cargo test -p domain core::tests                # the generation split and its table
```

These suites name the contracts above: startup by
`an_empty_catalogue_registers_the_discovered_core` and
`a_replaced_binary_updates_the_stored_major_and_name`; the parallel refresh by
`every_registered_core_is_probed_and_updated`, with
`a_missing_executable_is_an_error_that_names_the_core` and
`an_unreadable_probe_keeps_the_stored_core` for its failures; the list rules by
`the_preferred_asset_is_the_one_that_can_become_a_core` and
`a_release_with_no_usable_asset_is_not_listed`; the engine by
`only_the_engine_and_the_licence_are_kept` and
`an_entry_cannot_write_outside_the_directory`; and the end-to-end download paths
by `downloading_the_build_for_this_system_registers_it` and
`a_download_an_environment_variable_outranks_is_not_reported_as_installed`.

Opt-in suites, none of them part of the gate; all four dial GitHub, and the two
downloads move a real release over the network:

```sh
cargo test -p app core_releases::tests::the_real_repository_answers_with_releases_this_can_read -- --ignored
cargo test -p app core_releases::tests::a_real_release_downloads_and_unpacks_into_a_browser -- --ignored
cargo test -p app xray_releases::tests::the_real_repository_answers_with_releases_this_can_read -- --ignored
cargo test -p app xray_releases::tests::a_real_release_downloads_and_unpacks_into_an_engine -- --ignored
```

The repository-acceptance tests are the ones that would notice GitHub answering
in a shape the readers do not understand; the download tests are the only ones
exercising the redirect, the streaming reader, the `.part` rename and the
unpacker over a real release archive. On Linux test builds, the `real_chromium_*`
suites in `crates/app/src/real_browser.rs` exercise the probe and the capability
table against a real binary (`CHROMIUM_BIN=... cargo test -p app real_chromium
-- --ignored`); the addresses of the runtime acceptance suites are
[chromium-acceptance.md](../chromium-acceptance.md)'s.
