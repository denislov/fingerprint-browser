# First run

**Status: implemented, and pinned by a test.** This is the path from a window
that has just been installed to a browser running under a profile, with what the
window says at each step. It is written down because a first run is the one part
of a program nobody who built it ever sees: every check here has been run
hundreds of times on a machine that already had a core, a database and a config
file.

The test that walks it end to end is
`ui::tests::a_fresh_installation_can_be_walked_from_no_core_to_a_running_profile`
in `crates/app/src/ui.rs`. Each step is also covered on its own; what the walk
pins is the handover between them.

## What the program needs, and what it never does

Two external executables, and the program supplies neither:

| Needed for | What it is | Where it comes from |
| --- | --- | --- |
| Every profile | A fingerprint-chromium (or Chromium) binary - a *core* | The user, on the Browser Cores page, or discovered at startup |
| A profile with a proxy | An Xray binary | The user, on the Settings page, or the default `bin/xray` |

It never downloads either one, at first run or at any other time, and there is no
"install a browser for me" button. That is a rule rather than an omission: this
program exists to control exactly which binary runs and with which switches, and
a binary it fetched itself is one the user did not choose. See
[the plan](implementation-plan.md#why-distribution-is-a-task-and-not-a-paragraph).

## The walk

**1. The window opens on an empty profile list.** Nothing is installed yet, and
the list says so where the profiles would be:

> No browser core yet. Add a fingerprint-chromium binary on the Browser Cores
> page, or point FP_BROWSER_CHROMIUM_BIN at one and restart.

Both routes are named because both work, and the sentence carries the button that
takes the first one. `New Profile` is disabled: a profile with no core cannot
launch, and a form that refuses on save is a worse way to learn that.

**2. Add a browser core.** Either:

- press **Add a browser core** in the empty state, then **Add Core** on the
  Browser Cores page, or
- start the program with `FP_BROWSER_CHROMIUM_BIN` pointing at the binary, which
  is registered at startup and appears in the same list.

The form asks for the executable and reads its version out of it. A binary that
answers nothing to `--version` is refused rather than listed with a guess, and
the refusal names `FP_BROWSER_CHROMIUM_MAJOR` as the way to record it anyway. The
page also lists every core's detected major, because a binary's version decides
which switches a profile may claim.

**3. Create a profile.** **New Profile** opens the form: a name, the core it
launches on, and a fingerprint seed. The seed is generated for a new profile and
is what every noisy surface is derived from - it is the thing to copy when a
second profile has to look like the first one.

**4. Supply Xray, if the profile uses a proxy.** A profile with no proxy goes
direct and needs nothing else. For a proxied one the program starts Xray itself,
so the binary has to be there: set the **Xray executable** row on the Settings
page, or `FP_BROWSER_XRAY_BIN`, or leave the default `bin/xray` next to the
program. It is read at the start of a run, so a change takes effect at the next
start and the row says so. If it is missing, the launch fails closed - the
browser is not started to go direct instead, which is the failure this program
exists to prevent.

**5. Start it.** **Start** on the row. The row goes `Starting` -> `Running`, and
the Runtime Details panel has the pid, the CDP port, the SOCKS port when there is
one, and the exact arguments the browser was given (*Args* tab).

**6. Verify the fingerprint.** **Verify fingerprint** asks the running browser
what it reports and compares that against what the profile claims, one claim at a
time. The result is either the confirmation that every claim was read back, or a
list of the ones that disagree - with both values, so it is clear which side is
wrong. This step is the reason for all the others: a profile that launches is not
evidence that it presents the fingerprint it says it does.

## Where everything goes

One directory is the whole installation: the config file, the database, the
logs, the runtime files and each profile's browser data. It is
`%LOCALAPPDATA%\FpBrowser` on Windows, `~/.local/share/FpBrowser` on Linux
(or `$XDG_DATA_HOME/FpBrowser`), and `~/Library/Application Support/FpBrowser` on
macOS - or whatever `FP_BROWSER_DATA_DIR` says. The Settings page shows the
value in force and where it came from.

A first run creates that directory and the database in it. Nothing is written
anywhere else, so uninstalling is deleting one directory - and moving machines is
copying it; see [backup and restore](backup-and-restore.md).

## When something is wrong

- The **Log** page is what the window has done and seen, and `logs/activity.log`
  keeps it after the window closes. The first line of every run names the build,
  the platform and the data directory.
- **`--diagnostics`** writes one file about the installation: the build, every
  effective setting and the source it came from, the state and permission bits of
  every file, and the end of the log. See [diagnostics](diagnostics.md). It is
  the file to attach to a report, and the first thing to read when a launch does
  something the window does not explain.
- **`--version`** prints the version, the commit and the platform, which is the
  other half of any report.

## What is still missing here

The path is tested against the fake runtime, not against a real browser: a
first run on a machine that has one is [acceptance](chromium-acceptance.md), and
that remains opt-in. Packaging - an installer that puts the binary somewhere and
a shortcut on the desktop - is the next step in
[the plan](implementation-plan.md#next-bounded-tasks); today a first run means
running the binary from a checkout or an unpacked archive.
