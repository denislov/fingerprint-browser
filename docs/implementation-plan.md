# Implementation plan

Updated 2026-09-20. This page describes current status and remaining work.
Original phased plans and batch notes are preserved in
[implementation-history.md](implementation-history.md); historical next-batch
statements there are not current TODOs.

## Implemented

| Area | Current state |
| --- | --- |
| Workspace/domain/storage | Five Rust crates, domain validation, SQLite repositories/migrations |
| Direct runtime | Independent profiles, CDP readiness, cancel/stop/restart, graceful close and crashes |
| Xray | Per-profile process, six outbound builders, transport/TLS settings, fail-closed cleanup |
| Proxy diagnostics | One real request per test: exit address or a classified fault, through a temporary engine or one already running |
| Fingerprints | Version/capability checks, warnings, live read-back; Linux 142/144/148 measured; the read-back also reports the address a running browser's own traffic leaves from |
| Desktop UI | Profile forms, core/proxy management, link import, proxy tests, settings, runtime details, logs, a profile-list filter, a dark/light appearance switch and an English/Chinese language switch |
| Layout | One directory per installation: the config file lives in the data directory beside the database, the logs, the runtime files and the profiles |
| One instance | A kernel-held lock on the data directory; a second copy refuses by name, exits 3, and never reaches the database or the startup reclaim |
| Exit modes | Keep running in the background, leave with the browsers and tunnels running, or stop everything - asked, remembered, and switchable; a start adopts what was deliberately left |
| Windows lifecycle | Atomic job assignment, kill-on-close containment, native identity, recovery with retained failure records |
| Backup | Configuration export, import and restore are in, from the Settings page, along with the browser-data copy for stopped profiles - see below |
| Delivery | The program is named and versioned where it is seen: `--version` (version, commit, platform), `--help` in the chosen language, and `--diagnostics`, which writes one report about the installation for a bug report - see [diagnostics.md](diagnostics.md) |
| Validation | Linux/Windows CI configuration, native process tests, opt-in real Windows Chromium/Xray acceptance |

The desktop workflow is implemented. “Phase 5 has started” is outdated, and
“only two proxy protocols” describes the manual form, not the importer/runtime.

## One directory

The configuration file lives in the data directory, with the database, the logs,
the runtime files and the profiles. One directory is the whole installation: it is
what to look at when something is wrong, what to copy when moving machines, and
what to delete to start over. It used to live in the platform's config directory
(`~/.config/fp-browser/config.json`), which meant two places to know about and two
places a stale file could hide.

The cost is stated plainly because it is a real one: **the config file can no
longer say where the data directory is.** A file stored inside a directory cannot
decide where to look for itself, so `FP_BROWSER_DATA_DIR` (or the platform's own
directory) decides, and the Settings row for the data directory is now read-only -
its note says how to move it instead of offering a field that could not work.

- A config file left in the old location is **carried across on the first start**:
  its settings are written to `<data dir>/config.json`, the `data_dir` key is
  dropped from what is written (the next start resolves the same directory from
  the environment or the platform, and a second copy of that answer is how the two
  disagree), and the old file is removed. Nothing is written to the old location
  again.
- The one case the move cannot settle is an old file that named a **different**
  data directory. Honouring it would leave the file that names it somewhere the
  next start does not look, so it is left where it is, the run uses the directory
  it resolved, and the banner says which environment variable to set to go back.
  The row shows the ignored value as well, in the same sentence used for any value
  the environment overrides.
- `FP_BROWSER_CONFIG` still aims the program at an explicit file. That is an
  override rather than a location: a test, or a script, needs to point at a file
  without inventing a data directory for it, and an explicit path is not ours to
  migrate into.

## Proxy diagnostics

The gap this closed: `wait_ready` proves the engine bound its loopback inbound
and nothing more, and the launch path treats that as the only gate before
starting Chromium. A proxy that refuses the upstream credentials, cannot resolve
the target name or carries nothing at all passes it, and that profile then leaks -
its fingerprint and its exit address disagree.

- `runtime::diagnostic` sends one real HTTP request through the engine and reports
  the address the endpoint saw, or the class of the fault: engine, configuration,
  authentication, name resolution, unreachable, timeout, TLS, HTTP status,
  unreadable answer. The classes stay apart because they are fixed in different
  places.
- The SOCKS5 conversation is written out rather than delegated to a client
  library, and every read and write carries a deadline. `ureq`'s SOCKS connector
  performs the handshake on a worker thread and joins it, so a peer that accepts
  the connection and then says nothing held the probe for 60 seconds however the
  timeouts were configured - measured, not assumed. The protocol's own reply
  codes are also exact, where matching a library's error strings would be a guess
  at someone else's wording.
- `Engine::Temporary` starts and stops its own engine, so a proxy is testable
  before anything is launched with it; `Engine::Running` probes the engine a
  running profile is already using and leaves it alone. The temporary config,
  which holds the upstream credentials, is removed on every path including a
  failure.
- The address endpoint is plain HTTP and configurable (Settings, or
  `FP_BROWSER_ECHO_URL`). Over TLS a certificate problem would be reported as a
  proxy problem; an `https://` endpoint is refused rather than downgraded. The
  endpoint sees the exit address, which is why it is the user's choice, and a test
  runs only when the user starts one.
- The app layer reaches this through an injected port
  (`crates/app/src/proxy_tester.rs`), the same shape as the fingerprint verifier:
  the test blocks for as long as the far end takes, so it runs on a worker and
  reports through a channel the view drains.

Not established, and deliberately not claimed: that a browser was pointed at the
engine, or that a profile's remaining traffic takes the same path. Reading a
fingerprint back is the neighbouring check, not a substitute.

- **A start waits for that answer.** `AppState::begin_opening` takes the profile's
  lease and, when the profile leaves through a proxy, hands back the test job
  instead of queueing the command: the row reads `Starting`, the window runs the
  same test the Proxies page runs, and `finish_proxy_test` queues the launch on a
  reading that came back and refuses it otherwise - naming the profile, the proxy
  and the fault, and giving the profile back so the next press is possible. A
  start that joins a test already in flight waits for that one rather than asking
  twice, and a stop while the check runs calls the start off. A profile with no
  proxy has nothing to ask and is queued at once. The command is never queued
  first and checked afterwards: a launch that goes ahead produces a browser that
  is already running by the time anybody knows the proxy is down.

## Runtime exit read-back

The pre-flight asks about a *proxy*; it cannot say whether a *browser* was
pointed at it. Those come apart where it matters - a launch flag that was
ignored, a profile with no proxy, another proxy in between - and the symptom is
the one this project exists to prevent. So verification asks the browser too.

- `runtime::egress` opens a page of its own, in a tab the user is not looking
  at, at the address endpoint, and reads back the address the endpoint named.
  The request is the browser's, over the network path it was launched with;
  nothing here inspects a command line.
- The outcomes stay apart: reached and named an address, never committed to the
  endpoint, at the endpoint but not finished, answered without an address,
  could not be asked, endpoint unusable. "Never arrived" and "still arriving"
  are read from the page (`DocumentState`) rather than guessed from a timeout,
  so a page that committed somewhere else is reported promptly instead of after
  a deadline.
- `verify_egress` only judges when the proxy was *tested*: `expected` is the
  address the pre-flight measured. With no pre-flight there is no claim to
  contradict, and a reading is information rather than a finding.
- Only a profile **with a proxy** is asked. A direct profile claims nothing
  about an address, and the endpoint sees the address it was asked from.
- The read-back runs after the fingerprint reading, so a browser that cannot be
  read at all is reported as unreadable rather than as a path that carried
  nothing.
- The address goes into the activity log. The row has no space for it, and it is
  the half of the answer that says the traffic took the intended path.
- The read-back's own orchestration is covered without a browser: a stub CDP
  endpoint speaks both protocols a browser does on one port - metadata over
  HTTP, the session over a WebSocket - so readiness, creating a page of its own,
  polling the document, reading the answer and closing the page are all
  exercised in the gate. What a stub cannot answer is whether a *real* Chromium
  behaves as assumed, which is why the acceptance below stays opt-in.

Not established, and deliberately not claimed: that a page the user opened
takes the same path; this reads one document, in one tab, once.

## One instance

The lock that stops a second copy of the window from opening on the same data
directory, and the decision the plan left open: it **refuses** rather than
handing anything over.

- **What it prevents, precisely.** Both copies open the same database, and the
  second one's startup reclaim reads the first one's session records, finds the
  children they name alive, and stops them. The browsers someone is looking at,
  closed by a second launch. Packaging is what makes it easy to hit: a menu entry
  and a desktop shortcut are two ways to launch a program that is already
  running.
- **Refusing, not handing over.** Handing the arguments over has nothing to hand:
  the only invocation that opens a window carries no arguments, and the three
  that carry something - `--help`, `--version`, `--diagnostics` - are answered
  before the lock and keep working while a copy runs, which is exactly what makes
  them useful when the window will not open. Raising the existing window instead
  would be window-manager code per platform, for a menu entry that is only ever
  clicked once.
- **The lock is the kernel's, not a file this program interprets.**
  `flock(LOCK_EX | LOCK_NB)` on Unix and `LockFileEx` on Windows, both released
  when the process ends however it ends - a clean exit, a panic, a `SIGKILL`.
  There is no stale lock to age out, no pid to trust and no identity check to get
  wrong, which is the whole reason this is not a pid file. See
  `crates/app/src/instance.rs`.
- **It is taken after `--diagnostics` and before the database opens**, and held
  until `main` returns: `let _instance` in `crates/app/src/main.rs` is a named
  binding on purpose, because `let _ =` would drop the lock on the spot. A report
  about a running installation is the thing someone asks for when the window will
  not open, so it must not need the lock.
- **The holder writes a line** - pid, build, start time - that the refused run
  reads back so the refusal can name it. Nothing decides anything from that line:
  one that cannot be read still refuses, it only cannot say who. On Windows the
  lock is taken on a byte far past that line, because a Windows byte-range lock
  is mandatory and locking byte 0 would stop the refused run reading it.
- **The file outlives the run.** The lock goes with the process; `instance.lock`
  stays and the next run replaces the line. It is in the diagnostics report's
  file list, which is where a maintainer reads that a lock file's presence is not
  what holding the lock means.
- **A refused copy exits 3**, which is neither the refused-argument status nor a
  general failure, so a script can tell "already running" from "started badly".
- **What it does not cover: `Settings::load`.** The lock is taken after the config
  file has been read, because that is the call that resolves the data directory
  being locked. It is also where an installation upgraded from the old layout has
  its config file folded into the data directory - a one-time write of the same
  bytes by whichever copy gets there first, upstream of the two things the lock
  exists to protect: the database and the children.

**Not established, and deliberately not claimed:** nothing here makes two copies
on one data directory safe; it makes the second one not start. And a data
directory whose filesystem cannot hold a lock refuses the run rather than letting
it proceed unguarded.

## Exit modes

What closing the window does, and the one rule underneath it. Implemented, tray
icon included, and described in [exit modes](exit-modes.md).

- **Three answers and a question.** Keep running in the background, leave the
  program with the browsers and tunnels still running, or stop everything. "Ask
  every time" is the default and is a Settings-page choice rather than something a
  close can decide.
- **The hard case is the next start, not this one.** A start has always reclaimed
  what a previous run left, because a browser that survived a crash still holds its
  profile directory and its debugging port. A browser left *on purpose* holds the
  same things, so the answer is neither "leave it" nor "stop it" but **adopt it**:
  the session record carries a `left_running` marker, written at exit, and a marked
  record that is still alive becomes a running profile again. Unmarked records -
  a crash, and every record written before the field existed - are reclaimed
  exactly as before.
- **Adoption needs a second kind of child.** `Held::Owned` is a handle, which is
  the authority on everything; `Held::Adopted` is a record, so liveness is
  `journal::verdict` and stopping rechecks the recorded identity and stops the tree
  by pid. On Windows the job's `KILL_ON_JOB_CLOSE` limit is cleared before the
  handle is dropped, because that limit is what makes a manager that died take its
  browsers with it.
- **The question is asked before the window closes**, through
  `Window::on_window_should_close` - the hook is installed on the first frame,
  because the view is built before there is a window. A signal still means stop
  everything: there is nobody to ask.
- **The background mode needs the tray.** GPUI has no tray API and no hide of its
  own (`App::hide` is a no-op on both backends), so the window is hidden by
  `crates/app/src/window_visibility.rs` through the raw window handle GPUI does
  hand out: `ShowWindowAsync(SW_HIDE)` on Windows and `UnmapWindow` on the window's
  XCB connection on X11, with the compositor's minimize as the Wayland fallback.
  A tray icon that says so and brings the window back has two implementations:
  `ksni` on Linux (StatusNotifierItem over D-Bus, pure Rust, so no GTK and no
  libdbus enter the packages) and `tray-icon` on Windows (created on the window's
  own thread, its messages dispatched by the loop the window already runs). Both
  post into a queue the window's tick drains, so nothing calls into the UI from
  another thread. Its **Quit completely** item stops everything rather than
  obeying the remembered answer: a tray that could only re-enter "keep running"
  would be a program nobody could leave.

## Language

English and Chinese, and the switch between them, on the Settings page.

- **Every fixed label lives in one place.** `crates/app/src/text.rs` holds
  `Lang`, the `catalog!` macro (one line per message: English beside Chinese,
  both tables generated from the same list so a field added to one is a compile
  error in the other), `Text::EN` / `Text::ZH`, and `Text::PAIRS`. What a number
  or a name has to be slotted into is a **method on `Text`** rather than a field,
  because word order is part of the translation: "Showing 3 of 12 profiles" and
  "共 12 个档案，显示 3 个" do not agree on where the numbers go, and English
  needs plurals where Chinese does not. Sentence joining is per-language too
  (`join_sentences`), because Chinese does not put a space after its full stop.
- **The language is read from the state, never from a global.** This is the
  opposite of the appearance, and deliberately: the component theme is a
  process-wide fact because the library has exactly one, while the language is a
  fact about one `AppState`. Tests run in parallel, each with its own language; a
  global would make every English assertion a race against whichever test
  switched to Chinese first.
- Threading follows the palette's rule: a helper with a context does
  `let t = text(cx);` or `self.state.text()`, a helper without one takes
  `t: &Text`. The dialog forms are their own entities with no route to the state,
  so they **carry the table** (`text: &'static Text`) from the moment they are
  opened; the cost is that a switch does not repaint a dialog that is already
  open, and the benefit is that no global had to be introduced.
- **Element identifiers do not change with the language.** `Page::id()` returns
  frozen English slugs and `nav-{id}` is built from it, while the sidebar's
  visible name still comes from the table. Deriving an id from display text is
  the classic localisation trap: switching the language would make the click
  target of every test and every script disappear. `SettingKey::effect()` returns
  an `Effect` enum rather than the sentence for the same reason - program
  behaviour must not depend on the language in force.
- **English is the default.** No `lang` in the config file, or a value this build
  does not know, starts in English rather than refusing to start; `zh-Hans` and
  `zh-CN` are the same language as far as this build is concerned. The choice is
  written **before** the switch, like the appearance: a config file that cannot be
  written refuses it rather than showing a language that would be gone by the next
  start.
- The chips are labelled **in their own language** (`English` / `简体中文`), and
  those two words are deliberately *not* in the catalog: someone who cannot read
  the language they are in can only find the way out through an endonym.
- What stays English, at its call site: product and protocol names (`SOCKS5`,
  `Chrome`, `Windows`, `FP_BROWSER_*`), the JavaScript property names on the
  Runtime Details panel, and the **innermost text of a fault** - what `runtime`,
  `domain` and `storage` said, a parse error, an `io::Error`, the exit status of
  `xdg-open`. The window translates the frame and leaves the evidence as it came,
  so that a bug report matches the log. The one exception is `FaultClass`, a
  closed enum, which the window does translate. See `docs/i18n.md`.
- The app's own non-view modules are covered too: `core_detect`, `paths`,
  `reclaim`, `verifier`, `open_dir` and `settings` take `t: &Text` for the
  sentences they hand over, and `main.rs` resolves the table once, after the
  config file is read, so startup notices already speak the chosen language.
- The catalog is held to its own tests: a message that is identical in both
  languages fails unless it is on an explicit allowlist of names that are the
  same on purpose (`Xray PID`, `CDP port`, `WebSocket`), and the catalog's size is
  asserted so a shrunken table cannot pass vacuously.

## Appearance

Two palettes and the switch between them, on the Settings page.

- **The component theme is the single source of truth for which mode is in
  force.** `crates/app/src/theme.rs` holds `ThemeChoice` (stored as `dark` or
  `light`, unknown names falling back to dark so a file from a later build still
  starts) and `Palette`, two sets of semantic colours. The window paints its own
  chrome, and the component library themes its own widgets; if the two were
  switched separately a button's outline would vanish into the card behind it. So
  `Theme::change` moves both, and `palette(cx)` reads the mode back out of the
  component theme rather than from a second switch. The two layers cannot
  disagree, because there is only one of them.
- The refactor that made this possible was mechanical but wide: the six module
  constants and the scattered hex literals in `crates/app/src/ui.rs` - 96 call
  sites - became `Palette` fields. A helper with a context does
  `let p = palette(cx);`; a helper without one takes `p: Palette` by value, which
  is why every render path still says which palette it is painting with. The
  dialog forms (`editor.rs`, `proxy_editor.rs`, `core_editor.rs`,
  `proxy_import.rs`) went the same way.
- The card is **first on the Settings page**, and the control is a pair of chips
  rather than a toggle: a toggle hides the other option behind the label of the
  mode you are not currently looking at, which is the one thing the reader cannot
  check against the window in front of them.
- **It is the one setting whose effect is now.** Every other card on the page
  describes what the next start will do; this one repaints immediately. It is
  still a setting, so it is written **before** the theme changes: a config file
  that cannot be written refuses the switch rather than showing a mode that would
  be gone by the next start.
- The component theme is process-wide, so the two tests that assert on it take a
  shared lock (`theme::testing::exclusive`). Without it they would each observe
  the other's palette and fail on a race rather than on a mistake - which is a
  gate that fails for reasons unrelated to the code.
- "Chosen for contrast" is **checked, not asserted**. `both_palettes_have_readable_contrast`
  computes WCAG 2.1 ratios for every text-on-background pair, and every status
  colour twice: on the window and on the tinted background it is paired with in a
  badge. The light palette failed this on the first run - three status colours
  read at 3:1 or just under as text - which is exactly the kind of thing an eye
  cannot reliably catch and a test can.

The window boots into the stored choice, applied before the first frame, so it is
never painted in one palette and then corrected.

## Profile filter

A text filter over the profiles list, and nothing more than a filter.

- It matches, case-insensitively, the name, the fingerprint seed, the brand, the
  platform, the core and the proxy - `answers_to` in `crates/app/src/state.rs` is
  the one place the rule lives. The seed is included because it is the one number
  that identifies a profile whose name has stopped meaning anything; the core and
  the proxy because they are what someone scanning similar-looking profiles is
  looking for, and neither is in the name.
- It matches what is *stored*, not what the row renders. A profile with no proxy
  cannot be found by typing the word the row prints for having none.
- Nothing is started, stopped, deleted or reassigned. A row the filter hides
  keeps running, keeps its reading, and stays the profile the details panel
  answers for - the panel is about what was chosen, not about what is listed.
- The header says "Showing N of M profiles" while a filter is on, because that is
  how the user finds out the list is not the whole list; its accessible name also
  names the term. With no filter it goes back to explaining what a profile is.
- An empty list says which kind of empty it is. No profiles at all, or all of them
  hidden - and only the second offers to clear the filter, because only the second
  can be undone from the list.
- The field is built on the first render and kept, because an `InputState` needs a
  window and neither `AppView::new` nor `boot` has one. It is observed rather than
  subscribed to change events: the field notifies on every edit, and reading its
  value is what the listing needs, so there is no event kind to match on.
- `InputState::set_value` deliberately emits no change event, so the field's own
  clear button and the empty state's button both clear the filter themselves
  instead of waiting to be told. A test pins that, because it is the kind of thing
  that breaks silently.

## Backup and restore

**Implemented in full.** All six slices of the design in
[backup-and-restore.md](backup-and-restore.md) are in: the credential rule in
`domain`, the exchange document and the writing of it in `application`, the
export card on the Settings page, the import that reads a backup back under the
never-overwrite rules, the restore that makes the installation be the file, and
the browser-data copy for stopped profiles. The rules came first because three of
them decide the shape of the code rather than the other way round: what counts as
a credential, what happens to an identifier that is already taken, and what
happens to a path that does not exist on this machine.

What the design fixes, and why it is worth fixing before writing the code:

- **Two artifacts, not one.** A configuration backup is kilobytes of JSON; a
  browser-data backup is hundreds of megabytes of cookies per profile. A single
  "back everything" would make the common case - move my configuration to a new
  machine - cost gigabytes and leak a second copy of every session cookie to do it.
- **The configuration file is a versioned JSON document carrying the domain type
  shape**, with `format` and `version` first so a reader refuses before parsing
  anything else. An unknown `format` is refused, not guessed at.
- **Credentials are included or excluded per export.** "What counts as a
  credential" lives in one exhaustive `match` in `domain`, next to the outbound
  types, so a new protocol cannot be added without deciding; there is one test per
  protocol rather than one for the protocols someone remembered. A stripped proxy
  is marked as needing credentials and never looks usable.
- **Restore and import are not the same verb.** Restore means "be this file" and
  refuses a non-empty database without an explicit confirmation; import means "add
  these" and never overwrites. Both keep an identifier that is free and never
  overwrite a taken one - which is what makes the two artifacts line up again.
- **`user_data_dir` is kept when the directory exists and re-pointed at the local
  default when it is not**, because a path from another machine was never going to
  be right, and the alternative is a profile that starts with no sessions while
  claiming a directory it does not have.
- **`<data dir>/runtime/` and `logs/` are never in a backup.** The first holds the
  same upstream credentials in the transient Xray config, on a path the program
  actively cleans; the second is a record, not configuration, and it names proxies
  and exit addresses.

Implemented so far, and where it lives:

- The credential list is `ProxyOutbound::without_credentials` in
  `crates/domain/src/proxy.rs` - one exhaustive `match`, so a protocol cannot be
  added without deciding, with tests for transport settings surviving and for the
  rule being idempotent.
- `carries_credentials` sits beside it and is pinned to agree with it: an
  outbound carries credentials exactly when stripping changes it. An export that
  reports "none were written" while the file still holds a password is the failure
  the pair exists to prevent.
- What "a stripped proxy is not a working proxy" actually covers turned out to be
  narrower than the design first said. Shadowsocks, VMess, VLESS and Trojan
  require credentials, so a stripped one is already refused by `validate_proxy`;
  SOCKS5 and HTTP authenticate optionally, so a stripped one still validates. The
  file therefore records the choice rather than leaving an import to infer it, and
  a test pins which protocols fall on which side.
- `crates/application/src/config_backup.rs` holds the document: `format` and
  `version` first, the header read on its own so a foreign file is refused as
  foreign rather than as damaged, `deny_unknown_fields` so a field this build does
  not know is refused rather than dropped, and an `Excluded` export tested by
  searching the serialised text for the secret.
- `crates/application/src/export.rs` reads the three lists and writes the file,
  reporting what it wrote. The **Export configuration** card on the Settings page
  is the way in: a path field, a checkbox for the credentials, and a sentence
  that distinguishes "left out" from "there were none to leave out" - the second
  tells the reader the file is not the whole configuration, and neither is
  allowed to read as the other.
- The default destination is `<data dir>/exports/fp-browser-config-<stamp>.json`,
  with the stamp spelled `YYYYMMDD-HHMMSS`: a clock time's colons are not legal in
  a Windows path. Naming a new file each second is what makes it safe to write
  over an existing path at all, since an export replaces what is there.
- `crates/application/src/import.rs` is the other direction, split into a pure
  planner and an applier. The planner decides, from the document and the three
  lists, what would be added, kept, skipped and re-pointed; the applier writes in
  dependency order - cores, proxies, profiles - reading the identifiers back
  between phases so a record the database refuses takes its dependents with it as
  skipped. **Not one transaction**, correcting the design's first draft: the
  storage traits have no transaction seam, and ordering plus read-back is the
  better answer anyway, since one unreadable proxy should not take a good import
  down with it. Import is idempotent - the same file twice adds nothing the
  second time, and says so.
- The **Import configuration** card sits under the export one. A shortfall -
  a kept item that differs, a skipped profile, a profile that arrived without its
  proxy - goes to the banner and stays until dismissed; a clean import is a
  toast. A moved directory is reported in the sentence but raises no alarm,
  because moving every directory is what importing onto another machine *does*.
- **Restore is not the import read against an empty snapshot.** It was, and the
  half that mattered was wrong: an import that writes most of a file and reports
  the rest is a good import, and a restore that does the same is an installation
  that is neither what it was nor what the file asked for. Restore has its own
  planner - every record validated, repeated identifiers and dangling references
  refused, a file exported without credentials refused with its own sentence - and
  its own applier, which replaces all three lists in one transaction
  (`ConfigurationRepository::replace`): a refused record, a broken reference or a
  crash mid-way leaves the old configuration exactly where it was. The file's copy
  of an identifier still wins over a stored one, and the precondition still lives
  in the planner rather than at the call site, so a mistaken restore cannot reach
  a writer: `OnlyWhenEmpty` refuses a populated installation and carries the
  counts the window turns into a sentence. The **Restore configuration** card sits
  under the import one, and the confirmation is behind its button, only when there
  is something to replace.
- Browser data is not touched: deleting a profile keeps its directory, so a
  restart of the same identifier finds its sessions where they were, which is what
  makes a restored configuration line up with a browser-data copy.
- `crates/application/src/browser_data.rs` is the second artifact: each
  `profiles/<id>` directory copied to or from a directory the user names, whole
  and replacing rather than merging. Only stopped profiles are copied, and the
  refusal names the running ones before anything is written. A profile that has
  never run has no directory to copy, which is a skip and not a failure. Pointing
  the backup at the profile's own directory is refused by name, because clearing
  the destination first would delete the source.
- The **Browser data** card has one directory field and two buttons, and the copy
  runs on a worker with an injected `BrowserDataCopier` port - it can block for
  seconds, so the window must not freeze, and a test must be able to press the
  button without hundreds of megabytes being written. Restore and the copy both
  refuse while a profile is running, checked against a freshly reconciled runtime
  snapshot rather than a cached row.

Encryption is deliberately **not** decided, and the current recommendation is no:
without a key-management answer an encrypted file is plain text with extra steps.
Copying the data directory with the program closed remains a lossless backup that
needs no code - the page says so, and says what it does not buy.

## Windows lifecycle acceptance

- [x] Chromium/Xray enter private jobs atomically at creation.
- [x] Stop/rollback cleans descendants even after the leader exits.
- [x] Killing the manager closes its jobs and preserves unrelated processes.
- [x] PID + native creation time identifies records; mismatches are refused.
- [x] Failed reclamation retains records; stale records are removed.
- [x] Real Chromium: simultaneous profiles, cookie isolation/persistence,
  cancellation, normal close, crashes, Xray failure, shutdown and manager crash.
- [ ] Reproduce remote upstream forwarding on Windows.
- [ ] Measure Windows fingerprint surfaces independently of Linux evidence.

Commands and limits: [windows-acceptance.md](windows-acceptance.md).

## Next bounded tasks

1. Profile search: a filter over the list, matching name, seed, core and proxy.
   Deliberately not selection or batch operations - see below. **Done.**
2. Backup/restore: export, import, restore and the browser-data copy are all in,
   following the rules in [backup-and-restore.md](backup-and-restore.md).
   **Done.**
3. Distribution: repeatable Windows release build, first-run executable setup,
   diagnostics and packaging. Do not silently download binaries. **Done, except
   for the part only a release can prove** - the Windows installer and the
   workflow that builds it have not been run anywhere yet.
   - [x] **Self-description.** `--version` (version, injected commit, platform),
     `--help` in the language the config file names, the version in the window
     title, and the same line as the first line of the activity log.
   - [x] **Diagnostics.** `--diagnostics` and a Settings-page button write one
     markdown report: build, effective settings with their sources, the state and
     permissions of every file the installation keeps, and the end of the log. It
     never opens the database and never reads browser data. See
     [diagnostics.md](diagnostics.md).
   - [x] **Release build.** `[profile.release]` in the workspace manifest, each
     setting with its reason beside it; the version in one place and the commit
     injected from git; a CHANGELOG with the version policy; and the icon, PNG,
     ICO and SVG, generated by `scripts/make-icon.py`. See
     [release.md](release.md).
   - [x] **First-run setup.** The empty profile list names both ways to supply a
     core and carries the button that leads to the one inside the window; the
     walk from that empty state to a running profile is written down step by step
     in [first-run.md](first-run.md) and pinned end to end by a UI test that
     deletes the seeded core, adds one through its own form, creates a profile
     with it and starts it.
   - [x] **Packaging.** `packaging/linux/package.sh` (an archive with an installer
     for the current user), `packaging/windows/package.ps1` over an Inno Setup
     script, the icon and version in the executable's resource section, and a
     `v*`-tag workflow that builds both, refuses a tag or a changelog that
     disagrees with the version, and attaches the artifacts with their checksums.
4. **Two instances at once.** A second copy of the window would open the same
   database, and its startup reclaim would look at the first one's children and
   could stop them - and packaging makes it much easier to hit, since a menu entry
   and a desktop shortcut are two ways to launch something already running.
   **Done:** the data directory is locked with the kernel's own lock, taken after
   the command-line questions and before the database opens, and a refused copy
   says who holds it and exits 3. See [One instance](#one-instance).
5. **The first release.** Nothing here has been published, so the Windows
   installer, the release workflow and the artifact names have been read but not
   run. Cutting `v0.1.0` - after the changelog has a section for it, which the
   workflow insists on - is the task that turns all of that from written to
   verified.


### Why distribution is a task and not a paragraph

Everything above is a feature of a program that only its author can build. The
remaining work is what makes it a thing someone else can install, and it is
deliberately not "add an installer": each piece has a rule behind it.

- **The build has to describe itself.** Two builds of `0.1.0` are not the same
  program, so `--version` and every report carry the commit, injected at build
  time by `crates/app/build.rs` rather than written in the source. A bug report
  without it is a conversation; with it, it is a checkout.
- **The first question has to be answerable without the window.** A program whose
  only surface is a GUI cannot be asked anything by a script, and cannot say
  anything when the window will not open - which is exactly when someone needs to
  ask. Hence `--version`, `--help` and `--diagnostics`, and hence the rule that
  `--version` reads nothing and `--help` writes nothing.
- **Nothing is downloaded for the user.** Chromium and Xray are the user's, and
  the program never fetches them. That decision is what makes the first run
  something to explain rather than something to automate; the empty state points
  at the core page and names `FP_BROWSER_CHROMIUM_BIN` instead of offering to
  install a browser.
- **A report must be safe to send.** The proxy credentials are in the database,
  so the report measures it and does not read it, and the summary says what is in
  the file before anyone shares it.

**Done means:** on a clean Windows machine, from an artifact this repository
produced, a user can install it, see which build they have, be told what to
supply when there is no browser core, and produce one file to attach to a report
- and none of it required a download the program made by itself.


### Why profile search stays a single-profile feature

An earlier version of task 1 read "search, selection and bounded batch
start/stop/proxy assignment, with per-profile results and cancellation". It was
dropped after reading the command path, not for lack of room to build it:

- **The supervisor is serial, so a batch would only look parallel.** Commands
  arrive on a `crossbeam_channel::bounded(256)` and `run()` handles them one at a
  time on one thread. `start_profile` waits inline for Xray readiness (5s) and CDP
  readiness (12s), so N profiles cost up to N x 17s no matter what the UI says.
- **A batch command cannot report per-profile success.** `RuntimeService::start`
  reads storage, sends, and returns; the browser comes up later, as an event. The
  only honest per-profile answer is a snapshot read, which is what the row already
  shows.
- **Cancelling already works.** A `Stop` for a starting profile aborts it, and
  `poll_start_commands` drops that profile's queued `Start`/`Restart` commands.
  Nothing needed to be invented - but nothing needed a batch either.
- **The scale does not call for it.** This is a personal tool with a handful of
  profiles on screen. A selection model, a selection bar and a destructive batch
  verb are cost with no matching benefit; a filter is the whole of the demand.

So the list gains a filter and nothing else. Filtering is a way of looking: it
never starts, stops, deletes or reassigns anything, and a profile it hides keeps
running and stays the one the details panel answers for. Those properties are
pinned by tests in `crates/app/src/state.rs` and `crates/app/src/ui.rs`.

## Required gate

scripts/check.ps1 (Windows) and scripts/check.sh (Linux) run:

1. cargo fmt --all --check
2. cargo check --workspace
3. cargo test --workspace
4. cargo clippy --workspace --all-targets -- -D warnings

Linux additionally parses the packaging scripts (`sh -n`) and reads the
changelog's `Unreleased` section, so a syntax error in a packaging script or a
broken changelog pattern is caught on the push that introduced it rather than on
the day of a release. Building a release artifact is deliberately not in the
gate: it takes minutes, and the release workflow is where it belongs.

CI is configured for Windows and Ubuntu. Real browser/Xray acceptance remains
opt-in and is reported separately from the gate.
