use crate::error::StorageError;
use crate::migrations::run_migrations;
use crate::traits::{ConfigurationRepository, CoreRepository, ProfileRepository, ProxyRepository};
use domain::{
    BrowserCore, BrowserProfile, CoreId, FingerprintProfile, ProfileId, ProxyId, ProxyOutbound,
    ProxyProfile, StartTarget, WindowProfile,
};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Clone)]
pub struct SqliteStorage {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let path_ref = path.as_ref();
        if let Some(parent) = path_ref.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut conn = Connection::open(path_ref)?;
        Self::configure_connection(&mut conn)?;
        run_migrations(&mut conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn in_memory() -> Result<Self, StorageError> {
        let mut conn = Connection::open_in_memory()?;
        Self::configure_connection(&mut conn)?;
        run_migrations(&mut conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn configure_connection(conn: &mut Connection) -> Result<(), StorageError> {
        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            "#,
        )?;
        Ok(())
    }

    pub fn conn(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    pub fn profiles(&self) -> SqliteProfileRepository {
        SqliteProfileRepository {
            conn: Arc::clone(&self.conn),
        }
    }

    pub fn proxies(&self) -> SqliteProxyRepository {
        SqliteProxyRepository {
            conn: Arc::clone(&self.conn),
        }
    }

    pub fn cores(&self) -> SqliteCoreRepository {
        SqliteCoreRepository {
            conn: Arc::clone(&self.conn),
        }
    }
}

#[derive(Clone)]
pub struct SqliteProfileRepository {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteProfileRepository {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }
}

impl ProfileRepository for SqliteProfileRepository {
    fn get(&self, id: ProfileId) -> Result<Option<BrowserProfile>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, core_id, user_data_dir, fingerprint_json,
                   proxy_id, window_width, window_height, start_target_json
            FROM profiles
            WHERE id = ?1
            "#,
        )?;

        let id_str = id.to_string();
        let mut rows = stmt.query(params![id_str])?;

        if let Some(row) = rows.next()? {
            let id_raw: String = row.get(0)?;
            let name: String = row.get(1)?;
            let core_id_raw: String = row.get(2)?;
            let user_data_dir_str: String = row.get(3)?;
            let fingerprint_json: String = row.get(4)?;
            let proxy_id_raw: Option<String> = row.get(5)?;
            let width: u32 = row.get(6)?;
            let height: u32 = row.get(7)?;
            let start_target_json: String = row.get(8)?;

            let profile_id = ProfileId(
                Uuid::parse_str(&id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?,
            );
            let core_id = CoreId(
                Uuid::parse_str(&core_id_raw)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?,
            );
            let proxy_id = match proxy_id_raw {
                Some(p) => Some(ProxyId(
                    Uuid::parse_str(&p).map_err(|e| StorageError::Serialization(e.to_string()))?,
                )),
                None => None,
            };
            let fingerprint: FingerprintProfile = serde_json::from_str(&fingerprint_json)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            let start_target: StartTarget = serde_json::from_str(&start_target_json)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;

            Ok(Some(BrowserProfile {
                id: profile_id,
                name,
                core_id,
                user_data_dir: PathBuf::from(user_data_dir_str),
                fingerprint,
                proxy_id,
                window: WindowProfile::new(width, height),
                start_target,
            }))
        } else {
            Ok(None)
        }
    }

    fn list(&self) -> Result<Vec<BrowserProfile>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, name, core_id, user_data_dir, fingerprint_json,
                   proxy_id, window_width, window_height, start_target_json
            FROM profiles
            ORDER BY created_at ASC, id ASC
            "#,
        )?;

        let mut rows = stmt.query([])?;
        let mut list = Vec::new();

        while let Some(row) = rows.next()? {
            let id_raw: String = row.get(0)?;
            let name: String = row.get(1)?;
            let core_id_raw: String = row.get(2)?;
            let user_data_dir_str: String = row.get(3)?;
            let fingerprint_json: String = row.get(4)?;
            let proxy_id_raw: Option<String> = row.get(5)?;
            let width: u32 = row.get(6)?;
            let height: u32 = row.get(7)?;
            let start_target_json: String = row.get(8)?;

            let profile_id = ProfileId(
                Uuid::parse_str(&id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?,
            );
            let core_id = CoreId(
                Uuid::parse_str(&core_id_raw)
                    .map_err(|e| StorageError::Serialization(e.to_string()))?,
            );
            let proxy_id = match proxy_id_raw {
                Some(p) => Some(ProxyId(
                    Uuid::parse_str(&p).map_err(|e| StorageError::Serialization(e.to_string()))?,
                )),
                None => None,
            };
            let fingerprint: FingerprintProfile = serde_json::from_str(&fingerprint_json)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;
            let start_target: StartTarget = serde_json::from_str(&start_target_json)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;

            list.push(BrowserProfile {
                id: profile_id,
                name,
                core_id,
                user_data_dir: PathBuf::from(user_data_dir_str),
                fingerprint,
                proxy_id,
                window: WindowProfile::new(width, height),
                start_target,
            });
        }

        Ok(list)
    }

    fn insert(&self, profile: &BrowserProfile) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        insert_profile(&conn, profile)
    }

    fn update(&self, profile: &BrowserProfile) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;

        let id_str = profile.id.to_string();
        let core_id_str = profile.core_id.to_string();
        let user_data_str = profile.user_data_dir.to_string_lossy().to_string();
        let fingerprint_json = serde_json::to_string(&profile.fingerprint)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        let proxy_id_str = profile.proxy_id.map(|p| p.to_string());
        let start_target_json = serde_json::to_string(&profile.start_target)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        let now = now_ts();

        let affected = conn.execute(
            r#"
            UPDATE profiles SET
                name = ?2,
                core_id = ?3,
                user_data_dir = ?4,
                fingerprint_json = ?5,
                proxy_id = ?6,
                window_width = ?7,
                window_height = ?8,
                start_target_json = ?9,
                updated_at = ?10
            WHERE id = ?1
            "#,
            params![
                id_str,
                profile.name,
                core_id_str,
                user_data_str,
                fingerprint_json,
                proxy_id_str,
                profile.window.width,
                profile.window.height,
                start_target_json,
                now,
            ],
        )?;

        if affected == 0 {
            return Err(StorageError::NotFound(format!(
                "profile with id {} not found",
                profile.id
            )));
        }

        Ok(())
    }

    fn delete(&self, id: ProfileId) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let id_str = id.to_string();
        conn.execute("DELETE FROM profiles WHERE id = ?1", params![id_str])?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct SqliteProxyRepository {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteProxyRepository {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }
}

impl ProxyRepository for SqliteProxyRepository {
    fn get(&self, id: ProxyId) -> Result<Option<ProxyProfile>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let mut stmt = conn.prepare("SELECT id, name, outbound_json FROM proxies WHERE id = ?1")?;
        let id_str = id.to_string();
        let mut rows = stmt.query(params![id_str])?;

        if let Some(row) = rows.next()? {
            let id_raw: String = row.get(0)?;
            let name: String = row.get(1)?;
            let outbound_json: String = row.get(2)?;

            let proxy_id = ProxyId(
                Uuid::parse_str(&id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?,
            );
            let outbound: ProxyOutbound = serde_json::from_str(&outbound_json)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;

            Ok(Some(ProxyProfile {
                id: proxy_id,
                name,
                outbound,
            }))
        } else {
            Ok(None)
        }
    }

    fn list(&self) -> Result<Vec<ProxyProfile>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, name, outbound_json FROM proxies ORDER BY created_at ASC, id ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut list = Vec::new();

        while let Some(row) = rows.next()? {
            let id_raw: String = row.get(0)?;
            let name: String = row.get(1)?;
            let outbound_json: String = row.get(2)?;

            let proxy_id = ProxyId(
                Uuid::parse_str(&id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?,
            );
            let outbound: ProxyOutbound = serde_json::from_str(&outbound_json)
                .map_err(|e| StorageError::Serialization(e.to_string()))?;

            list.push(ProxyProfile {
                id: proxy_id,
                name,
                outbound,
            });
        }

        Ok(list)
    }

    fn save(&self, proxy: &ProxyProfile) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        save_proxy(&conn, proxy)
    }

    fn delete(&self, id: ProxyId) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let id_str = id.to_string();
        conn.execute("DELETE FROM proxies WHERE id = ?1", params![id_str])
            .map(|_| ())
            .map_err(|error| match refusal(error) {
                // A profile still names this proxy, and the foreign key refuses
                // the delete rather than clearing the reference. Named, because
                // "conflict" alone does not say which profile to look at.
                Refusal::Reference => {
                    StorageError::Conflict(used_by(&conn, "profiles", "proxy_id", &id_str, "proxy"))
                }
                // Unreachable for a delete: no unique key is written and every
                // column of the statement exists. Mapped rather than panicked,
                // because a classification is not worth a crash.
                Refusal::Duplicate => StorageError::Conflict(format!("proxy {id} is still used")),
                Refusal::Schema(detail) => StorageError::Invalid(format!("proxy {id}: {detail}")),
                Refusal::Other(error) => StorageError::Database(error),
            })
    }
}

#[derive(Clone)]
pub struct SqliteCoreRepository {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteCoreRepository {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }
}

impl CoreRepository for SqliteCoreRepository {
    fn get(&self, id: CoreId) -> Result<Option<BrowserCore>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let mut stmt =
            conn.prepare("SELECT id, name, executable, version, major FROM cores WHERE id = ?1")?;
        let id_str = id.to_string();
        let mut rows = stmt.query(params![id_str])?;

        if let Some(row) = rows.next()? {
            let id_raw: String = row.get(0)?;
            let name: String = row.get(1)?;
            let executable_str: String = row.get(2)?;
            let version: String = row.get(3)?;
            let major: u32 = row.get(4)?;

            let core_id = CoreId(
                Uuid::parse_str(&id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?,
            );

            Ok(Some(BrowserCore {
                id: core_id,
                name,
                executable: PathBuf::from(executable_str),
                version,
                major,
            }))
        } else {
            Ok(None)
        }
    }

    fn list(&self) -> Result<Vec<BrowserCore>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, name, executable, version, major FROM cores ORDER BY created_at ASC, id ASC",
        )?;
        let mut rows = stmt.query([])?;
        let mut list = Vec::new();

        while let Some(row) = rows.next()? {
            let id_raw: String = row.get(0)?;
            let name: String = row.get(1)?;
            let executable_str: String = row.get(2)?;
            let version: String = row.get(3)?;
            let major: u32 = row.get(4)?;

            let core_id = CoreId(
                Uuid::parse_str(&id_raw).map_err(|e| StorageError::Serialization(e.to_string()))?,
            );

            list.push(BrowserCore {
                id: core_id,
                name,
                executable: PathBuf::from(executable_str),
                version,
                major,
            });
        }

        Ok(list)
    }

    fn save(&self, core: &BrowserCore) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        save_core(&conn, core)
    }

    fn delete(&self, id: CoreId) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let id_str = id.to_string();
        conn.execute("DELETE FROM cores WHERE id = ?1", params![id_str])
            .map(|_| ())
            .map_err(|error| match refusal(error) {
                Refusal::Reference => {
                    StorageError::Conflict(used_by(&conn, "profiles", "core_id", &id_str, "core"))
                }
                Refusal::Duplicate => StorageError::Conflict(format!("core {id} is still used")),
                Refusal::Schema(detail) => StorageError::Invalid(format!("core {id}: {detail}")),
                Refusal::Other(error) => StorageError::Database(error),
            })
    }
}

impl ConfigurationRepository for SqliteStorage {
    fn replace(
        &self,
        cores: &[BrowserCore],
        proxies: &[ProxyProfile],
        profiles: &[BrowserProfile],
    ) -> Result<(), StorageError> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let tx = conn.transaction()?;

        // Removed in the order the foreign keys allow and written in the reverse,
        // exactly as the services would do it one call at a time; the difference
        // is that none of it is visible until the commit.
        tx.execute("DELETE FROM profiles", [])?;
        tx.execute("DELETE FROM proxies", [])?;
        tx.execute("DELETE FROM cores", [])?;
        for core in cores {
            save_core(&tx, core)?;
        }
        for proxy in proxies {
            save_proxy(&tx, proxy)?;
        }
        for profile in profiles {
            insert_profile(&tx, profile)?;
        }

        tx.commit()?;
        Ok(())
    }
}

/// One core, written on whatever connection is given.
///
/// A free function rather than a method because the same statement has to serve
/// two callers: a repository writing one record, and a replacement writing all of
/// them inside one transaction. Two copies of the SQL would be two places for the
/// column list to drift, and the drift would only show up as data read back
/// wrong.
fn save_core(conn: &Connection, core: &BrowserCore) -> Result<(), StorageError> {
    let id_str = core.id.to_string();
    let executable_str = core.executable.to_string_lossy().to_string();
    let now = now_ts();

    conn.execute(
        r#"
        INSERT INTO cores (id, name, executable, version, major, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            executable = excluded.executable,
            version = excluded.version,
            major = excluded.major,
            updated_at = excluded.updated_at
        "#,
        params![
            id_str,
            core.name,
            executable_str,
            core.version,
            core.major,
            now,
            now
        ],
    )
    .map(|_| ())
    .map_err(|error| {
        // This statement is an upsert, so a taken identifier is not a refusal,
        // and a core references nothing. The two arms are here because the
        // classification is total, not because they can happen - and a value the
        // schema will not take is very much reachable, where "database error"
        // would not say which column it was.
        match refusal(error) {
            Refusal::Schema(detail) => StorageError::Invalid(format!("core {}: {detail}", core.id)),
            Refusal::Duplicate => StorageError::Conflict(format!("core {} is taken", core.id)),
            Refusal::Reference => {
                StorageError::Dangling(format!("core {} names what is not stored", core.id))
            }
            Refusal::Other(error) => StorageError::Database(error),
        }
    })
}

/// One proxy, written on whatever connection is given.
fn save_proxy(conn: &Connection, proxy: &ProxyProfile) -> Result<(), StorageError> {
    let id_str = proxy.id.to_string();
    let outbound_json = serde_json::to_string(&proxy.outbound)
        .map_err(|e| StorageError::Serialization(e.to_string()))?;
    let now = now_ts();

    conn.execute(
        r#"
        INSERT INTO proxies (id, name, outbound_json, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            outbound_json = excluded.outbound_json,
            updated_at = excluded.updated_at
        "#,
        params![id_str, proxy.name, outbound_json, now, now],
    )
    .map(|_| ())
    .map_err(|error| match refusal(error) {
        Refusal::Schema(detail) => StorageError::Invalid(format!("proxy {}: {detail}", proxy.id)),
        Refusal::Duplicate => StorageError::Conflict(format!("proxy {} is taken", proxy.id)),
        Refusal::Reference => {
            StorageError::Dangling(format!("proxy {} names what is not stored", proxy.id))
        }
        Refusal::Other(error) => StorageError::Database(error),
    })
}

/// One profile, written on whatever connection is given.
///
/// Unlike the other two this is an insert and not an upsert, because a profile's
/// rows are what a browser's data directory is tied to: overwriting one silently
/// would be a way to lose the record of a directory that is still on disk.
fn insert_profile(conn: &Connection, profile: &BrowserProfile) -> Result<(), StorageError> {
    let id_str = profile.id.to_string();
    let core_id_str = profile.core_id.to_string();
    let user_data_str = profile.user_data_dir.to_string_lossy().to_string();
    let fingerprint_json = serde_json::to_string(&profile.fingerprint)
        .map_err(|e| StorageError::Serialization(e.to_string()))?;
    let proxy_id_str = profile.proxy_id.map(|p| p.to_string());
    let start_target_json = serde_json::to_string(&profile.start_target)
        .map_err(|e| StorageError::Serialization(e.to_string()))?;
    let now = now_ts();

    let result = conn.execute(
        r#"
        INSERT INTO profiles (
            id, name, core_id, user_data_dir, fingerprint_json,
            proxy_id, window_width, window_height, start_target_json,
            created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        "#,
        params![
            id_str,
            profile.name,
            core_id_str,
            user_data_str,
            fingerprint_json,
            proxy_id_str,
            profile.window.width,
            profile.window.height,
            start_target_json,
            now,
            now,
        ],
    );

    result.map(|_| ()).map_err(|error| match refusal(error) {
        // The identifier is taken. Which of the two constraints it was does not
        // change the answer: the row cannot go in as it stands.
        Refusal::Duplicate => {
            StorageError::Conflict(format!("profile {} already exists", profile.id))
        }
        // A reference resolves to nothing, and SQLite's message does not say
        // which one, so the question is asked directly.
        Refusal::Reference => StorageError::Dangling(missing_reference(conn, profile)),
        Refusal::Schema(detail) => {
            StorageError::Invalid(format!("profile {}: {detail}", profile.id))
        }
        Refusal::Other(error) => StorageError::Database(error),
    })
}

/// Why SQLite refused a write, as far as its extended error code can say.
///
/// A duplicate key, a foreign key that resolves to nothing and a value the schema
/// itself refuses all arrive as `ConstraintViolation`; only the extended code
/// tells them apart, and nothing outside this module should have to read SQLite's
/// English message to find out which one happened.
enum Refusal {
    /// The identifier is already taken.
    Duplicate,
    /// A foreign key refused the statement - either a reference that resolves to
    /// nothing, or a delete that would leave a referencing row dangling.
    ///
    /// One variant for both because SQLite reports them with two different codes
    /// and the same meaning to a caller: an insert or update naming something
    /// missing comes back as `FOREIGNKEY`, and a delete that a `RESTRICT` action
    /// refuses comes back as `TRIGGER`, because SQLite implements the action as an
    /// internal trigger program. Which reference, and which record still points at
    /// it, is the caller's to name.
    Reference,
    /// The record cannot be stored as it stands - a `NOT NULL` that was not met,
    /// or a `CHECK` that did not hold - with SQLite's own account of the column.
    Schema(String),
    /// Anything else, including every failure that is not a constraint at all.
    Other(rusqlite::Error),
}

fn refusal(error: rusqlite::Error) -> Refusal {
    let (code, extended, detail) = match &error {
        rusqlite::Error::SqliteFailure(code, detail) => (code.code, code.extended_code, detail),
        _ => return Refusal::Other(error),
    };
    if code != rusqlite::ErrorCode::ConstraintViolation {
        return Refusal::Other(error);
    }
    match extended {
        rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY | rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE => {
            Refusal::Duplicate
        }
        rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY | rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER => {
            Refusal::Reference
        }
        rusqlite::ffi::SQLITE_CONSTRAINT_NOTNULL | rusqlite::ffi::SQLITE_CONSTRAINT_CHECK => {
            Refusal::Schema(
                detail
                    .clone()
                    .unwrap_or_else(|| "the schema refused it".to_string()),
            )
        }
        _ => Refusal::Other(error),
    }
}

/// Which of a profile's references is not stored, named.
///
/// The two have different consequences - a profile without its core cannot be
/// launched at all, and a profile without its proxy would go direct - so the
/// answer names the one that is missing rather than saying "a core or a proxy".
/// Read on the way out of a refused write, which is why it is a query rather than
/// a check: the write is the thing that decided, and this is only the sentence.
fn missing_reference(conn: &Connection, profile: &BrowserProfile) -> String {
    if stored(conn, "cores", &profile.core_id.to_string()).is_none() {
        return format!(
            "profile {} names core {} which is not stored",
            profile.id, profile.core_id
        );
    }
    if let Some(proxy_id) = profile.proxy_id
        && stored(conn, "proxies", &proxy_id.to_string()).is_none()
    {
        return format!(
            "profile {} names proxy {proxy_id} which is not stored",
            profile.id
        );
    }
    // A reference this connection can resolve now, so the write was refused for
    // something else - a race with another writer, or a constraint this does not
    // model. Said as what it is rather than guessed at.
    format!(
        "profile {} names a reference the schema refused",
        profile.id
    )
}

/// Why a delete was refused, naming what still uses the record.
///
/// `table` and `column` are literals from this module, never built from input, so
/// the statement this builds is one of a fixed few.
fn used_by(conn: &Connection, table: &str, column: &str, id: &str, kind: &str) -> String {
    let sql = format!("SELECT name FROM {table} WHERE {column} = ?1 LIMIT 1");
    match conn.query_row(&sql, params![id], |row| row.get::<_, String>(0)) {
        Ok(user) => format!("{kind} {id} is still used by {user}"),
        // The reference is there somewhere - the delete was refused - so a
        // profile whose name cannot be read is still worth the sentence.
        Err(_) => format!("{kind} {id} is still used"),
    }
}

/// Whether a record with this identifier is stored, by name.
fn stored(conn: &Connection, table: &str, id: &str) -> Option<String> {
    let sql = format!("SELECT name FROM {table} WHERE id = ?1");
    conn.query_row(&sql, params![id], |row| row.get::<_, String>(0))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::ffi;

    /// A constraint failure as SQLite hands it over: one primary code, and the
    /// extended code that is the only thing saying which constraint it was.
    fn failure(extended: i32) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            ffi::Error {
                code: rusqlite::ErrorCode::ConstraintViolation,
                extended_code: extended,
            },
            Some("constraint failed: profiles.name".to_string()),
        )
    }

    /// SQLite reports four different mistakes with one code, and everything
    /// downstream branches on the answer - so every code this program can meet is
    /// pinned here, including the one that used to be read as "already exists".
    #[test]
    fn every_constraint_sqlite_reports_is_classified_by_its_extended_code() {
        for duplicate in [
            ffi::SQLITE_CONSTRAINT_PRIMARYKEY,
            ffi::SQLITE_CONSTRAINT_UNIQUE,
        ] {
            assert!(
                matches!(refusal(failure(duplicate)), Refusal::Duplicate),
                "{duplicate} is a taken identifier"
            );
        }
        // Both of the codes a foreign key comes back as: an insert or update that
        // names something missing, and a delete an action refused.
        for reference in [
            ffi::SQLITE_CONSTRAINT_FOREIGNKEY,
            ffi::SQLITE_CONSTRAINT_TRIGGER,
        ] {
            assert!(
                matches!(refusal(failure(reference)), Refusal::Reference),
                "{reference} is a foreign key refusing the statement"
            );
        }
        for refused in [ffi::SQLITE_CONSTRAINT_NOTNULL, ffi::SQLITE_CONSTRAINT_CHECK] {
            let Refusal::Schema(detail) = refusal(failure(refused)) else {
                panic!("{refused} is the schema refusing the record");
            };
            assert!(detail.contains("profiles.name"), "{detail}");
        }

        // An extended code this build has never heard of is a database failure,
        // not a guess: "already exists" about an unknown failure is exactly the
        // mistake this classification was written to stop making.
        assert!(matches!(refusal(failure(0)), Refusal::Other(_)));
        // And a failure that is not a constraint at all is not classified either.
        assert!(matches!(
            refusal(rusqlite::Error::QueryReturnedNoRows),
            Refusal::Other(_)
        ));
    }
}
