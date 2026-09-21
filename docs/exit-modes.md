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

Two facts about the framework decide the shape of "keep running", and both were
read out of the vendored sources rather than assumed:

- `App::hide()` is a no-op on **both** backends - `fn hide(&self) {}` on Windows,
  and "hide is not implemented on Linux, ignoring the call" on Linux. The Windows
  backend only ever calls `SW_MINIMIZE`, `SW_MAXIMIZE` and `SW_RESTORE`, and the
  application is not given the native window handle, so there is no way to unmap
  the window from here.
- GPUI has no tray support at all.

So "in the background" means the window is **minimized**, not hidden, and the tray
icon is what says the program is still there. `Window::minimize_window()` and
`Window::activate_window()` are both implemented on X11 and Windows, so the tray's
"show window" has something to call on both.

The tray itself is two implementations, because the platforms have nothing in
common here:

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
window** - which calls `activate_window`, the same call the taskbar makes - and
**Quit…**, which is deliberately *the question* rather than the remembered answer:
a tray whose only way out re-entered "keep running" would be a program nobody
could leave. The icon itself is drawn by `scripts/make-icon.py`, which also writes
the PNG, the ICO and the SVG, and `assets/tray.rgba` is that same drawing at
64×64: `tray-icon` wants RGBA and `ksni` wants ARGB32, so the byte order is swapped
in four lines rather than committed twice.

## What this does not do

- It does not hide the window. See above: the framework cannot.
- It does not keep running with *no* window at all. The tick ends the program if
  the window list ever becomes empty, which is the safety net that has always been
  there: a program managing browsers with nothing on screen and nothing in the tray
  would be a program with no way to reach it. "In the background" is therefore
  minimize-plus-tray, and a desktop that refuses the tray icon is warned about in
  the log rather than refused - the window still minimizes.
- It does not adopt what a crash left. That is still reclaimed, on purpose, and it
  is the behaviour every existing record gets.
- It does not make two data directories one installation. An adopted session
  belongs to the data directory whose runtime directory recorded it.
