use crate::error::StorageError;
use crate::migrations::run_migrations;
use crate::traits::{CoreRepository, ProfileRepository, ProxyRepository};
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
            ORDER BY created_at ASC
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

        let id_str = profile.id.to_string();
        let core_id_str = profile.core_id.to_string();
        let user_data_str = profile.user_data_dir.to_string_lossy().to_string();
        let fingerprint_json = serde_json::to_string(&profile.fingerprint)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        let proxy_id_str = profile.proxy_id.map(|p| p.to_string());
        let start_target_json = serde_json::to_string(&profile.start_target)
            .map_err(|e| StorageError::Serialization(e.to_string()))?;
        let now = now_ts();

        let res = conn.execute(
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

        match res {
            Ok(_) => Ok(()),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Err(StorageError::Conflict(format!(
                    "profile with id {} already exists",
                    profile.id
                )))
            }
            Err(e) => Err(StorageError::Database(e)),
        }
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
        let mut stmt =
            conn.prepare("SELECT id, name, outbound_json FROM proxies ORDER BY created_at ASC")?;
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
        )?;

        Ok(())
    }

    fn delete(&self, id: ProxyId) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let id_str = id.to_string();
        conn.execute("DELETE FROM proxies WHERE id = ?1", params![id_str])?;
        Ok(())
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
            "SELECT id, name, executable, version, major FROM cores ORDER BY created_at ASC",
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
        )?;

        Ok(())
    }

    fn delete(&self, id: CoreId) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        let id_str = id.to_string();
        conn.execute("DELETE FROM cores WHERE id = ?1", params![id_str])?;
        Ok(())
    }
}
