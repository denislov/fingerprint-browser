# Changelog

Notable changes to this program. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), the versioning
[Semantic Versioning](https://semver.org/spec/v2.0.0.html), and the rules for
both are in [release.md](docs/release.md).

Nothing has been published yet. `0.1.0` is the version in `Cargo.toml`, and the
section for it below is what the `v0.1.0` tag will publish - the release workflow
refuses a tag the changelog says nothing about. Work that lands after this point
goes under `Unreleased`, and is folded into the version's section before the tag
is cut.

## [Unreleased]

## [0.1.0] - 2026-09-21

### Added

- One window per data directory. A second copy cannot take the lock on the data
  directory, prints which process holds it, and exits 3 - instead of opening the
  same database and reading the first copy's running browsers as orphans to stop.
  The lock belongs to the operating system, so a crash cannot strand it, and
  `--help`, `--version` and `--diagnostics` are still answered while a copy runs.
  The lock file is one more row in the diagnostics report's file list.
- `--version` prints the version, the commit the binary was built from and the
  platform; the same version is in the window's title bar, and the build, the
  platform and the data directory are the first line of the activity log.
- `--help`, in the language the config file names, and refusals for arguments
  this program does not have.
- `--diagnostics` writes one report about this installation for a bug report:
  the build, every effective setting with the source it came from, the state and
  permission bits of every file the installation keeps, and the end of the
  activity log. The same report is behind a button on the Settings page. It
  never opens the database and never reads browser data.
- A program icon, generated from `scripts/make-icon.py`: a PNG, an ICO with
  seven sizes, and an SVG. On Windows it also goes into the executable's resource
  section, with the product name and version.
- A Linux archive (`packaging/linux/package.sh`) with the binary, an installer
  for the current user, a menu entry and the documentation, and a Windows
  installer (`packaging/windows/package.ps1`) built with Inno Setup. Each ships
  with a checksum, and neither ships or downloads a browser or a proxy engine.
- `.github/workflows/release.yml`, which builds both artifacts for a `v*` tag,
  refuses a tag or a changelog that disagrees with the version, and attaches the
  artifacts to the release. Nothing has been released yet.

### Changed

- The binary is named `fingerprint-browser`, which is what the help text and the
  documentation have been telling people to run.
- The distributed build is built with `[profile.release]`: thin LTO, one codegen
  unit, and symbols kept so a crash can be read. Not `panic = "abort"`, which
  would take the supervisor's child-process cleanup with it.
- An empty profile list says how to get a browser core in the order a reader can
  act on it: the page inside the window first, then `FP_BROWSER_CHROMIUM_BIN` and
  a restart. It used to name only the second, while offering a button for the
  first. The whole first run is now written down in
  [first run](docs/first-run.md).

### Fixed

- A build made after a commit no longer reports the commit before it. The build
  script relied on cargo's default rule - run again when a file in this package
  changes - and a commit changes what `git describe` answers without touching any
  file, so a clean tree could produce a binary that named an older commit and
  called itself `-dirty`. It now watches the git files that answer is read from.
- A configuration file left in the old per-platform directory is folded into the
  data directory at the next start, and the README no longer claims settings live
  outside the data directory - which stopped being true when the config file
  moved in beside the database.

[Unreleased]: https://github.com/denislov/fingerprint-browser/commits/master
[0.1.0]: https://github.com/denislov/fingerprint-browser/releases/tag/v0.1.0
