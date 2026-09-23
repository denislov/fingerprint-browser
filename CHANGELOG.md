# Changelog

Notable changes to this program. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), the versioning
[Semantic Versioning](https://semver.org/spec/v2.0.0.html), and the rules for
both are in [release.md](docs/release.md).

`0.1.0` is the first release, and the version in `Cargo.toml`. Work that lands
after a release goes under `Unreleased`, and is folded into the next version's
section before its tag is cut - the release workflow refuses a tag the changelog
says nothing about, and the notes are that section rather than the whole file.

## [Unreleased]

### Fixed

- A record the database refuses is now reported as what it actually is. Every
  constraint failure used to be reported as "id already exists", so a profile
  naming a core that was not stored was told its identifier was taken; the
  classification now reads SQLite's extended error code and separates a taken
  identifier, a missing reference (naming which one), a value the schema refuses,
  and a real database failure. The in-memory backend, which had no reference rules
  at all - so the application's own tests proved nothing about them - now refuses
  the same writes for the same reasons, and one contract test runs against both.
- A profile whose browser could not be stopped is no longer reported as stopped.
  When a session adopted from a previous run could not be identified, read or
  terminated, the stop failure went to the log while the session record, the
  temporary Xray configuration and the profile's state were all cleared anyway - so
  a live browser was left running with nothing tracking it, nothing refusing to
  start it again, and no record for the next run to find. The stop now reports what
  happened; a failure keeps the record, the configuration and the session, leaves
  the profile running so it can be stopped again, and says so in the window.
- Deleting a proxy a profile uses no longer quietly leaves that profile without a
  proxy. The database's foreign key cleared the reference (`ON DELETE SET NULL`)
  while the service refused the same deletion, so any path that did not go through
  the service - or two of them at once - silently changed a profile's network
  egress. The key is now restrictive, which is the same rule where nothing can go
  around it.
- Restoring a configuration backup is now all or nothing. It used to delete every
  stored core, proxy and profile and then write the file's records one at a time,
  with the file only being validated as each record was written - so a file with an
  empty core name, a profile naming a core that was not in it, or a disk that
  filled up halfway left an installation holding neither the old configuration nor
  the new one. Restore now validates the whole file first (the rules a form is
  held to, plus repeated identifiers, dangling references, and a backup whose
  credentials were exported away, each refused with its own sentence) and then
  replaces everything in a single database transaction, so a failure at any point
  leaves the previous configuration exactly as it was.
- A start, a browser-data copy and a configuration replacement can no longer
  overlap on the same profile. A start returns as soon as its command is queued,
  so the runtime's snapshot still said the profile was stopped while its browser
  was coming up - long enough to begin copying a profile that was already
  starting, or to begin a second copy that cleared a destination the first was
  still writing. Each operation now takes a lease on the profiles it will use, the
  worker that runs a copy owns that lease and gives it back when it returns, fails
  or panics, and replacing the configuration is refused while any profile is held.
- A profile's start page must be a page. It is appended to the browser's command
  line as an argument of its own, so a value from an imported configuration that
  spelled a switch - `--no-proxy-server`, say - was Chromium's switch to obey, and
  the proxy the profile's fingerprint depends on was off. The domain validation,
  the service that stores a profile, the configuration import and the launch
  planner all refuse it now, and the error names what is wrong with it.
- A browser-data copy can no longer destroy the data it is reading. Every path in
  a run is resolved and compared before the first write, so a backup directory
  inside the profile it copies, a restore whose destination contains its source, a
  destination reached through a symlink, and two profiles pointed at overlapping
  directories are all refused with nothing written.
- A browser-data copy is built whole beside its destination and renamed onto it
  only once it is complete, so a copy that fails partway leaves the directory that
  was there exactly as it was. A run killed between the two renames is put back by
  the next start - or, for a destination inside a backup directory, by the next
  copy - and the profiles it was put back for are named in the window.

- A desktop that will not take a tray icon no longer hides the window. "Keep
  running" put the window away whether or not the icon had been created, and on a
  desktop with no StatusNotifierItem host - a refusals path the code already
  logged and then ignored - the window went with nothing left to bring it back: a
  running program with no window, no icon and no way to reach it. The icon is now
  started through an injected starter (`tray::TrayStarter`), a refusal keeps the
  window on screen, and the banner names the desktop's own reason. The behaviour
  is pinned by a test that hands the window a starter which always fails.

### Changed

- Writing a configuration backup, importing one, restoring one and re-reading a
  core's version no longer run on the thread that draws the window. All four are
  slow in the way a page switch is not - an export reads every core, proxy and
  profile, an import then writes every record in the file one at a time, a restore
  replaces the whole configuration in one transaction, and a version probe starts a
  program and waits for it to answer - and each of them used to run inside its own
  click handler, so the window stopped redrawing until it finished. They now run on
  a worker and report one answer type through one channel, so a fifth task cannot
  report somewhere the window does not hear it. Only one runs at a time: they read
  and write the same rows and the same file, and a second click while one is
  running is refused with a sentence rather than interleaved with it.
- A new profile's fingerprint seed now comes from the operating system's entropy
  rather than from the clock. The seed is what makes two profiles' fingerprints
  differ, so a repeated seed is two profiles a site can correlate and a guessable
  one is a fingerprint a site can anticipate - and the low 32 bits of a nanosecond
  count repeat every four seconds and are guessable from any timestamp the machine
  reports. The source is injected, so the tests that have to know what the seed was
  can say.
- The runtime's test-support builders are now shared, and the command queue's rules
  are tested on every platform. The supervisor's test module spawns shell scripts,
  so it is Unix-only; the parts of the state machine that need no process - a full
  queue, a closed channel, the exit handover's patience - now live outside it in a
  portable module, and run on Windows CI too.
- A command the runtime cannot take is now refused instead of freezing the window.
  Every start, stop and restart was sent through a bounded queue with a blocking
  send, from the window's own thread - so while the supervisor was inside a
  readiness wait or a process-tree kill, the window waited with it. A full queue
  is now refused immediately and says the runtime is still busy, which is a
  different answer from a channel that has closed and is not worth retrying. The
  one command that waits is the exit path's "leave the running sessions running",
  which waits for the supervisor to take it for a couple of seconds and then warns
  rather than blocking the exit for ever.
- A start now gives back everything it acquired from one place. The ports, the
  Xray process and its temporary configuration, the browser and the session record
  used to be undone by five separate hand-written rollbacks, one per failure point,
  each of which had to remember every resource acquired before it; they now live in
  one value that undoes them when it is dropped. A failure path added later cannot
  forget one.
- Starting (or restarting) a profile that uses a proxy now asks the proxy first and
  refuses when nothing comes back. A launch used to go ahead on the strength of the
  engine binding its loopback port, which says nothing about whether the upstream
  carries traffic - so a proxy that had stopped working produced a browser that ran
  and leaked, or one that showed nothing but network errors, and the only way to
  find out was to press **Start** and look. The check is the same request the
  Proxies page sends, run on the same worker and reported in the same place, and
  the launch is queued only once a byte has come back through the engine the
  profile will use. The row reads `Starting` while it runs, **Stop** calls the
  start off, a profile with no proxy is unaffected, and a refusal names the
  profile, the proxy and the stage of the path that failed. Every wait ends
  somewhere: a reading, a stop, an edit to the proxy that was being asked, or a
  deadline, so a check nobody answers cannot leave a profile unstartable. See
  [proxy testing](README.md#proxy-testing).
- Closing the window to the background now really takes it off the screen, on
  Linux as well as on Windows. "In the background" used to mean *minimized*, which
  left the window's button on the Cinnamon panel and its entry in Alt+Tab - the
  opposite of what closing a window looks like - while Windows had already moved
  to hiding it. `crates/app/src/window_visibility.rs` hides it through the raw
  window handle GPUI hands out: `ShowWindowAsync(SW_HIDE)` on Windows, and
  `UnmapWindow` on the window's own XCB connection on X11, which withdraws it from
  the window manager, the taskbar, the window list and Alt+Tab. The tray's **Show
  window** maps it again and focuses it. Wayland keeps the compositor's minimize,
  because an `xdg_toplevel` has no withdrawn state and a surface cannot be unmapped
  without destroying it. See [exit modes](docs/exit-modes.md).
- The tray's **Quit completely** stops everything the program started in one step,
  instead of opening the exit question. The item says what it does, and the
  question is still what closing the window asks.

- The window's chrome is the sidebar, and the exit button above every page is
  gone. The content area opened with a header of its own - the product name, a
  hard-coded `v1`, the stack and a Quit button - under a title bar that already
  named the real version, which cost a row of height on every page and offered a
  second way to do what the window's own close control does. The brand and its
  version now head the sidebar, the stack and the commit moved to an **About**
  card on the Settings page, and the two rare ways out - closing through the
  saved policy and stopping everything - sit in a menu behind the sidebar's `⋯`,
  where they name what they do. The close control, the tray and the four
  remembered answers are unchanged, and a test drives the menu's close through
  the same decision the window's control uses.
- Every page now shares one header, one control and one icon. Each page used to
  draw its own title at its own size with its own spacing, and the profiles page
  was the only one with a summary line. `crates/app/src/ui/components.rs` now
  holds the baseline - page header, card, choice chip, status badge, empty state,
  icon button and the measurements they are built from - and all five pages use
  it, so a title is 22px, a control is 34px, a surface is one step rounder than a
  button and an empty page has a mark, a reason and one action. The icons come
  from one vocabulary in `crates/app/src/ui/icons.rs`: the same Lucide set the
  widget library draws with, three sizes, one stroke weight, and a colour
  inherited from the text beside it. The app's palette gained the interaction and
  accent tokens that make selection, hover and focus mean the same thing as the
  library's, `theme::apply` writes the accent into the component theme so a
  primary button and a selected row are the same blue, and the contrast test now
  holds supporting text to 4.5:1 rather than 3:1.
- A profile row offers the one action its state can carry, and the rest are in a
  menu. Start, Stop and Restart were all drawn on every row with two of them
  disabled, so a list of twelve profiles was thirty-six buttons that all had to
  be read to find the one that would work, and a stopped profile still wore a red
  Stop. The row now shows Start (neutral, quieter than the page's New Profile),
  Stop or Cancel start while a transition owns the profile, Stopping while it
  stops, and Retry after a failure; Edit, Duplicate, Open data dir, Verify
  fingerprint, Restart and Delete moved into the row's `⋯` menu, disabled where
  the runtime would refuse them. The proxy and core rows do the same, keeping
  their one repeated action (Test, Re-detect) and moving the once-per-object ones
  into a menu.
- The Runtime Details panel is opened by choosing a profile and closed by the
  reader, and where it appears depends on the window. It used to be drawn under
  the list at a fixed height of up to 320 pixels whether or not anybody was
  reading it, so the list paid for it on every frame and the panel spent most of
  its life showing a profile nobody had asked about. A window with room for both
  now gives it a 380-pixel column beside the list and keeps the two in view
  together; a narrower one lets it take the list's place and carries a **Back to
  the list** button, because a panel narrow enough to leave a readable list
  beside it would not be worth reading itself. The panel's own header names the
  profile rather than the words "Runtime Details", repeats its state for the
  covering case where the row is hidden, and holds the two controls that belong
  to the panel: asking for a fingerprint reading, and putting it away. The rest -
  edit, duplicate, open the data directory, delete - stayed where the row's menu
  already had them, the data directory's own value carries the button that opens
  it, the launch line carries **Copy args**, and the panel is now three groups -
  configuration, runtime, verification - instead of one seventeen-row list.
- A profile row is a row of columns. It was a card per profile with two lines of
  text on the left and a pile of badges, warnings and buttons on the right, which
  is to say no columns at all: nothing could be compared down the page. The list
  now has headings - profile, proxy, state, actions - and every row puts its name
  and engine, its route, its state and its one action under them, on the same
  lines at the same widths. The seed, the platform and the launch details that
  the row used to carry moved into the panel, which is where a reader who wants
  them is going; the error summary stayed, shortened to the column and complete on
  its tooltip, because a row that cannot say something is wrong is worse than a
  row that is not perfectly even.
- The profile form is three groups with the overrides folded away, and its two
  resources are chosen rather than read. Seventeen fields in one column put the
  accept-language and the CPU count at the same level as the name and the engine,
  and offered every installed core and every saved proxy as a row of chips - a
  list that grows with each engine and each provider, which is a list nobody
  scans. The form is now **Profile** (name, core, proxy), **Fingerprint** (seed,
  brand, platform, language, timezone, window, WebRTC) and **Advanced overrides**
  (brand and platform versions, accept-language, CPU cores, excluded spoofing),
  folded until they are wanted and marked **modified** when one of them has been
  changed away from what the profile arrived with. The core and the proxy are
  searchable selectors; a profile whose core or proxy is gone still shows it and
  says so rather than silently reading as something else.

## [0.1.0] - 2026-09-22

### Added

- Three ways to leave, and the question that picks between them. Closing the
  window can keep the program running in the background, leave the program while
  the browsers and proxy tunnels it started keep running, or stop everything -
  and the choice can be remembered, or changed on the Settings page. "In the
  background" means the window is minimized and a tray icon brings it back or
  leaves: a StatusNotifierItem over D-Bus on Linux (`ksni`, pure Rust, so no GTK
  or libdbus enters the packages) and the shell's notification area on Windows
  (`tray-icon`); the tray's **Quit…** always asks rather than obeying the
  remembered answer, so the tray cannot become a program nobody can leave.
- A start **adopts** the sessions a previous run deliberately left running, so
  they are running profiles again rather than leftovers to clean up. What a crash
  left is reclaimed exactly as before, and a session record that cannot be marked
  is released and logged rather than stranded. A signal still means stop
  everything. See [exit modes](docs/exit-modes.md).
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
