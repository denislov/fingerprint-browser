# UI shell

The window is one GPUI view: `AppView` in `crates/app/src/ui/mod.rs`, over
`AppState` in `crates/app/src/state/mod.rs`. It draws five pages - Profiles,
Proxies, Cores, Log, Settings - under a sidebar, opens the dialogs and editors
that sit above them, and forwards every user action into the state. The view
owns no runtime state (the runtime facade's snapshots are the single source of
truth), and the work that would block the window is injected as a port and run
off the drawing thread. The window's close question and the tray are
[exit modes](../exit-modes.md); the words are [interface text](../i18n.md).
The layout's design history is the [UI beautification plan](../ui-beautification-plan.md)
(2026-09); what it left unverified is tracked as
[issue #10](https://github.com/denislov/fingerprint-browser/issues/10).

## Responsibilities and boundaries

What this subsystem owns:

- The five pages, their order and the sidebar between them: `Page` in the read
  models (`state/rows.rs`), and `PAGES`, `sidebar` and `nav_item` in
  `crates/app/src/ui/mod.rs`. The order is Profiles, Proxies, Cores, Log,
  Settings.
- The window chrome: the brand head, the rail switch (`brand`, `rail_head`,
  `sidebar_toggle`), the sidebar footer's version and its menu (About, Close
  window, Stop everything and quit), and the one-line notice banner above each
  page's body (`notice_banner`).
- The rows, columns and empty states each page draws - `ui/profiles.rs`,
  `ui/proxies.rs`, `ui/cores.rs`, `ui/logs.rs`, `ui/settings.rs`, `ui/xray.rs` -
  the Runtime Details panel (`ui/details.rs`), and the shared pieces in
  `ui/components.rs` (`PageHeader`, `card`, `chip`, `status_badge`,
  `EmptyState`, `icon_button`, `menu_item`).
- The dialogs and editors over the view: the profile, proxy, link-import and
  core forms (`editor.rs`, `proxy_editor.rs`, `proxy_import.rs`,
  `core_editor.rs`), the setting-value dialog, the exit dialog, the delete and
  restore confirmations, and the Download Core and Download Xray dialogs
  (`ui/cores.rs`, `ui/xray.rs`).
- The view's own state: which page is shown, the selection and details panel,
  the filter text, the log filter and expanded lines, the settings group, and
  the focus and dialog bookkeeping held in `AppView`'s fields.
- Blocking work as ports: `FingerprintVerifier`, `ProxyTester`,
  `DirectoryOpener`, `BrowserDataCopier`, `ReleaseCatalog`/`CoreDownloader`,
  `XrayCatalog`/`XrayDownloader` and `TrayStarter`. These are the eleven
  arguments of `AppView::new` - the state and the runtime event receiver, plus
  nine ports, each a seam a test replaces in `crates/app/src/ui/tests.rs`.

It does not own:

- What closing the window, the tray and one window per data directory do:
  [exit modes](../exit-modes.md).
- Any interface string, when a language switch applies, or why element ids are
  frozen: [interface text](../i18n.md).
- What an export, import, restore or browser-data copy does:
  [backup and restore](../backup-and-restore.md).
- The diagnostics report: [diagnostics](../diagnostics.md).
- Runtime state, launch, stop and adoption: [runtime lifecycle](../lifecycle.md)
  and the service signatures in [service contracts](../service-contracts.md).
- Type definitions (profiles, cores, proxies, snapshots):
  [domain model](../domain-model.md); what a fingerprint claim means:
  [fingerprint matrix](../fingerprint-matrix.md).

The v1 UI sketch in [architecture](../architecture.md) section 15 lists the
same five pages as `PAGES`; the code's five pages are the reference.

## Source of truth

- `crates/app/src/ui/mod.rs` - `AppView`, its fields and `Render` impl, the
  sidebar and brand helpers, `profiles_workspace`, `notice_banner`,
  `push_toasts`, and the measurements `TICK`, `RECONCILE_EVERY`,
  `DETAILS_DOCK_REST`, `PROFILE_FILTER_WIDTH`, `PAGES`, `NAV_ROW`,
  `SIDEBAR_HEAD`.
- Per page: `ui/profiles.rs`, `ui/proxies.rs`, `ui/cores.rs`, `ui/logs.rs`,
  `ui/settings.rs` plus `ui/settings/actions.rs`, `ui/xray.rs`, `ui/details.rs`;
  shared chrome and controls in `ui/components.rs`; the pieces the two download
  dialogs share in `ui/downloads.rs`; icon and image assets in `ui/icons.rs`.
- The window's own keyboard: `ui/keys.rs` (`actions!`,
  `FocusProfileFilter`, `CloseDetails`, `bind`).
- Background polling and answer dispatch: `ui/workers.rs`
  (`AppView::poll_runtime` and the `drain_*` methods).
- Window close, tray and background-mode routing: `ui/lifecycle.rs`.
- The state the view renders and writes: `state/mod.rs` (`AppState`), the read
  models in `state/rows.rs`, and the per-subject operations in `state/profiles.rs`,
  `state/proxies.rs`, `state/cores.rs`, `state/configuration.rs`,
  `state/activity.rs`, `state/releases.rs`, `state/xray_releases.rs`.
- The dialog bodies: `editor.rs` (`ProfileEditor`, `ProfileEdit`),
  `proxy_editor.rs` (`ProxyEditor`), `proxy_import.rs` (`ProxyImport`),
  `core_editor.rs` (`CoreEditor`).
- Wiring and appearance: `crates/app/src/main.rs`, `theme.rs`
  (`ThemeChoice`, `Palette`, `apply`, `palette`, `rail_spring`),
  `window_visibility.rs`, `tray.rs`, `text.rs`.

Where this page and the code disagree, the named symbol wins.

## Main runtime paths

### Boot, and the one storage read

`main` builds storage, the supervisor, the services and `AppState`, then opens
the window through `gpui_kit::application().run`: it registers `ui::icons::AppAssets`,
binds `ui::keys::bind`, applies the stored appearance with
`theme::apply(app_state.theme(), cx)` before the first frame, and creates a
window 1200x680 with a 960x560 minimum and `version::window_title()` in the
title bar. Inside it, `AppView::new` receives the state, the runtime event
receiver and the nine ports, and `AppView::boot` runs once: `AppState::load`
reads storage (the one read `boot` makes - `main` reads storage for its own
startup checks but deliberately does not pre-load the rows; later reads are the
per-action reloads and the page switches to Proxies and Cores), a focus handle
is made, `on_window_closed` is
installed (quitting once the window list
is empty, which is what lets `main`'s shutdown reclaim children on every
platform), the two global actions are registered, and `poll_runtime` starts.
`boot` discards `load`'s `Result`: a failure is already recorded by `load`
itself through `set_notice`, so the window opens with the error in the banner
and no rows.

### The tick, and when the window redraws

`AppView::poll_runtime` wakes every `TICK` (200 ms). Each tick drains what the
workers reported: runtime events (`drain_events`), verifications, proxy tests,
directory-opener results, browser-data copies, maintenance outcomes, and the
two release streams (`drain_verifications`, `drain_proxy_tests`,
`drain_open_results`, `drain_browser_data`, `drain_maintenance`,
`drain_releases`, `drain_xray_releases`). Every fifth tick (`RECONCILE_EVERY`)
it reconciles unconditionally, in case a notification was dropped.

`AppState::refresh_runtime` then re-reads each profile's snapshot and says
whether anything the list draws moved. The window is redrawn only when
something did: the tick calls `cx.notify()` when a queue drained, a snapshot
changed, or `AppState::clock_driven` is true - which it is on the Log page and
while the details panel is open, the two places whose text (line ages, uptime)
is read off the clock rather than off the configuration. A window that is not
drawing any of those stays still. Every user action that changes something
calls `cx.notify()` itself, so a click is redrawn without waiting for a tick.
Queued toasts are pushed into every open window's notification layer at the end
of the tick, and tray requests are handled per window there; a tick that finds
the window list empty quits the app, the same safety net `boot` installs.

### One action end to end: starting a profile

A row's Start button (`on_start` in `ui/profiles.rs`) calls `open_profile`,
which asks `AppState::begin_opening`. The state answers with a `StartGate`:
`Queued` when there was nothing to ask, `Awaiting` when a test of the same
proxy is already in flight, `Checking(job)` when the proxy needs a fresh
reading. Only the `Checking` arm starts a worker in the view: it spawns a
thread running the injected `ProxyTester`, whose answer
arrives through the `proxy_tests` channel and is applied by
`AppState::complete_proxy_test` on a later tick - a passing answer queues the
launch, a failing one is refused with a sentence naming the stage that failed.
The view never blocks on the test and never touches the runtime itself;
commands reach it through `AppState` -> `RuntimeService` ->
`ChannelRuntimeFacade`. The gate itself - when a start waits, and what a refusal
means - is owned by [Traffic verification](traffic-verification.md).

### The other workers

Every other long-running action has the same shape - build a job in the state,
run it on a `std::thread`, send one answer into a channel the tick drains:

- Verify: `begin_verification` -> `verifier.verify` on a thread ->
  `complete_verification`, which drops an answer whose browser has since
  stopped or restarted.
- Test a proxy: `begin_proxy_test` -> `tester.test` -> `complete_proxy_test`,
  which drops an answer for a proxy that was edited or deleted meanwhile.
- Open a directory: `DirectoryOpener` -> the `open_results` channel -> a
  notice saying it opened, or why it did not.
- Export, import, restore, redetect and core save: a `maintenance::Outcome`
  through the `maintenance` channel; `drain_maintenance` applies it and
  `settle_core_save` answers the form that asked, matched by its token. Only one
  configuration task may be in flight at a time. A second export, import,
  restore or redetect requested while one is running is refused, and the refusal
  is said through the notice path: `AppState::begin_maintenance` returns the
  `maintenance_busy` sentence, "Another configuration task is still running;
  wait for it to finish." The reason: the four read and write the same file and
  the same rows, and the window has one button rather than two
  (`maintenance::Kind` is exactly `Export`/`Import`/`Restore`/`Redetect`).
- Browser data: `BrowserDataCopier` -> the `browser_data` channel.
- The two release dialogs: the catalog lists on a worker when the dialog asks
  for it (the first open, or the Refresh button); a download reports `Progress`
  and `Downloaded` through its own channel, and the finished core is registered
  by a second worker (`AppState::finish_core_download` hands back a job that
  becomes an `Outcome::CoreAdded`). `sync_downloads` and `sync_xray_downloads`
  copy the state into the dialog entities after each answer.

### Motion

The window draws itself, so it moves itself, and the values are the design
system's rather than this program's: `Theme::motion` (`MotionTokens`, from
`gpui_kit::component::theme`) holds the durations, the enter/exit/move curves,
the two springs and the two distances, in the same way it holds the colours
[`palette`] reads back. What the window decides is *which* token each of its own
transitions uses - `theme::rail_spring` is the whole of that list today, and the
reason for its choice is on the function.

**One transition is the window's own: the sidebar's width.** `sidebar` samples
`base::motion::spring("sidebar-width", width, rail_spring(cx), window, cx)` and
lays out the value it returns, so folding the rail is watched rather than
jumped. Three properties of that helper are why it is the one used:

- it is keyed by identifier and keyed state carries position **and velocity**,
  so a toggle pressed twice redirects the rail from where the first press got to
  instead of restarting it;
- a spring at rest returns its target after a read and two comparisons, so a
  settled sidebar asks for no frames at all;
- it reads `App::reduce_motion` itself and settles on the target when the setting
  is on, which is the same thing `AnimationExt` does for the component library's
  dialogs and notifications - so a reader who has asked their system for less
  motion gets a window that folds instantly rather than one that folds slowly.

Nothing else in this window's own chrome is animated. A page change is a
*remount*, not a value that moves in place, and the only helpers that replay on a
remount sample the wall clock - which would make the check that guards them a
check on how fast the machine ran. A colour or an opacity change is not
observable from the headless test API at all (`ElementSnapshot` carries bounds,
labels and roles), so it would ship as a claim nothing could falsify. What the
component library draws - dialogs, sheets, notifications, the download progress
bar - animates on its own and is not this window's to configure.

## State and transitions

`AppState` holds what the window shows; the table lists the transitions a user
can put it through. None of these fields is runtime state.

| Field | Meaning and transitions |
| --- | --- |
| `page` | The page in force. `set_page` switches it, and re-reads the rows when the page is Proxies or Cores (their usage lists come from the profiles). |
| `selected`, `details_open` | `select` sets both, so choosing a profile opens the Runtime Details panel. `close_details` closes the panel and keeps the selection, which is why the row stays lit. A selection whose profile has gone is dropped by `load_rows`. |
| `profile_filter` | The Profiles page's filter text. Narrowing affects the list only: a filtered-out profile keeps running and stays the panel's subject (`visible_rows`, `answers_to`). It matches case-insensitively (`answers_to` in `crates/app/src/state/rows.rs` is the one place the rule lives) against the name, the fingerprint seed, the brand, the platform, the core and the proxy - the seed because it is the one number that identifies a profile whose name has stopped meaning anything, and the core and the proxy because they are what someone scanning similar-looking profiles is looking for and neither is in the name. It matches what is stored, not what the row renders: a profile with no proxy cannot be found by typing the word the row prints for having none. |
| `details_tab` | Which of the panel's three views is shown (`DetailsTab::ALL`). The effective launch-args view is kept from v1 on purpose: it is worth a lot when investigating version-compatibility problems. |
| `log_filter`, `expanded_logs` | The Log page's level filter and the lines the reader has opened. A line is keyed by its timestamp, not its index, so new lines do not move it; the set is trimmed with the log's `LOG_CAPACITY`. |
| `settings_group` | Which of the Settings page's four groups is on screen (`SettingGroup::ALL`: General, Runtime, Data, Diagnostics). Kept in state so leaving the page and returning does not move the reader. |
| `theme`, `language`, `exit_mode`, `sidebar_collapsed` | Stored settings. Each is written before it is shown or applied; `sidebar_collapsed` and `theme` repaint in the same frame, `language` repaints the whole window (and restarts a live tray icon in the new language), and `exit_mode` only moves its chip because it is read when a window is closed. |

`AppView`'s own fields hold what is not state: the open editor entities
(`editor`, `proxy_editor`, `proxy_import`, `core_editor`, `setting_editor`,
`downloads`, `xray_downloads`), the core-save waiter and its sequence number,
the input fields the pages build on their first render, the per-profile focus
handles (`row_focus`), the deferred focus flags (`focus_filter`, `focus_row`,
`escape`), the tray, and `close_hook`.

Rows present runtime state without owning it: `ProfileRow` carries the display
names plus a `snapshot` copy, and `ProfileRow::state()` answers `Starting`
while a start is waiting for its proxy (`checking_proxy`) or queued
(`queued_start`), otherwise the snapshot's state, otherwise `Stopped`.

Appearance is a standing choice of the same kind. `ThemeChoice` is Dark (the
default, and what a stored name this build does not know falls back to) or
Light; the Appearance card's two chips store the choice, and only when the
store succeeded does `theme::apply` switch the component theme and write this
window's accent family into it, so the widgets the library draws and the chrome
the window draws cannot disagree. Every render reads the mode back out of the
component theme through `palette(cx)`, so there is one switch, not two.

## Constraints and failure behavior

- Nothing blocking runs on the thread that draws. Every worker port
  (`FingerprintVerifier`, `ProxyTester`, `DirectoryOpener`, `BrowserDataCopier`,
  the two catalogs and downloaders) is called on a `std::thread` and reports
  through a crossbeam channel; the state's own writes (creating, editing,
  deleting, duplicating, settings) stay synchronous because they are local, and
  the one file task that runs inline is the diagnostics report
  (`AppState::write_diagnostics`). The tray's starter is the one port called
  where the desktop API requires it, not on a worker.
- The view never owns runtime state. Every read goes through
  `RuntimeService::snapshot`; the list's copy exists to be drawn, and
  `refresh_runtime` is the documented reconciliation point.
- Dialog bodies are separate entities because a dialog is drawn by the `Root`
  while the window it belongs to is being read: the two download dialogs
  (`CoreDownloads`, `XrayDownloads`) hold copies and are updated by `sync_*`,
  and the exit dialog keeps its checkbox in a cell because it is rebuilt every
  frame. The view can only test for a dialog through the window
  (`window.has_active_dialog`), which is why Escape is handled where the
  window is.
- Escape closes one layer at a time: a dialog first (deferred, through
  `window.close_dialog`), otherwise the details panel. Ctrl-F puts the keyboard
  in the profile filter from anywhere, but not while a dialog owns it. Both are
  bound without a key context, so a more specific binding wins on the layer
  above.
- The first frame gives the keyboard to the window body (a window that opens
  with nothing focused answers Tab with nothing); a profile row is a tab stop,
  Enter opens its panel, and closing the panel returns the keyboard to the row
  it was opened from.
- Layout is measured from the viewport each frame: the details panel docks when
  the width is at least `DETAILS_DOCK_REST` plus the sidebar's width and covers
  the list otherwise; the Proxies and Cores tables move metadata under the name
  below `components::resource_table_min`; the log message clamp is computed
  from the window width minus `logs::MESSAGE_OFFSET`. Sidebar widths are
  `components::SIDEBAR` (200) and `components::SIDEBAR_RAIL` (56).
- Failures reach the reader two ways: an error owns the one-line banner above
  the page until it is dismissed, while a message that is not a problem clears
  the banner and is only a transient toast. Either way the sentence is kept in
  the activity log, and one too long for the banner is truncated there with the
  whole text on its tooltip and in the log. A refusal from a dialog's own save
  is shown inside the form and the dialog stays open.
- A worker answer that no longer matches its ask is dropped: a verification
  whose browser restarted, a proxy test whose proxy changed. A core save whose
  form was closed is not delivered to the form; its answer is said through the
  notice path instead - the banner for a refusal, the activity log and a toast
  either way. A start waiting on a proxy that never answers is cancelled after
  `CHECK_LEASE` (60 s) and gives its profile back.
- A desktop that refuses the tray icon keeps the window and says why in the
  banner, because a hidden window with no tray would be unreachable.
- The view never draws what a page is not showing: each page's rows are
  gathered only when that page is drawn, so repainting one list does not read
  another page's tables out of storage.
- The Profiles list gains a filter and nothing else: there is deliberately no
  selection model, no multi-select and no batch verb. An earlier scope
  ("search, selection and bounded batch start/stop/proxy assignment, with
  per-profile results and cancellation") was dropped after reading the command
  path, not for lack of room to build it.
- The supervisor is serial, so a batch would only look parallel: commands
  arrive on a `crossbeam_channel::bounded(256)` (the app's `EVENT_CAPACITY`),
  `run()` handles them one at a time on one thread, and a start waits inline for
  Xray readiness (5 s) and CDP readiness (`cdp_ready_timeout`, 12 s by default) -
  N profiles cost up to N × 17 s no matter what the UI says.
- A batch command cannot report per-profile success: `RuntimeService::start`
  reads storage, sends and returns, the browser comes up later as an event, and
  the only honest per-profile answer is a snapshot read, which is what the row
  already shows.
- Cancelling already works: a `Stop` for a starting profile aborts it and
  `poll_start_commands` drops that profile's queued `Start`/`Restart`.
- The scale does not call for it: this is a personal tool with a handful of
  profiles, and a selection bar and a destructive batch verb are cost with no
  matching benefit.
- While a filter is on, the header reads "Showing N of M profiles." and its
  accessible name also names the term - that is how the reader finds out the
  list is not the whole list; with no filter it goes back to the sentence
  explaining what a profile is. An empty list says which kind of empty it is,
  and only the "all hidden" one offers to clear the filter, because only that
  one can be undone from the list.
- The filter field is built on the first render and kept, because an
  `InputState` needs a window and neither `AppView::new` nor `boot` has one. It
  is observed rather than subscribed to change events - the field notifies on
  every edit and reading its value is what the listing needs, so there is no
  event kind to match on - and `InputState::set_value` deliberately emits no
  change event, so the field's own clear button and the empty state's button
  both clear the filter themselves; a test pins that
  (`a_filter_that_matches_nothing_offers_the_way_back`).
- The two palettes are written as separate sets of values rather than derived
  from each other: contrast is relative to the background, so a green that reads
  on white is nearly invisible on near-black, and the light palette's status
  colors are pressed darker instead of reusing the same hue. In the light
  palette the `_strong` variants are darker rather than brighter - the opposite
  of the dark palette's sense of "strong" - because the name means the role (a
  status read on a tinted background, or a solid mark), not lightness.
- The `Palette` fields are semantic names rather than the colors they happen to
  be, so a light palette is a second set of values and not a second set of call
  sites (`danger` is the color a failure reads in, whichever mode is on).
- The Appearance card is first on the Settings page, leading the General group,
  and its control is a pair of chips rather than a toggle: a toggle shows one
  option and hides the other behind its label.
- The refactor that introduced `Palette` replaced the six module-level constants
  and the scattered hex literals of `crates/app/src/ui.rs` (before the file
  became `crates/app/src/ui/`) at 96 call sites, and the four dialog forms
  (`editor.rs`, `proxy_editor.rs`, `core_editor.rs`, `proxy_import.rs`) read the
  palette the same way as the view: `palette(cx)` where there is a context,
  `p: Palette` by value where there is not.

## Verification

The window's tests drive the real view headless through `TestAppContext`. The
harness in `crates/app/src/ui/tests.rs` injects one fake per port, each defined
in the `testing` module of the port it replaces (the verifier, the proxy
tester, the opener, the browser-data copier, the core and Xray catalogs and
downloaders, and the tray's starter): no display and no network are needed.

```sh
cargo test -p app                 # the whole app crate, the UI suite included
cargo test -p app ui::tests       # the window's own suite
```

Names worth naming, all in `crates/app/src/ui/tests/` and runnable as
`cargo test -p app <name>`:

- `the_filter_narrows_the_profile_list` - the filter narrows the list only.
- `the_sidebar_folds_to_a_rail_and_opens_again` - the sidebar's two shapes.
- `the_rail_travels_between_its_widths` - the one test that opens the window
  with motion on. It asserts the frame the press lands in has not moved, that 60
  ms of the test clock later the rail is on its way, and that it comes to rest
  at the layout's own width. Every step is the test clock's, which is the clock
  the design system's spring reads, so a slow machine cannot turn a moving rail
  into a settled one - and the check above, run with motion off, is what says the
  same fold is instant for a reader who asked for less motion.
- `a_wide_window_docks_the_details_panel_and_a_narrow_one_covers_the_list` -
  the docking breakpoint.
- `navigation_and_settings_choices_work_without_a_pointer`,
  `every_icon_only_control_has_a_name`, `ctrl_f_puts_the_keyboard_in_the_filter_from_any_page`
  and `closing_the_panel_gives_the_keyboard_back_to_the_row` - keyboard and
  accessible-name behavior of the shell.
- `the_rows_line_up_under_the_column_headings` and
  `the_log_rows_line_up_under_the_column_headings` - the table layout the pages
  share.
- `a_long_notice_still_leaves_its_dismiss_button_on_screen` - the banner's
  truncation.
- In `crates/app/src/theme.rs`: `both_palettes_have_readable_contrast`,
  `the_palette_follows_the_component_theme` and
  `both_layers_carry_the_same_accent` - the window's colors and the component
  theme moving together. `both_palettes_have_readable_contrast` holds WCAG 2.1
  ratios - 4.5:1 for body text and for supporting text (held to the body-text
  floor, replacing the old 3:1), 2:1 for `dim` - and checks every status color
  in both places it is read (on the window and on its paired tinted background
  inside a badge) and the accent in the three places this window uses it (on the
  window, on a selected row, as the label of a filled control); the light
  palette was caught by it on the first run, when three status colors read at
  3:1 or just under as text. Tests that assert on the component theme serialize
  through `theme::testing::exclusive`, because the component theme is
  process-wide state and two tests switching at once would each see the other's
  palette.

Opt-in suites (`#[ignore]`d; never part of the gate, per
[code standards](../code-standards.md)):

```sh
CHROMIUM_BIN=/absolute/path/to/chrome cargo test -p app real_chromium -- --ignored --nocapture
```

runs the `real_browser` acceptance: `AppState` to a real headless Chromium,
including `real_chromium_fingerprint_verification_through_the_verifier` and
`real_chromium_reports_the_address_its_own_traffic_leaves_from` (the latter
wants `ECHO_URL`). That module is `#[cfg(all(test, target_os = "linux"))]`, so
these tests exist only on Linux. `cargo test -p app -- --ignored` additionally
runs the tests that dial GitHub (`core_releases` and `xray_releases`) and, on
Linux with a StatusNotifierItem host, the tray test in `crates/app/src/tray.rs`.
