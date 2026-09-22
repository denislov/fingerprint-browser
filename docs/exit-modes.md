# Exit modes

**Status: implemented.** Closing the window offers three answers - keep running in
the background, leave the program but leave the browsers and tunnels running, or
stop everything - and the question can be answered for good. This page is the
reasoning; the code is `crates/app/src/exit.rs` for the choices,
`crates/app/src/tray.rs` for the background mode's one piece of desktop, and
`crates/runtime/src/journal.rs` for what a start does with what a previous run
left.

## The three answers

| Answer | The program | The browsers and tunnels | How it happens |
| --- | --- | --- | --- |
| **Keep running** | stays, with no window | stay, still managed | the window is minimized; the supervisor is untouched |
| **Leave browsers running** | ends | stay, unmanaged until the next start | the supervisor marks each session record and gives up its handles without stopping anything |
| **Stop everything** | ends | stop with it | what the program has always done: `RuntimeCommand::ShutdownAll` |

The fourth thing on the Settings page is not an answer: **Ask every time** is the
question, and it is the default.

## One rule decides the hard case

A start has always reclaimed what a previous run left running, and the reason is
in `journal.rs`: a browser that survived a crash still holds its profile directory
and its debugging port, and a new start must not race it.

A browser left **on purpose** holds exactly the same things. So the rule cannot be
"leave it alone" - the new start still has to do something about it - and it cannot
stay "stop it", because then "keep running" would mean "keep running until you open
the window again", which is not what the user asked for.

The answer is the session record's `left_running` marker:

- **Marked** - written when a run was told to leave its children running - and
  alive and identified as ours: the session is **adopted**. It becomes a running
  profile again, in the snapshots, with its ports and its processes, and an
  ordinary stop stops it.
- **Unmarked**: **reclaimed**, exactly as before. That covers a crash, and it
  covers every record written before this field existed, because the field
  defaults to `false`.

The marker is written at *exit* rather than at start, because at start there is no
answer yet to what closing the window will mean. A record that cannot be marked is
released anyway and logged: a missing mark makes the next start stop the session,
which is the safe direction, while refusing to release would strand processes the
user asked to keep.

## Adoption is a second kind of child

`RuntimeSupervisor` used to hold a `ManagedChild` per running profile, and a
handle is the authority on everything: whether it is alive, and how to stop it. An
adopted session has no handle - the handle died with the run that made it - so the
record is the authority instead: a pid, the kernel start time that tells one
process instance from the next, and the ports.

That is `Held::Owned` and `Held::Adopted`, and every question the supervisor asks
about a child goes through it:

- **Is it alive?** The handle for an owned child, `journal::verdict` for an adopted
  one. "Cannot be asked" is deliberately not "gone": an unreadable process must
  not turn into a crash report and a stopped profile.
- **Stop it.** The job object or the process group for an owned child; for an
  adopted one, the recorded identity is rechecked at the moment of termination and
  the tree is stopped by pid - which on Windows is `taskkill /F /T`, already the
  path an orphaned browser took.
- **Leave it.** Nothing on Unix: a child is not signalled when its handle is
  dropped. On Windows the job's `KILL_ON_JOB_CLOSE` limit is cleared first, because
  that limit is exactly what makes a manager that died take its browsers with it.

One thing is lost and is not pretended otherwise: an adopted process has no exit
status to read, so a browser that disappears between starts is reported as a
normal stop rather than guessed at as a crash. The alternative is a false alarm
every time someone closes a browser window while the manager is not looking.

## What the window does

- The close button and the Quit button ask the same question through the same
  code, because they are the same request.
- The question is asked **before** the window closes, which is what
  `Window::on_window_should_close` is for - `on_window_closed` is told after the
  fact, when the chance to ask has gone. The hook is installed on the first frame,
  because the view is built before there is a window.
- The three answers are rows in the dialog rather than footer buttons: "leave
  browsers running" and "stop everything" are one word apart and opposite in
  effect, and the sentences are what tell them apart. Each row is given a height
  rather than measured: left to the dialog's content box, the last of three
  identical rows came out a text line shorter than the two above it and painted
  its own note over its bottom border. Reordering the rows, changing the text and
  pinning the line height each changed nothing - so the box is what is pinned, and
  `the_exit_dialog_shows_three_equal_answers_inside_itself` is the assertion that
  it stays that way.
- The "remember" box is the same setting the Settings page writes, and it is
  written before the exit is carried out - like every other stored choice, a
  config file that cannot be written refuses the change. The exit still happens:
  the answer was given.
- A **signal** (`SIGINT`, `SIGTERM`, `SIGHUP`) still means stop everything. There
  is nobody to ask, and a signal is an explicit request to stop.

## The tray, and what this UI stack will not do

One fact about the framework decides the shape of "keep running", and it was read
out of the vendored sources rather than assumed: **GPUI has neither a tray nor a
hide.** `App::hide()` is a no-op on every backend, and `PlatformWindow` has
`minimize`, `activate` and the `map_window` window creation uses - and no `unmap`
at all.

What it does have is the way around both. `Window` implements `raw-window-handle`,
so `crates/app/src/window_visibility.rs` can take the native window and put it
away itself: `ShowWindowAsync(SW_HIDE)` on Windows, and `UnmapWindow` on the
window's own XCB connection on X11. An unmapped X11 window is *withdrawn* - the
window manager stops managing it, so it leaves the taskbar, the window list and
Alt+Tab - which is what closing the window was supposed to mean.
`minimize_window()` is what is left on Wayland, where an `xdg_toplevel` has no
withdrawn state and a surface cannot be unmapped without destroying it.

The tray icon is what says the program is still there. It is two implementations,
because the platforms have nothing in common here:

| Platform | Crate | Why |
| --- | --- | --- |
| Linux | `ksni` | StatusNotifierItem over D-Bus, pure Rust through `zbus` - no GTK, no libdbus, so no new system library to build or package |
| Windows | `tray-icon` | the native shell notification area; it is target-gated, so its Linux half (GTK, libappindicator) is never built |

The two are reached differently, and the difference is worth knowing before
changing either:

- **Linux**: `ksni`'s blocking API spawns the service on a thread of its own and
  hands back a handle. The menu callbacks run on that thread, so they do nothing
  but post into a channel.
- **Windows**: the icon is created on the window's own thread and lives in the
  view. Its messages are dispatched by the message loop the window already runs,
  so there is no second pump to write - and this is why it is safe for the icon to
  be `!Send`: the design never asks it to cross a thread.

Either way the window's tick is what acts, which is the same thing the runtime
events already do.

The tray is created on the first "keep running" and never removed, because the
user who put the window away once may do it again. Its two items are **Show
window** - which maps the window again and calls `activate_window`, the same call
the taskbar makes - and **Quit completely**, which stops everything and is
deliberately neither a mode nor the remembered answer: a tray whose only way out
re-entered "keep running" would be a program nobody could leave, and one that left
the browsers behind would be a menu item that says "quit" and does not. The icon
itself is drawn by `scripts/make-icon.py`, which also writes the PNG, the ICO and
the SVG, and `assets/tray.rgba` is that same drawing at 64×64: `tray-icon` wants
RGBA and `ksni` wants ARGB32, so the byte order is swapped in four lines rather
than committed twice.

## What this does not do

- It does not destroy the window. "In the background" is the window hidden, not
  the window gone: the view, the tick and the tray are the same objects before and
  after, which is what makes showing it again immediate and what keeps the
  snapshot cache warm.
- It does not hide the window on **Wayland**. An `xdg_toplevel` cannot be withdrawn
  without destroying its surface, so there the window is minimized and the
  compositor keeps it in its own list; the tray still says the program is there,
  and the compositor's window list is the way back.
- It does not keep running with *no* window at all. The tick ends the program if
  the window list ever becomes empty, which is the safety net that has always been
  there: a program managing browsers with nothing on screen and nothing in the tray
  would be a program with no way to reach it. A desktop that refuses the tray icon
  is warned about in the log rather than refused - the window still goes away.
- It does not adopt what a crash left. That is still reclaimed, on purpose, and it
  is the behaviour every existing record gets.
- It does not make two data directories one installation. An adopted session
  belongs to the data directory whose runtime directory recorded it.
