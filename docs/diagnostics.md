# Diagnostics

**Status: implemented.** One command writes one file about this installation:
`fingerprint-browser --diagnostics`. The Settings page has the same report
behind a button. It exists because the first three messages of a bug report are
always the same - which build, which platform, where the files are - and none of
them should have to be asked for.

## What the report holds

| Section | What is in it |
| --- | --- |
| Build | The version, the commit the binary was built from, `os arch (profile)` and the time the report was written |
| Settings in force | Every row of the Settings page: the value in force, where it came from (environment, config file, default), and any stored value the environment is overriding |
| Files | For each of the data directory, the config file, the database, the instance lock, both activity logs, the runtime directory, the browser-data directory and the Xray executable: whether it is there, and - where it is - its size or entry count and its permission bits |
| The end of the activity log | The last 40 lines of `logs/activity.log`, oldest first, with how many lines the file holds |

## What it deliberately leaves out

- **It never opens the database.** Proxy credentials live in it
  (`proxies.outbound_json`). The report is meant to be pasted somewhere public,
  so the database is measured - size and mode - and not read.
- **It never copies browser data.** The directory is counted, not listed: how
  many profiles are on disk is a fact about the installation, and what is inside
  them is the user's browsing.
- **It does not resolve the environment for you.** The values are in the
  settings section with the variable that set them, rather than as a second
  table of variables that could disagree with the first.
- **It is not a log of the run.** The activity log is the app's own account and
  is included; `tracing` output goes to stderr and is not.

The one thing worth knowing before sharing a report is the log tail: it names
profiles and, after a proxy test, the address traffic left from. `diag_intro`
says the report holds the end of the log, so the file says what it contains
before anyone has to guess.

## Where it goes

`<data dir>/diagnostics/fp-browser-diagnostics-<stamp>.md`, or the path given to
`--out`. The stamp is the same rule as the configuration export
(`paths::default_diagnostics_file`): a name that carries the second cannot land
on an earlier report by accident, so the default destination is always safe to
write over, and a report that replaces one is one the user pointed at.

The Settings page's **Diagnostics** card is the same report behind one button,
and it has no path field: it writes the same default, the line under the button
names the exact file, and the window says where it went. A path to type there
would be a second way to say something the program already knows - `--out` is
that second way, and it is for scripts.

The report is markdown, and reads as plain text: headings and `- label: value`
lines, so it renders in an issue and greps in a terminal.

## The command line

```
fingerprint-browser [option]

  -h, --help         Print this help and exit
  -V, --version      Print the version, the commit and the platform, and exit
      --diagnostics  Write a report about this installation and exit
      --out <path>   Where --diagnostics writes; the data directory by default
```

With no option the window opens. Exit codes: `0` for an answer, `1` for a report
that could not be written, `2` for an argument that was refused, `3` for a second
copy finding the data directory already locked (`--diagnostics` is answered
before the lock, so it works either way).

Two rules are worth stating because they are the difference between a tool you
can put in a script and one you cannot:

- **`--version` reads nothing.** It prints `Fingerprint Browser 0.1.0 (a1b2c3d)`
  and ends. No config file, no migration, no data directory, no window: a script
  asking what is installed cannot change the answer by asking.
- **`--help` writes nothing.** It is answered in the language the config file
  names, read through `settings::language_hint`, which parses the `lang` field
  and nothing else. A full `Settings::load` would fold an older installation's
  config file into the data directory - the documented first-start-after-upgrade
  behaviour - as a side effect of asking how the program is used.

The version line also goes in the window's title bar, and the same line plus the
platform and the data directory goes in as the first line of the activity log,
so the build a session ran under is recorded even when nobody thought to look.

## Why the words are translated and the version is not

`--help` and every message the report prints are in the catalog
(`crates/app/src/text.rs`), like the rest of the window's text: the person
reading them is the same person who chose the language. What is *not* translated
is the content the report quotes - paths, versions, a commit hash, the lines of
the log, the operating system's own error text. Those are the same in every
language, and a translated path would not resolve.

The commit is injected at build time by `crates/app/build.rs`
(`git describe --always --dirty --abbrev=9`), not written in the source: a
checkout reports its own commit, a modified tree says so with a `-dirty` suffix,
and a source tree with no `.git` at all still builds - it reports `unknown`.

## Tests

- `cli::tests` - every argument spelling, the refusals, and that a refusal reads
  in the language in force.
- `diagnostics::tests` - that the build, every setting row and every path are in
  the report; that a file that is not there says `missing` and then reports its
  size; that only the tail of the log is carried; that a database full of
  credentials is measured and never quoted; that the report is written where it
  was asked and that a write failure names the path; and, on unix, that the
  permission bits are in it.
- `paths::tests` - the default file name, its folder, and that it holds no
  character a Windows path cannot.
- `version::tests` - that the line names the crate version, that a commit is
  always recorded, and that the title bar does not carry the commit.
