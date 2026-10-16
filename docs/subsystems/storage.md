# Storage

Storage is the program's local SQLite database and the repository backends the rest of the program reads and writes through. It owns the schema and its migrations, the four repository contracts, the whole-configuration replacement a restore runs on, and the atomic file write used for files that must never be left half-written. It does not own what a record means ([Domain model](../domain-model.md)), the rules that decide what may be written ([Service contracts](../service-contracts.md)), the file formats around the database ([Backup and restore](../backup-and-restore.md)), or runtime state: a running session is not in the database ([Runtime lifecycle](../lifecycle.md)).

## Responsibilities and boundaries

- **The schema and its migrations.** The tables `cores`, `proxies`, `profiles` and `schema_migrations` are created and upgraded by `run_migrations` over the ordered `MIGRATIONS` list in `crates/storage/src/migrations.rs`. Opening a database an older build wrote upgrades it; a database a newer build wrote is refused rather than opened.
- **The repository contracts.** Four traits in `crates/storage/src/traits.rs` are the interface the rest of the program has to the database: `ProfileRepository`, `ProxyRepository`, `CoreRepository` and `ConfigurationRepository`. The error type they return is `StorageError` in `crates/storage/src/error.rs`.
- **Two backends.** `SqliteStorage` is the one the program runs on, opened in `crates/app/src/main.rs`. The in-memory backend (`MemConfiguration` and the three `Mem*Repository` types in `crates/storage/src/memory.rs`) exists so tests can run the same flows without a file; one shared body of assertions is run against both backends, which is what makes the parity a checked property rather than a promise.
- **The whole-configuration replace.** `ConfigurationRepository::replace` removes what is stored and writes the given cores, proxies and profiles as one operation: on success every record is one of the arguments, on failure nothing has changed.
- **Atomic file writes.** `atomic_file::write` replaces a file only once its whole replacement has been written and synced. It is what the settings document and a configuration backup are written through.

What storage does not own:

- **What a record means.** Names, paths, fingerprints, proxy outbounds and identifiers carry their semantics in `domain` ([Domain model](../domain-model.md)); storage writes them down and reads them back.
- **What may be written.** Record validation - a name that is not blank, a window larger than zero, a start page that is a page - belongs to the domain and the services. The database enforces only its own rules: keys, columns and references.
- **When a write happens.** The services, the export/import/restore jobs and the window decide that; storage is called. The restore and import rules themselves are in [Backup and restore](../backup-and-restore.md).
- **Reading the database for diagnostics.** The diagnostics report measures `app.db` and its sidecars through the filesystem and never opens them ([Diagnostics](../diagnostics.md)), because `proxies.outbound_json` holds credentials.
- **Browser data.** A profile's `user_data_dir` is a path stored as text. The directory behind it is copied by the browser-data backup, not by storage.

## Source of truth

| What | Where |
| --- | --- |
| Schema and migration runner | `crates/storage/src/migrations.rs` (`MIGRATIONS`, `run_migrations`) |
| Repository contracts | `crates/storage/src/traits.rs` (`ProfileRepository`, `ProxyRepository`, `CoreRepository`, `ConfigurationRepository`) |
| Failure taxonomy | `crates/storage/src/error.rs` (`StorageError`) |
| SQLite backend | `crates/storage/src/sqlite.rs` (`SqliteStorage`, `SqliteProfileRepository`, `SqliteProxyRepository`, `SqliteCoreRepository`, `refusal`, `missing_reference`, `used_by`) and `crates/storage/src/sqlite/codec.rs` (row-to-record mapping) |
| In-memory backend | `crates/storage/src/memory.rs` (`MemConfiguration`, `MemProfileRepository`, `MemProxyRepository`, `MemCoreRepository`) |
| Atomic file write | `crates/storage/src/atomic_file.rs` (`write`) |
| Production wiring | `crates/app/src/main.rs` (open, the three repositories, `ConfigurationRepository`); callers `crates/application/src/restore.rs` (`apply_restore`), `crates/application/src/export.rs` (`write_config_backup`), `crates/app/src/settings/persistence.rs` (`write_config`) |

The model behind the tables is in [Domain model](../domain-model.md); this page owns the schema, durability and the contracts.

### The schema

`MIGRATIONS` is an ordered list; a migration's version is its 1-based position, and its name is stored with the version it applied.

| Table | Columns | Notes |
| --- | --- | --- |
| `cores` | `id`, `name`, `executable`, `version`, `major`, `created_at`, `updated_at` | `id` is a `CoreId` written as UUID text; `executable` is the path the core was registered with, stored as text (no absoluteness is enforced; discovered, configured or imported paths are stored as given); `major` is the number capability resolution reads |
| `proxies` | `id`, `name`, `outbound_json`, `created_at`, `updated_at` | `outbound_json` is the `ProxyOutbound` as `serde_json`, credentials included |
| `profiles` | `id`, `name`, `core_id`, `user_data_dir`, `fingerprint_json`, `proxy_id`, `window_width`, `window_height`, `start_target_json`, `created_at`, `updated_at` | `id` is the `ProfileId` UUID text - the same value its browser-data directory is named after; `proxy_id` is `NULL` when the profile has no proxy; the JSON columns hold `FingerprintProfile` and `StartTarget` |
| `schema_migrations` | `version`, `name`, `applied_at` | which migration ran, and when |

Constraints the columns carry:

- `profiles.core_id` is a foreign key to `cores(id)` with no cascade: a core a profile names cannot be deleted.
- `profiles.proxy_id` is a nullable foreign key to `proxies(id)` with `ON DELETE RESTRICT` (migration `002_proxy_reference_is_restrictive`). It was `SET NULL` in the first schema; the migration rebuilt the table because SQLite cannot change a foreign key in place, copying every row and keeping `NULL` as the normal "no proxy" state.
- One index per column something looks up by value: `idx_profiles_core_id` and `idx_profiles_proxy_id` (migration `003_profile_reference_indexes`) serve the delete path and the `RESTRICT` action, which both ask "which profile uses this core or proxy"; `idx_proxies_name` and `idx_cores_name` exist from the initial schema, while `idx_profiles_name` is re-created by `002_proxy_reference_is_restrictive` as part of the profiles rebuild (dropping the table drops its indexes). Names are not unique - the identifier is the identity.
- `created_at` and `updated_at` are Unix seconds, written by `now_ts`; `created_at` is kept across an upsert and `updated_at` is bumped on every write. `list` returns records ordered `created_at ASC, id ASC`, which with second-precision timestamps is insertion order with the identifier as the tiebreaker.
- Paths (`executable`, `user_data_dir`) are stored through `to_string_lossy`, so a path that is not valid UTF-8 does not round-trip byte-for-byte.

### The repository contracts

The method lists are in [Service contracts](../service-contracts.md); what follows is what the signatures do not say.

- `ProfileRepository` has `get`, `list`, `insert`, `update` and `delete`, and deliberately no upsert. `insert` refuses an identifier that is taken (`StorageError::Conflict`); `update` refuses one that is not there (`StorageError::NotFound`). Both refuse a profile naming a core or proxy that is not stored, with a `StorageError::Dangling` whose message names which of the two it was (`missing_reference`). A profile is never silently overwritten: its identifier is what its browser-data directory is named after, and overwriting the row would lose the record of a directory still on disk. `delete` removes the row, is not an error when the row was not there, and never touches the directory.
- `ProxyRepository` and `CoreRepository` have `get`, `list`, `save` and `delete`. `save` is an upsert (`save_proxy`, `save_core`: `INSERT ... ON CONFLICT(id) DO UPDATE`): it writes the record under the identifier it carries, replacing whatever was stored under that identifier. Deciding when that may happen is the caller's job - an import writes only the identifiers its plan found free ([Backup and restore](../backup-and-restore.md)), and an edit goes through the application service. `delete` is refused while a profile still names the record, with a `StorageError::Conflict` that names the profile (`used_by`); once nothing names it, the delete removes it, and deleting an absent id is not an error.
- `ConfigurationRepository::replace(cores, proxies, profiles)` is total: on success every core, proxy and profile is one of the arguments; on failure nothing has changed. It exists because the three repositories above take the shared connection one record at a time, and a caller that deletes everything and then writes the new records commits after every step - the state where a crash, a full disk or a refused record in the middle leaves neither the old configuration nor the new one. Restore is the one caller ([Backup and restore](../backup-and-restore.md)).
- The traits are implemented by `SqliteStorage`/`Sqlite*Repository` and by the `Mem*Repository` types. Which implementation a caller gets is decided once, where the services are built (`crates/app/src/main.rs` for the program, the test fixtures for tests); the services themselves depend only on the traits.

## Main runtime paths

**Open at startup.** `crates/app/src/main.rs` calls `SqliteStorage::open(data_dir.join("app.db"))` before the window opens. `open` creates the parent directory, opens the file, calls `configure_connection` and runs the migrations; `SqliteStorage::in_memory` is the same sequence without a file, and is what the storage tests and the application's restore, import and export tests open; the application's service tests use the `Mem*` repositories instead, apart from two that run a service over the SQLite backend. From the one `SqliteStorage`, main builds the three repositories (`profiles()`, `cores()`, `proxies()`) and, with `storage.clone()`, one `Arc<dyn ConfigurationRepository>` over the same connection. A failure here is the one the window cannot show, because the window's own state would come from this database: the reason is printed, a diagnostics report is written, and the program exits with code 1.

**One record at a time.** Saving a profile, importing a record, registering a detected core, starting a profile (which reads the profile, its core, and its proxy if it has one) - each goes through one repository call: one SQL statement on the shared connection, committing on its own. Services hold no cache; a read is always answered from the database.

**The whole-configuration replace.** A confirmed restore reads the file, `plan_restore` validates every record first, and `apply_restore` hands the three lists to `ConfigurationRepository::replace`. `SqliteStorage::replace` opens one transaction, deletes `profiles`, then `proxies`, then `cores` (the order the foreign keys allow), writes cores, proxies and profiles back in the reverse order with the same statements the single-record path uses, and commits. A refused record, a dangling reference or a crash before the commit leaves the transaction uncommitted, and the installation is exactly what it was. Inside the batch, repeated core or proxy identifiers behave like any `save` (the later copy wins) and a repeated profile identifier fails like any `insert`; a restore refuses repeated identifiers before it gets here ([Backup and restore](../backup-and-restore.md)).

**Atomic file writes.** `write_config_backup` and the settings `write_config` call `atomic_file::write`: a temporary file is created in the destination's directory (`tempfile::NamedTempFile`, so the rename stays on one filesystem), the bytes are written and `sync_all`ed, and only then is it persisted over the destination - on Unix the parent directory is synced too. A failure at any point leaves the previous file in place and the temporary file removed. These two are the only callers.

**The in-memory backend.** `MemConfiguration::new(cores, proxies, profiles)` wires the three repositories to each other so the reference rules exist at all, and is what the application's runtime service tests and the window's and state's tests build. The wiring is `Weak`, so three repositories pointing at each other do not keep their storage alive past the test that made it, and a repository that has to read another one makes that check before taking its own lock, so the locks are always ordered the same way. `MemConfiguration::replace` lists the three maps first, replaces each in turn, and puts the old lists back if any step fails. The program itself never constructs it.

## State and transitions

- **Schema version.** An absent or empty database starts at version 0; `run_migrations` applies each `MIGRATIONS` entry above the recorded `MAX(version)`, each inside its own transaction that also inserts the `schema_migrations` row, so the database is never between a migration and its record. Opening a database already current applies nothing. A database whose recorded version is greater than `MIGRATIONS.len()` is refused with `StorageError::SchemaTooNew { found, supported }` before anything is applied: this build does not know what the migrations it never heard of mean.
- **Journal mode.** Chosen once per file at open. `configure_connection` asks for `journal_mode = WAL` and deliberately ignores the answer - a filesystem that cannot keep the shared memory (some network mounts) refuses the mode, and both modes are correct. WAL is a property of the database file, so a database once put in WAL stays in it. `synchronous = NORMAL` is set for the connection on every open. The two sidecar files exist while the database is open and are gone after a clean close.
- **The configuration.** `replace` is a transition between two whole configurations: the old one is visible until the commit, the new one after it, and nothing in between is observable. In memory the transition is emulated: the old lists are read before, and restored if a step fails.
- **Records and references.** A core or proxy can be deleted exactly while no profile names it. A profile's `proxy_id` can be `NULL`, which is the "no proxy" state and not a dangling reference.
- **Timestamps.** `created_at` is written once, at the record's first insert; `updated_at` moves with every `save` or `update`.

## Constraints and failure behavior

- **Concurrency.** `SqliteStorage` holds one `Arc<Mutex<Connection>>`; its three repositories and its `ConfigurationRepository` implementation share it, so every statement this process makes is serialized on that mutex. `busy_timeout = 5000` waits up to five seconds for a lock another process holds before failing. Nothing here makes two copies of the program safe on one data directory - the instance lock does not make them safe either, it makes the second one not start ([Exit modes](../exit-modes.md)).
- **Durability.** WAL with `synchronous = NORMAL` trades the last few committed transactions after a power loss (the database itself stays consistent - WAL commits atomically) for writes that no longer create, sync and delete a journal file each. The rows are configuration the user just typed, not data being streamed. The sidecar files `app.db-wal` and `app.db-shm` sit beside the database while it is open, and the diagnostics report names them ([Diagnostics](../diagnostics.md)).
- **Failure classification.** A write SQLite refuses is classified by its extended result code in `refusal`: `PRIMARYKEY`/`UNIQUE` is a taken identifier (`Refusal::Duplicate` to `StorageError::Conflict`); `FOREIGNKEY`, and `TRIGGER` - which is how SQLite reports an `ON DELETE RESTRICT` action - is a reference (`Refusal::Reference` to `StorageError::Dangling` on a profile write and to `Conflict` on a refused delete, each naming which reference); `NOTNULL`/`CHECK` is the schema refusing the record (`StorageError::Invalid`, carrying SQLite's own account of the column); anything else is `StorageError::Database`. A code the build has never heard of is a database failure, never guessed at as a duplicate.
- **Reading a stored value.** Identifiers are parsed with `Uuid::parse_str` and the JSON columns with `serde_json::from_str` (`codec.rs`); a value that cannot be decoded fails the read with `StorageError::Serialization` rather than being repaired, skipped or defaulted. A poisoned connection lock surfaces as `StorageError::Other`.
- **The remaining error variants.** `StorageError::Io` carries a filesystem error from the part of opening a database that creates its parent directory; the atomic write returns its own `io::Error` to the caller, which wraps it in the caller's error ([Backup and restore](../backup-and-restore.md) describes the export's). `NotFound` is what `ProfileRepository::update` answers for an identifier that is not stored; a repeated `insert` is a `Conflict` instead. Each calling layer passes the variants through rather than collapsing them into one string (`AppError::Storage` is transparent).
- **The memory backend's parity is bounded.** It shares the contracts' answers - the same assertion body runs against both backends - but not durability, concurrency, ordering or the schema: it has no file, its `list` returns `HashMap` iteration order rather than `created_at` order, and its only failure is a poisoned lock. Without `MemConfiguration::new` (or the individual `check_references`/`check_usage` wiring) it enforces no reference rules at all, because it has no foreign keys of its own.
- **Compatibility.** An older database is upgraded in place by `run_migrations`; `002` copies every column and keeps `NULL` proxies, and a test pins that no row is lost. A newer database is refused. Nothing downgrades a database.

## Verification

`cargo test -p storage` runs the crate's tests - 15 tests, none ignored. What each group proves:

- `the_database_keeps_the_repository_contract` and `the_memory_backend_keeps_the_repository_contract` run one body of assertions (`a_repository_contract`) against both backends: a taken identifier is a `Conflict` and nothing is written; a profile naming an unstored core or proxy is a `Dangling` naming which; a delete a profile still uses is refused naming the profile; and the same records are deletable once nothing names them.
- `the_database_replaces_the_whole_configuration` and `the_memory_backend_replaces_the_whole_configuration` run `replaces_the_whole_configuration` against both; `a_replacement_that_fails_part_way_leaves_the_old_configuration` pins the rollback with a foreign key violation on the second insert.
- `a_file_database_is_wal_and_its_reference_columns_are_indexed` asks an opened file for `journal_mode = wal`, `synchronous` NORMAL and the two reference indexes in `sqlite_master`; `test_sqlite_migration_idempotent` re-runs `run_migrations` on a migrated database; `a_database_from_the_previous_schema_upgrades_without_losing_rows` upgrades the first schema; `a_database_from_a_newer_build_is_refused` pins `SchemaTooNew`.
- `every_constraint_sqlite_reports_is_classified_by_its_extended_code` pins every `refusal` branch, including the unknown-code fallthrough.
- `a_partial_write_failure_preserves_the_previous_file` and `a_failed_rename_does_not_remove_the_target_or_leak_a_tempfile` cover `atomic_file::write`'s failure behavior.
- `test_sqlite_core_crud_and_roundtrip`, `test_sqlite_proxy_crud_and_all_outbound_types` and `test_sqlite_profile_crud_and_roundtrip` cover the record round-trips, including every outbound type and the `Conflict` on a repeated profile insert.

The contracts as the rest of the program uses them:

- `cargo test -p application --lib restore::tests` - the replace path over `SqliteStorage::in_memory`; `a_confirmed_restore_replaces_the_configuration_with_the_file`, `a_file_whose_profile_names_a_core_it_does_not_hold_changes_nothing` and `a_file_exported_without_credentials_changes_nothing` are the all-or-nothing and refusal behavior seen from the caller.
- `cargo test -p application --lib import::tests` - the sequence of single-record writes and its read-back, e.g. `an_import_writes_cores_proxies_and_profiles_that_line_up`.
- `cargo test -p application --lib export::tests` - the file written through `atomic_file::write`.
- `cargo test -p app` - the window's and state's tests build the services on `storage::MemConfiguration`, which is the backend their assertions rest on.

Opt-in: the storage crate has no `#[ignore]`d tests. The file database (rather than the in-memory one) is exercised by the app acceptance harness, whose module is Linux-only (`#[cfg(all(test, target_os = "linux"))]` on `mod real_browser` in `crates/app/src/main.rs`):

```
CHROMIUM_BIN=/path/to/chrome cargo test -p app real_chromium_start_stop_through_app_state -- --ignored --nocapture
```

as well as `real_chromium_fingerprint_verification_through_the_verifier` and `real_chromium_reports_the_address_its_own_traffic_leaves_from`, each of which opens `SqliteStorage` on a file in a temporary directory.

The gate and its acceptance rules are in [Code standards](../code-standards.md): `./scripts/check.sh` on Linux or `./scripts/check.ps1` on Windows.
