# Backup and restore

**Status: a configuration backup can be written and read back - export and
import are both in, from the Settings page. Restore and the browser-data copy
are designed, not implemented.** This page defines the rules before the rest of
the code exists, because three of them - what counts as a credential, what
happens to an identifier that is already taken, and what happens to a path that
does not exist on this machine - decide the shape of the code rather than the
other way round.

What is in the program today:

- `ProxyOutbound::without_credentials` and `carries_credentials` in
  `crates/domain/src/proxy.rs` - the one place the credential list lives.
- `crates/application/src/config_backup.rs` - the document, its credential
  choice, and the reader that refuses a file it cannot be sure it understands.
- `crates/application/src/export.rs` - reading the three lists and writing the
  file, with a report of what was written.
- `crates/application/src/import.rs` - the planner that decides what a document
  would do to an installation, and the applier that writes it in dependency
  order and reports what landed.
- The Settings page's **Export configuration** card - a path field, the
  credential choice, and a sentence saying what happened.
- The Settings page's **Import configuration** card - a path field and a
  sentence that reports arrivals, keeps, skips and re-pointed directories in
  one line.

See [The first slice](#the-first-slice) for what is left. Until then, see
[What you can already do](#what-you-can-already-do).

## What is on disk

Measured from the code, not assumed:

| Location | Contents | Relevant to a backup |
| --- | --- | --- |
| `<data dir>/app.db` | SQLite: `cores`, `proxies`, `profiles`, `schema_migrations` | The configuration. `proxies.outbound_json` is `serde_json` of the outbound, so credentials sit in it as plain text. |
| `<data dir>/profiles/<profile uuid>/` | One Chromium user-data directory per profile: cookies, Local Storage, IndexedDB, caches | The browser data. Large, session-bearing, and locked while the profile runs. |
| `<data dir>/logs/activity.log` (+ `.log.1`) | The activity log, rotating at 512 KiB | Not part of any backup. |
| `<data dir>/runtime/<profile id>/config.json` | The Xray config written at start, holding upstream credentials | **Never exported.** See below. |
| `<config dir>/fp-browser/config.json` | `data_dir`, `xray_executable`, `echo_url` | Machine-local, and deliberately outside the data directory. |

Two consequences follow from the layout rather than from taste:

- The config file lives **outside** the data directory precisely so it can say
  where the data directory is. A backup of the data directory therefore never
  carries the pointer to itself, and restoring one does not require rewriting a
  setting. What it also means is that "settings" are not part of a backup at all.
- `profiles.user_data_dir` is stored as an **absolute path**, and profile, core
  and proxy identifiers are UUIDv4. A backup restored on the same machine can line
  browser data back up; a backup carried to another machine cannot, because the
  path names a directory that was never there.

## Two artifacts, not one

| | Configuration backup | Browser-data backup |
| --- | --- | --- |
| Size | Kilobytes | Hundreds of megabytes per profile |
| Content | Cores, proxies, profiles | Cookies, origins, sessions |
| Contains credentials | May, by choice - see [Credentials](#credentials) | Yes, of the sites visited |
| Portable between machines | Yes, with the path rules below | Yes, but only via the profile it belongs to |
| Reason to take one | Move to a new machine, recover from a mistake | Recover a logged-in session, move one profile |

They are separate because a single "back up everything" would make the common
case - move my configuration to a new machine - cost gigabytes and leak a second
copy of every session cookie to do it.

## The configuration file

### Shape

A versioned JSON document. The domain types are already `Serialize` and
`Deserialize`, so the file carries the domain shape rather than a parallel one
invented for export; `ProxyOutbound` is internally tagged, so a protocol appears
as its own name:

```json
{
  "format": "fp-browser/config-backup",
  "version": 1,
  "credentials": "included",
  "exported_at": "2026-09-20T16:04:00Z",
  "source_data_dir": "/home/alice/.local/share/FpBrowser",
  "cores": [
    {
      "id": "0f9c1d20-...",
      "name": "Fingerprint Chromium 148",
      "executable": "/opt/chromium-148/chrome",
      "version": "148.0.7778.215",
      "major": 148
    }
  ],
  "proxies": [
    {
      "id": "7b21a4c8-...",
      "name": "Zurich exit",
      "outbound": {
        "type": "socks5",
        "host": "203.0.113.10",
        "port": 1080,
        "username": "alice",
        "password": "correct horse battery staple"
      }
    }
  ],
  "profiles": [
    {
      "id": "c4d8e119-...",
      "name": "Work laptop",
      "core_id": "0f9c1d20-...",
      "user_data_dir": "/home/alice/.local/share/FpBrowser/profiles/c4d8e119-...",
      "fingerprint": { "seed": 4242, "brand": "chrome", "platform": "windows" },
      "proxy_id": "7b21a4c8-...",
      "window": {},
      "start_target": {}
    }
  ]
}
```

- `format` and `version` come first so a reader can refuse before parsing
  anything else. `version` is the version of **this document**, not of the
  program: it changes when the shape changes, which is what makes an old file
  readable by a newer build instead of by trusting that the domain types happen
  not to have moved.
- An unknown `format` is refused, not guessed at - the same rule
  `settings::Stored` already applies with `deny_unknown_fields`.
- `credentials` records what was actually written, so an import does not have to
  inspect every proxy to find out, and a reader can see it before reading.
- `exported_at` and `source_data_dir` are informational. Nothing decides anything
  from them, but the second one is what lets the import report "these paths came
  from `/home/alice/...`" instead of only "re-pointed".
- `user_data_dir` is written for every profile even though the default is
  derivable from the identifier. A conditionally written field is a branch to get
  wrong; the import rule below is where the machine-specific part is handled.

### Credentials

Credentials are **included or excluded per export**, chosen when the export runs.
Rules that apply either way:

- **What counting as a credential means.** Stripping removes the fields that
  authorise this client to the upstream, and keeps the host, the port and the
  transport/TLS settings, because those are not credentials and re-typing them is
  the tedious part. The exact list is per protocol - `username`/`password` for
  SOCKS5 and HTTP, `password` for Shadowsocks and Trojan, `uuid` for VMess and
  VLESS, plus REALITY's `public_key` and `short_id` - and it lives in **one
  exhaustive `match`** (`ProxyOutbound::without_credentials`, next to the outbound
  types). A new protocol then cannot be added without deciding what its
  credentials are, because the `match` stops compiling, and there is one test per
  protocol rather than one test for the protocols someone remembered.
  REALITY's `public_key` is the server's public key rather than this client's
  secret, and stripping it is still the safer default: it can be re-entered, and
  a stripped file that over-removes is recoverable where one that under-removes
  is not. `server_name`, `fingerprint` and `spider_x` stay - a borrowed SNI, a
  client-hello template name and a fallback path are not things a server checks a
  secret against.
- **The export says what it did.** A file that carries plain-text passwords is
  not a file to hand to someone casually, so the choice is stated at the moment
  of export and recorded in the file, not only in this page.
- **A stripped proxy is not silently a working proxy.** Which protocols actually
  *refuse* one is narrower than the sentence suggests, and worth having exactly:
  Shadowsocks, VMess, VLESS and Trojan require credentials, so stripping them
  produces a proxy that `validate_proxy` already refuses - nothing new was needed
  for that. SOCKS5 and HTTP authenticate *optionally*, so stripping one produces a
  different proxy rather than a broken one, and it still validates. An import
  cannot tell those apart from the outbound alone, which is why the file records
  its own choice rather than leaving it to be inferred. A test pins the boundary:
  `a_stripped_proxy_is_refused_only_where_credentials_are_mandatory`.
- **REALITY material is load-bearing on its own.** A stream that says
  `security: reality` without a public key is already refused by `validate_stream`
  for the missing key, so losing it does not leave a quietly degraded proxy behind.
- **The runtime directory is never part of any backup.** `<data dir>/runtime/`
  holds the same credentials in the Xray config written at start, and that file is
  transient by design: `clear_start_files` and journal reclamation remove it, and
  a probe leaves nothing behind. Copying it into a backup would put a second copy
  of the credentials somewhere the program never cleans, and would resurrect
  stale ports and paths along with it. A backup takes configuration and browser
  data; it does not take scratch files.
- **The activity log is never part of any backup.** It is a record of what
  happened, not configuration, and it names proxies and exit addresses.

Whether an export should be encrypted is **not decided, and the current
recommendation is no**: encryption without a key-management story is a false
comfort. The file is plain text, and the export says so.

### Identity: restore and import

Two verbs, and they must not share one rule:

- **Restore** - "make this installation be that file". Its precondition is a
  database with no cores, proxies or profiles in it, or an explicit confirmation
  to replace what is there. It refuses to start otherwise, because an operation
  whose whole meaning is "be this" must not quietly end up with a mixture.
- **Import** - "add these to what I have". It never overwrites.

They share one rule: **an identifier is kept when it is free, and a taken
identifier is never overwritten.** Keeping identifiers is what makes a
configuration backup and a browser-data backup line up again - the directory is
named after the profile's identifier, and the default path is derived from it.

Why the default is not "overwrite": the moment someone reaches for import is
usually the moment they have lost something, and replacing live rows with older
ones is the worst possible outcome of that action. Restore, which *is* meant to
replace, says so out loud instead.

Further rules:

- **Import is not one transaction, and that is a correction to the first draft
  of this page.** The plan said "one transaction" because `profiles.core_id` is
  a foreign key and a partial import must not leave rows that point at nothing.
  But the storage traits have no transaction seam - transactions exist only
  inside `migrations.rs` - and adding one for this would be a schema-shaped
  answer to an ordering problem. The ordering is the answer instead: cores are
  written first, then proxies, then profiles, and after each of the first two
  phases the identifiers are **read back out of the database**, so the profile
  phase decides against what is stored rather than against what the plan meant
  to store. A core the database refused therefore takes its profiles with it as
  *skipped* - the same answer the rules already give when the file itself is
  the reason, said about a write that did not happen. An item the database
  refuses does not stop the rest, because a transaction would let one unreadable
  proxy take an otherwise good import down with it; what the transaction was
  protecting against is what the ordering and the read-back protect.
- Import never overwrites, which is what makes it idempotent: the same file
  twice adds nothing the second time and says so. `profiles.insert` is a plain
  `INSERT` with no upsert, and that is relied on rather than worked around.
- **A profile is imported only when the core it names is present** - from this
  file or already in the database. A profile whose core is missing is skipped and
  reported. Re-pointing it at another core would change the engine it runs on,
  which changes the fingerprint it claims, which is not a decision an import is
  allowed to make quietly.
- **A profile whose proxy is missing is imported with no proxy**, and reported.
  With `proxies.id` a foreign key and `ON DELETE SET NULL`, the database cannot
  hold a dangling proxy reference, so an export cannot produce one either; this
  rule exists because the file is a document a person may edit, and a file is
  untrusted input. Reported per item, not fatal to the whole import - the same
  treatment an unreadable journal record already gets.
- A well-formed file makes those two cases impossible. They are still handled,
  because "the format forbids it" is not the same as "it cannot arrive".

### Paths

`user_data_dir` and a core's `executable` are absolute paths from the machine the
backup was taken on.

- **`user_data_dir` is kept when the directory exists, and re-pointed at the
  local default for that identifier when it does not** - `<data dir>/profiles/<id>`.
  The report says which ones were moved. A path that does not exist on this
  machine was never going to be right, and the alternative is a profile that
  silently starts with no sessions while claiming a directory it does not have.
- **A core's `executable` is kept as written, and the core is left in the state
  the program already has for a binary it cannot read.** No new flag is needed:
  a core whose version cannot be read is already refused rather than asserted, and
  is already shown as one that cannot be started. Importing a core whose binary is
  absent on this machine therefore lands in a state the UI and the launch path
  already handle and already explain, and re-pointing it is one field on the
  Cores page.

### Machine-local settings are not in the file

`data_dir`, `xray_executable` and `echo_url` are not exported. They describe this
machine, and one of them is the setting that says where the data directory is -
which is the file's own context, not its content.

### Where an export writes

- **The default is `<data dir>/exports/fp-browser-config-<stamp>.json`**, with the
  stamp a UTC reading spelled `YYYYMMDD-HHMMSS`, as in
  `fp-browser-config-20260920-160400.json`.
- **The stamp is not the activity log's spelling.** A clock time carries colons
  and a Windows path cannot hold one, so a name built from that formatter would
  produce a backup that cannot be written on one of the platforms this program
  runs on. Both read the same calendar arithmetic; only the punctuation differs.
- **A new name each second is what makes the default safe to write over.** An
  export replaces whatever is at its path, with no confirmation and no
  already-exists refusal. That is the right rule for a backup someone keeps at one
  path and refreshes - and it is only safe because the *default* can never land on
  an older backup, so reaching an existing path means someone typed it.
- **The field starts empty rather than filled with the default.** An empty field
  resolves to the current second when it is used, so a window left open overnight
  does not propose yesterday's name, and the line under the field names the file
  it would write right now.
- **The path is typed rather than picked from a native file dialog.** A chooser
  would be another dependency, and opening one is the part of a desktop
  integration most likely to behave differently over RDP - which is a supported
  way to run this. The cost is honest and visible: the user types a path, and the
  export says which file it wrote.

## Browser data

- A browser-data backup is a copy of one or more `profiles/<id>` directories.
- **Only for stopped profiles.** A running Chromium holds its singleton lock and
  writes while the copy runs, so the copy would be of an unspecified moment.
  Export refuses and names the profiles that are running; restore does the same.
- **The directory is copied whole.** Excluding caches would mean tracking
  Chromium's internal layout across versions, which changes; the cost of copying
  whole is size, and the cost of a filter is a backup that is silently incomplete
  after a Chromium update. Size is the cheaper of the two, and it is visible
  before the copy starts.
- Restore places the directory at the path the restored profile has. Because
  identifiers are kept, that is the same relative place, so a same-machine
  restore lines up and a cross-machine restore still lands somewhere correct.

## What is not in a backup

- `<data dir>/runtime/` - transient, and holds live credentials.
- `<data dir>/logs/` - a record, not configuration.
- `<config dir>/fp-browser/config.json` - machine-local by design.
- `data_dir`, `xray_executable`, `echo_url` - settings, as above.
- `schema_migrations` - a property of the database, not of the user's
  configuration. The document's own `version` is what a reader checks.

## What you can already do

With the program closed, copying the data directory is a complete backup of the
configuration and of every profile's browser data:

```sh
# While the program is not running.
cp -a "$HOME/.local/share/FpBrowser" "$HOME/FpBrowser-backup-$(date +%F)"
```

This is lossless and needs no code. What it does not do, and what the design above
adds, is: move a configuration to a machine with a different home directory;
import without risking what is already there; leave credentials out; and restore
one profile rather than all of them. It also carries the plain-text credentials in
`app.db`, which is worth knowing before that directory goes anywhere else.

## Open questions

- **Encryption.** Recommendation: none. Revisit only with a key-management
  answer, because an encrypted file whose key sits beside it is plain text with
  extra steps.
- **Whether an export should record the program version** that wrote it. The
  document `version` covers the shape; a program version would only help
  diagnostics.
- ~~**How a partial import is presented.**~~ **Answered.** The report is one
  sentence with a clause per kind of shortfall - what arrived, what was kept
  because the identifier was taken, what differs and was therefore not
  overwritten, what was skipped for a missing core, what arrived without its
  proxy, and whose directory moved. A clean import is a toast; an import that
  did less than the file asked for is put in the banner, which stays until it
  is dismissed. The banner has to mean something, and a shortfall is the thing
  the reader came to find out.

## The first slice

Small enough to land and check on its own, and each step is useful without the
next:

1. [x] `domain`: the credential-stripping function, one exhaustive `match`, one
   test per protocol.
2. [x] `application`: serialise cores, proxies and profiles into the document,
   with the credential choice as a parameter, plus the inverse parse that refuses
   an unknown `format` and an unknown `version`.
3. [x] Export to a chosen path, and the statement that the file carries
   plain-text credentials when it does.
4. [x] Import, with the identifier, path and core rules above, reporting per item.
5. [ ] Restore, which is import plus the precondition and the confirmation.
6. [ ] Browser-data copy for stopped profiles.

Steps 1 to 4 are in. Things about them worth knowing before step 5:

- The document is built from a `ConfigSnapshot` and an `ExportOrigin`, both plain
  values, so `application` has no clock and no filesystem of its own. The caller
  supplies the timestamp and the data directory. That is what makes "an excluded
  export leaves the secret out of the file" a test that reads a string rather than
  one that needs a disk.
- `proxies_carrying_credentials` exists on both `ConfigSnapshot` and
  `ConfigBackup`, and on a built document it is always zero when credentials were
  excluded. That is not redundancy: the count a user needs to be told is the one
  taken *before* stripping, and an export that reports "no credentials were
  written" when there were some is the failure the pair exists to prevent. Three
  sentences come out of it - included, left out, and nothing to leave out - and
  only the second tells the reader the file is not the whole configuration.
- Export runs on the calling thread, unlike a proxy test or a fingerprint reading.
  Reading three lists and writing one small file are local and quick, which is the
  same reason starting a profile reads storage inline. Nothing here waits on a far
  end, so there is no worker and no channel to reconcile.
- Import is split the same way export is: `plan_import` is a pure function of the
  document, the three lists and the data directory (it asks the filesystem one
  question, whether a recorded directory exists), so the decision table is read
  and tested as a table; `apply_import` then writes in dependency order and counts
  what landed from the writes rather than from the plan. `read_config_backup`
  keeps the two file-level failures apart - a path with nothing at it is a path
  to correct, and a file that is not one of ours is a different file to pick.
- A profile whose directory had to move is **reported but not alarmed about**.
  Importing onto another machine moves every directory there is, so the move is
  the expected outcome of the feature, and an alarm for it would train the reader
  to dismiss the alarm. `needs_attention` is therefore exactly "the import did
  less than the file asked for": a kept item that differs, a skipped profile, or
  a profile that arrived without its proxy - and a partial import lands in the
  banner for that reason, while a clean one is a toast.
- Import never re-probes and never re-derives. `create` mints an identifier and
  `update` re-reads a core's version, so the services gained insert-as-given
  methods for exactly this caller, and a record arrives as the file says it was.

The gate is unchanged and applies to all of it - see
[implementation-plan.md](implementation-plan.md#required-gate).
