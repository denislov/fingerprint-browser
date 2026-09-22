use crate::error::StorageError;
use rusqlite::Connection;

pub const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_initial_schema",
        r#"
    CREATE TABLE IF NOT EXISTS schema_migrations (
        version INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        applied_at INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS cores (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        executable TEXT NOT NULL,
        version TEXT NOT NULL,
        major INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS proxies (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        outbound_json TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS profiles (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        core_id TEXT NOT NULL,
        user_data_dir TEXT NOT NULL,
        fingerprint_json TEXT NOT NULL,
        proxy_id TEXT,
        window_width INTEGER NOT NULL,
        window_height INTEGER NOT NULL,
        start_target_json TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        FOREIGN KEY (core_id) REFERENCES cores(id),
        FOREIGN KEY (proxy_id) REFERENCES proxies(id) ON DELETE SET NULL
    );

    CREATE INDEX IF NOT EXISTS idx_profiles_name ON profiles(name);
    CREATE INDEX IF NOT EXISTS idx_proxies_name ON proxies(name);
    CREATE INDEX IF NOT EXISTS idx_cores_name ON cores(name);
    "#,
    ),
    (
        "002_proxy_reference_is_restrictive",
        // A profile's proxy was `ON DELETE SET NULL`, so deleting a proxy the
        // database could see a profile using silently left that profile with no
        // proxy. The service refuses that deletion by checking who uses the proxy
        // first, which is a rule between two reads rather than a guarantee - and
        // the row that decides it is not the row the delete touches. `RESTRICT`
        // is the same rule where nothing can go around it, including a future
        // caller that deletes proxies directly, or two of them at once.
        //
        // SQLite cannot change a foreign key in place, so the table is rebuilt:
        // the rows are copied unchanged, and `NULL` - the state a profile whose
        // proxy was already cleared before this migration is in - is still
        // allowed, because "no proxy" is a normal profile and not a dangling
        // reference.
        r#"
    CREATE TABLE profiles_new (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        core_id TEXT NOT NULL,
        user_data_dir TEXT NOT NULL,
        fingerprint_json TEXT NOT NULL,
        proxy_id TEXT,
        window_width INTEGER NOT NULL,
        window_height INTEGER NOT NULL,
        start_target_json TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        FOREIGN KEY (core_id) REFERENCES cores(id),
        FOREIGN KEY (proxy_id) REFERENCES proxies(id) ON DELETE RESTRICT
    );

    INSERT INTO profiles_new (
        id, name, core_id, user_data_dir, fingerprint_json, proxy_id,
        window_width, window_height, start_target_json, created_at, updated_at
    )
    SELECT
        id, name, core_id, user_data_dir, fingerprint_json, proxy_id,
        window_width, window_height, start_target_json, created_at, updated_at
    FROM profiles;

    DROP TABLE profiles;
    ALTER TABLE profiles_new RENAME TO profiles;

    CREATE INDEX IF NOT EXISTS idx_profiles_name ON profiles(name);
    "#,
    ),
];

pub fn run_migrations(conn: &mut Connection) -> Result<(), StorageError> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        );
        "#,
    )?;

    // Asked rather than assumed: a database this program cannot read is a
    // database whose version it does not know, and reading it as zero would
    // apply every migration over a schema that may already have them.
    let current_version: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    let known = MIGRATIONS.len() as i64;
    if current_version > known {
        return Err(StorageError::SchemaTooNew {
            found: current_version,
            supported: known,
        });
    }

    for (idx, (name, sql)) in MIGRATIONS.iter().enumerate() {
        let version = (idx + 1) as i64;
        if version > current_version {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            tx.execute(
                "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![version, name, now],
            )?;
            tx.commit()?;
        }
    }

    Ok(())
}
