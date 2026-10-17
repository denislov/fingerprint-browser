# Settings

The settings subsystem reads this installation's configuration once, at startup -
the process environment first, then the config file, then each value's built-in
default - and resolves the values the rest of the program runs on. It owns the
config file: where it lives, what it holds, how it is rewritten, how a file left
in the old location is carried across, and what a run does when the file cannot
be read. It also owns the layout rule that one directory holds the whole
installation - which is why the config file cannot decide where the data
directory is.

What each resolved value *does* is documented where it is used; this page owns
resolution and storage.

## Responsibilities and boundaries

In scope:

- Reading the process environment once into `Environment`
  (`Environment::from_process`), so a test can describe a machine instead of
  borrowing one.
- Reading the config file into `Stored`
  (`crates/app/src/settings/persistence.rs`, `read_config`) and resolving each
  effective value (`Settings::load`, and `resolve_data_dir`,
  `resolve_config_path`, `resolve_echo_url`, `resolve_use_system_proxy` in
  `crates/app/src/settings/resolution.rs`).
- Reporting, per row, the value in force, where it came from (`Source`,
  `SettingRow::source_label`) and any stored value the environment is overriding
  (`SettingRow::shadowed`, `SettingRow::shadowed_label`).
- Writing the whole file on every change, and refusing to write while the file
  is unreadable (`write_config`; the replacement itself is
  `storage::atomic_file::write`).
- The one-time move of a config file left in the old location (`read_stored`).
- Answering `--help` and a refused argument in the language the file names,
  without loading or moving anything (`language_hint`).
- The layout: the data directory, the config file inside it, the runtime
  directory derived from it, and the platform rules in `paths.rs`.

Out of scope:

- Services and their interfaces - settings are process state read by `main.rs`,
  not a service (see [Service contracts](../service-contracts.md)).
- What is in a backup, what a profile is, what a session record is: [Backup and
  restore](../backup-and-restore.md), [Domain model](../domain-model.md),
  [Runtime lifecycle](../lifecycle.md).
- The report's own content: [Diagnostics](../diagnostics.md) renders the rows
  this page produces into its "Settings in force" section (`diagnostics.rs`,
  `settings_section`).
- Which languages exist and how the catalog is kept honest: [Interface
  text](../i18n.md).

## Source of truth

| Concern | Source |
| --- | --- |
| Resolved values, keys, rows, sources, groups | `crates/app/src/settings.rs`: `Settings`, `SettingKey`, `SettingRow`, `Source`, `Effect`, `SettingGroup`, the `*_ENV` constants |
| Precedence, migration, language hint | `crates/app/src/settings/resolution.rs`: `resolve_data_dir`, `resolve_config_path`, `resolve_echo_url`, `resolve_use_system_proxy`, `read_stored`, `switch_on`, `language_hint` |
| Reading and replacing the document | `crates/app/src/settings/persistence.rs`: `read_config`, `write_config` |
| Platform directories | `crates/app/src/paths.rs`: `Host`, `data_dir`, `config_dir`, `data_dir_or_fallback`, `APP_DIR`, `CONFIG_DIR`, `CONFIG_FILE`, `FALLBACK_DATA_DIR` |
| The window's copy | `crates/app/src/state/mod.rs`: `AppState`'s `settings` field; `state/activity.rs` for which group is on screen; `crates/app/src/ui/settings.rs` for the page |
| Stored names of the standing choices | `ThemeChoice::code` / `from_code` (`theme.rs`), `Lang::code` / `from_code` (`text.rs`), `ExitMode::code` / `from_code` (`exit.rs`) |

The environment is read once, so a variable set after the window opens is not
seen by this run. The file is likewise read once: `Settings::load` is the only
reader of the config document in the running program, and tests build their own
`Environment` instead of touching the process's.

### The layout

One directory holds an installation: everything a run needs is under it, and the
config file sits inside it rather than beside it. It is what to look at when
something is wrong, what to copy when moving machines, and what to delete to
start over.

| Path under the data directory | What it holds |
| --- | --- |
| `config.json` | this subsystem's file; its path is `FP_BROWSER_CONFIG` when set (`resolve_config_path`) |
| `app.db` | the SQLite configuration and profiles (`main.rs` opens it) |
| `profiles/<profile uuid>/` | one Chromium user-data directory per profile (`application::default_user_data_dir`) |
| `cores/<release>/` | browser cores this program downloaded (`paths::CORES_DIR`) |
| `xray-releases/<release>/` | Xray engines this program downloaded (`paths::XRAY_DIR`) |
| `runtime/<profile id>/` | session records and generated Xray configs, removed at stop (`Settings::runtime_dir`; the supervisor writes them, see [Runtime lifecycle](../lifecycle.md)) |
| `logs/activity.log` | the activity log (`log_file::LOG_DIR`, `LOG_FILE`) |
| `exports/` | where an export writes by default (`paths::EXPORT_DIR`, `default_export_file`) |
| `diagnostics/` | where a report writes by default (`paths::DIAGNOSTICS_DIR`, `default_diagnostics_file`) |
| `instance.lock` | the one-window-per-directory lock (`instance::LOCK_FILE`; see [Exit modes](../exit-modes.md)) |

The data directory itself is `FP_BROWSER_DATA_DIR`, or the platform's own place
for application data (`paths::data_dir`): on Windows `%LOCALAPPDATA%`, falling
back to `%APPDATA%\Local`; on macOS `~/Library/Application Support`; elsewhere an
absolute `$XDG_DATA_HOME` or `~/.local/share` - under `<base>/FpBrowser`
(`paths::APP_DIR`). A host `paths::data_dir` cannot be told about gets the
relative `data` (`paths::FALLBACK_DATA_DIR`) - the old behaviour, now only the
fallback, and the move of that default is announced once
(`paths::moved_data_dir_notice`).

## Main runtime paths

### Startup

`main.rs` calls `Settings::load(Environment::from_process())` before storage
opens, and the resolved settings decide what this run does:

- `Settings::data_dir` decides the instance lock (`instance::InstanceLock::acquire`),
  the database path (`SqliteStorage::open(<data dir>/app.db)`) and the log.
- `Settings::xray_executable`, `Settings::runtime_dir` and
  `Settings::use_system_proxy` are handed to the supervisor
  (`SupervisorComponents`) and to the proxy tester (`XrayProxyTester::new`).
- `Settings::theme` is applied before the first frame (`theme::apply`).
- `Settings::language` picks the `Text` table (`Settings::text`) every later
  sentence is built from, including the notices produced before the window
  exists.
- The second return value, a notice, is pushed to the window's banner
  (`AppState::push_notice`), or printed to stderr for `--diagnostics`.

### Reading and reporting the rows

`Settings::rows` builds one `SettingRow` per key - `SettingKey::id` is the id
the window and the tests address a row by - and `AppState::setting_rows` renders
them in the language in force. `SettingGroup::of` places six rows in `Runtime`
and three in `Data` (`General` and `Diagnostics` hold cards rather than rows;
`SettingGroup::ALL` is the order of the chips). Each row carries the value in
force, its `Source`, the environment variable behind it when one decided it, any
stored value the environment is shadowing, and a note. `Effect` is an enum, not
a phrase, so the window compares it and only then turns it into words: `DataDir`,
`XrayExecutable` and `UseSystemProxy` are `Effect::NextStart`; `RuntimeDir` is
`Effect::Derived`; `EchoUrl`, `StatusPage`, `ChromiumBin`, `ChromiumMajor` and
`ConfigFile` are `Effect::Now` - the identity page's switch is read when Start is
pressed rather than when the program starts, so turning it off is enough to leave
the page out of the next launch. The data directory row is the one whose source is never
`ConfigFile` - nothing in the file can decide it - and the Chromium rows can
only be `Environment` or `Default`, because those values are not stored. The
data directory row is read-only, and its note says how to move the directory -
set `FP_BROWSER_DATA_DIR` before a start - instead of offering an input that
could not take effect.

### Writing

Every stored change goes through the same path: check the file is readable, clone
the stored document, change one field, `write_config` the whole document, and
only then commit the change in memory. The theme choice is written before it is
applied: a config file that cannot be written refuses the switch instead of
repainting the window into a mode the next start will not keep - the "looks
effective but is not" failure this page avoids. The entry points are:

- `Settings::set` for the two typed values (`XrayExecutable`, `EchoUrl`);
- `Settings::set_theme`, `set_language`, `set_exit_mode`,
  `set_use_system_proxy`, `set_sidebar_collapsed` for the rest.

`AppState` wraps each of them (`update_setting`, `set_theme`, `set_language`,
`set_exit_mode`, `set_use_system_proxy`, `set_sidebar_collapsed`) and turns the
result into a notice: a refusal is an error in the banner and nothing changes;
a success is a toast, except for the sidebar switch, which reports nothing
because the window has just moved. One writer goes through `Settings::set`
without those wrappers: a finished Xray download
(`AppState::finish_xray_download`) stores its executable there directly, with
its own notice - an info toast when the setting is saved, but an error in the
banner when `FP_BROWSER_XRAY_BIN` outranks the path just stored
(`Settings::xray_from_env`). For `XrayExecutable` and `EchoUrl` the
settings are re-resolved after the write (`Settings::set`), so the page shows the
value the next start will use while the running process keeps the old one - the
engine path and runtime directory were handed to the supervisor at startup. The
endpoint is the exception: it is read when a proxy test is assembled
(`AppState::begin_proxy_test`), so a saved endpoint is live for the next test.

The `use system proxy` switch is the one that can refuse a click before writing:
`use_system_proxy_from_env` is true while `FP_BROWSER_SYSTEM_PROXY` decides it,
and `AppState::set_use_system_proxy` refuses the click naming that variable,
rather than storing a value the next start would not read.

### The language hint

`main.rs` answers `--help` and a refused argument before the window exists, in
the language the config file names. `language_hint` (exported from
`crates/app/src/settings.rs`) resolves the same config path and reads only the
`lang` field, in English on anything it cannot read; it never migrates, writes
or reports, so asking how the program is used cannot fold an older
installation's config file into the data directory as a side effect.

## State and transitions

### Resolution

Environment, then the stored value, then the default - and the row says which of
the three won.

| Setting | Environment | Stored key | Default |
| --- | --- | --- | --- |
| Data directory | `FP_BROWSER_DATA_DIR` | not read (the old `data_dir` key is a migration source) | the platform place, else relative `data` |
| Config file | `FP_BROWSER_CONFIG` | not read | `<data dir>/config.json` |
| Xray executable | `FP_BROWSER_XRAY_BIN` | `xray_executable` | `bin/xray.exe` on Windows, `bin/xray` elsewhere (`default_xray`) |
| Use system proxy | `FP_BROWSER_SYSTEM_PROXY` | `system_proxy: "on"` | off |
| Identity page | - | `status_page: "off"` | **on** (the stored name is the off state) |
| Echo endpoint | `FP_BROWSER_ECHO_URL` | `echo_url` | `runtime::DEFAULT_ECHO_URL` |
| Chromium binary | `FP_BROWSER_CHROMIUM_BIN` | not stored | not set; `core_detect::discover` finds one on disk |
| Chromium major | `FP_BROWSER_CHROMIUM_MAJOR` | not stored | not set; the major is read from each binary (`core_detect::env_major` parses the override) |
| Runtime directory | - | not stored | `<data dir>/runtime`, derived (`Settings::runtime_dir`) |
| Appearance | - | `theme` (`"dark"`, `"light"`) | `dark` |
| Language | - | `lang` (`"en"`, `"zh"`) | `en` |
| Exit mode | - | `exit_mode` (`"ask"`, `"background"`, `"keep-running"`, `"exit-all"`) | `ask` |
| Sidebar | - | `sidebar` (`"collapsed"`) | expanded |

`FP_BROWSER_CONFIG` is an override rather than a location: a test or a script
needs to point at a file without inventing a data directory for it, and an
explicitly set path is not ours to migrate into. An ignored `data_dir` value in
a legacy file is reported on the data-directory row with the same sentence as
the environment override - "the config file holds …, which this overrides" - so
the row's wording covers "the environment is shadowing" and "the file's value is
ignored" the same way.

The standing choices (appearance, language, exit mode, sidebar) have no
environment override: they are choices about the installation, not about what
this process does, so a script cannot start the program with them.

### The config file

- **Absent** - the defaults are in force and there is no notice; a missing file
  is not an error.
- **Readable** - the values are in force; anything the environment overrides is
  reported on the row.
- **Present but unreadable** - `read_config` returns an error (a missing file is
  `Stored::default`; a read or parse failure is not); the error is reported, the
  defaults are in force so the app still starts, and `config_error` stays set,
  refusing every `set_*` for the rest of the run - writing would replace the
  values the file still holds. Fixing the file takes effect at the next start.

### The move from the old location

The move exists because a config file outside the data directory meant two
places to remember and two places a stale copy could hide. The old location is
a migration source, never a write target: nothing is written there again.

A config file written before the file moved into the data directory lives at
`legacy_config_path` = `paths::config_dir(host).join(paths::CONFIG_FILE)` =
`<platform config place>/fp-browser/config.json` - the platform's config place
being `%APPDATA%` on Windows, `~/Library/Application Support` on macOS, an
absolute `$XDG_CONFIG_HOME` or `~/.config` elsewhere, and `paths::config_dir`
itself already appending the `fp-browser` component (`paths::CONFIG_DIR`) - or
the relative `fp-browser/config.json` when the host has no config directory.
`read_stored` reads it when the new path does not exist and
`FP_BROWSER_CONFIG` is unset, and it has three endings:

- **Moved** - the values are read, the `data_dir` key is cleared, the file is
  written into the data directory and removed from the old place. The stored
  `data_dir` is never carried over: the next start resolves the directory the
  same way this run did.
- **Left where it is** (`Migration::IgnoredDataDir`) - an old file that named a
  data directory other than the resolved one is not moved, because it is the only
  record of where that directory is. It is not the only ending that leaves the
  old file in place: `Migration::NotWritten` (the new location could not be
  written) leaves it too, and a legacy file that cannot be read is left where it
  is as well. The banner names the old directory and the one way back: set
  `FP_BROWSER_DATA_DIR` to it, and the next start resolves that directory and
  moves the file there.
- **Read but not written** (`Migration::NotWritten`) - the values are in force
  and the old file stays and keeps being read, but the next start will not find
  them at the new location; the banner says so rather than leaving a page that
  looks like it saved.

## Constraints and failure behavior

- **The environment wins, and the loser is visible.** A stored value the
  environment overrides is shown, not hidden (`SettingRow::shadowed_label`) -
  a setting that looks saved and is not is worse than no setting.
- **An empty variable is not a value.** `Environment::from_process` treats a
  variable that is missing or whitespace-only as unset.
- **A word this build does not know is not a value.** `switch_on` accepts `on`,
  `true`, `yes`, `1` and `off`, `false`, `no`, `0` (trimmed, case-insensitive) and
  passes anything else over, so a typo in `FP_BROWSER_SYSTEM_PROXY` leaves the
  stored switch in force. Stored names for the appearance, language, exit mode
  and sidebar fall back to their defaults instead: a name from a later build
  must not stop this one from starting.
- **A key this build does not know is an error.** `Stored` deserializes with
  `deny_unknown_fields`, so a config file with an unrecognized key fails the
  parse and is treated as unreadable. `language_hint` is the deliberate
  exception: it reads `lang` alone, so `--help` still answers in the file's
  language even when the full read would refuse the file.
- **Writing is all-or-nothing.** `write_config` creates the parent directory,
  serializes the whole `Stored` as pretty JSON, and replaces the file through
  `storage::atomic_file::write`, so a crash cannot leave half a document.
  `data_dir` is never written back (`skip_serializing`), because storing a
  second copy of the answer is how the two answers start to disagree. The two
  switches' default states are stored as absence: `set_use_system_proxy(false)`
  and an expanded sidebar both write `null`, the spelling a config file from
  before those switches existed already had. The identity page is the one switch
  that runs the other way - it is **on** by default, so what it writes down is
  the off state (`status_page: "off"`), and absence keeps meaning what it meant
  to a build that had never heard of the switch: a reader who gets the page. The
  key is written only in the off state; `Stored` denies unknown fields, so
  leaving the default out keeps a config file written by this build readable by
  one that predates the switch, as long as the reader never turned the page off.
  The standing choices are not like that - `set_theme`, `set_language` and
  `set_exit_mode` write their code even when it is the default, so `"dark"` and
  `null` both mean dark, `"en"` and `null` both mean English, and `"ask"` and
  `null` both mean ask.
- **Saving refusals.** An empty value (after trimming) is refused first; a key
  that is not editable is refused by `Settings::set` (only `SettingKey::editable`
  rows offer a change); a write that fails is reported with the path and the
  error, and nothing is committed to memory until it returned `Ok`.
- **The stored document has no validation of its own beyond shape.** Values are
  strings; a path that does not exist, or an endpoint that does not answer, is
  found where it is used, not at load. The one refusal up front is the
  diagnostic's: `check_echo_url` in `runtime::diagnostic::transport` turns an
  `https://` endpoint away with a configuration fault rather than asking a
  different question than the address appears to ask.
- **The data directory row is the one shown against the working directory.**
  It goes through `Settings::rendered` and `setting_relative_to`, so a fallback
  relative `data` is displayed with what it resolves against. The
  Xray-executable and runtime-directory rows show the path as it stands: their
  defaults are relative (`bin/xray.exe`/`bin/xray`; `data/runtime` when the
  data directory itself falls back) and are not joined to the working
  directory.
- **One file, one writer.** The program writes the config file only through
  `Settings`, and the window is the only caller; a second copy of the program on
  the same data directory is refused the instance lock instead of racing this
  one ([Exit modes](../exit-modes.md)). The lock is taken after `Settings::load`,
  the call that reads the config file and resolves the data directory being
  locked. A legacy config file is therefore folded into the data directory
  before the lock exists - a one-time write of the same bytes, upstream of the
  database and the children the lock protects - and the refusal of a second copy
  happens after that read.

## Verification

The gate runs the unit tests of the whole workspace (`cargo test --workspace`,
through `./scripts/check.sh` on Linux or `./scripts/check.ps1` on Windows). The
tests below are in the `app` crate's binary target:

- `cargo test -p app settings::tests` - resolution, storage, migration and the
  page's own rules, in `crates/app/src/settings/tests.rs`: for example
  `a_broken_config_file_is_reported_and_does_not_lose_settings`,
  `a_config_file_in_the_old_location_is_moved_into_the_data_directory`,
  `a_config_file_naming_another_data_directory_is_left_where_it_is` and
  `the_environment_decides_the_system_proxy_switch`.
- `cargo test -p app state::tests::settings` - what the window does with a
  resolved setting, in `crates/app/src/state/tests/settings.rs`: for example
  `saving_a_setting_stores_it_and_says_when_it_applies` and
  `a_click_is_refused_while_the_environment_decides_the_system_proxy`.
- `cargo test -p app ui::tests::settings` - the page through the real view, in
  `crates/app/src/ui/tests/settings.rs`: for example
  `a_setting_can_be_changed_from_the_window`,
  `only_the_editable_settings_offer_a_button` and
  `the_system_proxy_row_switches_and_stores_the_choice`.
- `cargo test -p app paths::tests` - the platform rules and the default-move
  notice, in `crates/app/src/paths.rs`: for example
  `the_data_directory_is_the_platforms_own_place`,
  `xdg_data_home_wins_over_the_home_directory` and
  `a_host_with_no_home_directory_keeps_the_relative_directory`.
- `cargo test -p app diagnostics::tests` - the report's settings section carries
  every row with its source (`a_report_names_the_build_the_place_and_every_setting`).

No `#[ignore]`d suite drives the config file; every test of resolution and
storage is in the gate. The opt-in suites that consume values this page
provisions take the endpoint from their own variable rather than from the stored
file, so they are relevant to a change in meaning, not to a change in resolution:

- `CHROMIUM_BIN=... ECHO_URL=http://api.ipify.org cargo test -p app real_chromium_reports_the_address_its_own_traffic_leaves_from -- --ignored`
- `XRAY_BIN=... DIAG_SOCKS5='host:port[:user:password]' DIAG_ECHO_URL=http://... cargo test -p runtime --test proxy_diag_real -- --ignored`
