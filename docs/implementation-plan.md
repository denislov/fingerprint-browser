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
| Desktop UI | Profile forms, core/proxy management, link import, proxy tests, settings, runtime details, logs, a profile-list filter and a dark/light appearance switch |
| Windows lifecycle | Atomic job assignment, kill-on-close containment, native identity, recovery with retained failure records |
| Backup | Configuration export, import and restore are in, from the Settings page, along with the browser-data copy for stopped profiles - see below |
| Validation | Linux/Windows CI configuration, native process tests, opt-in real Windows Chromium/Xray acceptance |

The desktop workflow is implemented. “Phase 5 has started” is outdated, and
“only two proxy protocols” describes the manual form, not the importer/runtime.

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
- **Restore is the import read against an empty snapshot** - `plan_restore` hands
  `plan_import` a `ConfigSnapshot::default()`, so the file's copy of an identifier
  wins instead of being kept and reported as taken. The precondition lives in that
  function rather than at the call site, so a mistaken restore cannot reach a
  writer: `OnlyWhenEmpty` refuses a populated installation and carries the counts
  the window turns into a sentence. The **Restore configuration** card sits under
  the import one, and the confirmation is behind its button, only when there is
  something to replace.
- Restore **removes before it writes, in the reverse of the write order** -
  profiles, then proxies, then cores - because that is what keeps the services'
  own referential rules from refusing the removal. Browser data is not touched:
  deleting a profile keeps its directory, so a restart of the same identifier
  finds its sessions where they were, which is what makes a restored
  configuration line up with a browser-data copy.
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
   diagnostics and packaging. Do not silently download binaries. This is the
   remaining item that does not need another machine.

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

CI is configured for Windows and Ubuntu. Real browser/Xray acceptance remains
opt-in and is reported separately from the gate.
