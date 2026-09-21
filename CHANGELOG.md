# Changelog

Notable changes to this program. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), the versioning
[Semantic Versioning](https://semver.org/spec/v2.0.0.html), and the rules for
both are in [release.md](docs/release.md).

Nothing has been released yet: `0.1.0` is the version in `Cargo.toml` and the
first artifact is not published. Everything below is under `Unreleased` until
it is.

## [Unreleased]

### Added

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
  seven sizes, and an SVG.

### Changed

- The binary is named `fingerprint-browser`, which is what the help text and the
  documentation have been telling people to run.
- The distributed build is built with `[profile.release]`: thin LTO, one codegen
  unit, and symbols kept so a crash can be read. Not `panic = "abort"`, which
  would take the supervisor's child-process cleanup with it.

### Fixed

- A configuration file left in the old per-platform directory is folded into the
  data directory at the next start, and the README no longer claims settings live
  outside the data directory - which stopped being true when the config file
  moved in beside the database.

[Unreleased]: https://github.com/denislov/fingerprint-browser/commits/master
