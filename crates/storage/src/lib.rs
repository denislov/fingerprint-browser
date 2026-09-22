pub mod atomic_file;
pub mod error;
pub mod memory;
pub mod migrations;
pub mod sqlite;
pub mod traits;

pub use error::StorageError;
pub use memory::{MemConfiguration, MemCoreRepository, MemProfileRepository, MemProxyRepository};
pub use migrations::run_migrations;
pub use sqlite::{
    SqliteCoreRepository, SqliteProfileRepository, SqliteProxyRepository, SqliteStorage,
};
pub use traits::{ConfigurationRepository, CoreRepository, ProfileRepository, ProxyRepository};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migrations::MIGRATIONS;
    use domain::{
        BrowserBrand, BrowserCore, BrowserProfile, CoreId, FingerprintProfile, HttpOutbound,
        Platform, ProfileId, ProxyId, ProxyOutbound, ProxyProfile, ShadowsocksOutbound,
        Socks5Outbound, SpoofingFeature, StartTarget, StreamSettings, TrojanOutbound,
        VlessOutbound, VmessOutbound, WebRtcPolicy, WindowProfile,
    };
    use std::path::PathBuf;
    use std::sync::Arc;

    #[test]
    fn test_sqlite_migration_idempotent() {
        let storage = SqliteStorage::in_memory().expect("init in-memory sqlite");
        let conn_arc = storage.conn();
        let mut conn = conn_arc.lock().unwrap();
        assert!(run_migrations(&mut conn).is_ok());
    }

    #[test]
    fn test_sqlite_core_crud_and_roundtrip() {
        let storage = SqliteStorage::in_memory().expect("init in-memory sqlite");
        let core_repo = storage.cores();

        let core_id = CoreId::new();
        let core = BrowserCore {
            id: core_id,
            name: "Chromium 128 Dedicated".to_string(),
            executable: PathBuf::from("C:\\cores\\chromium-128\\chrome.exe"),
            version: "128.0.6613.120".to_string(),
            major: 128,
        };

        // Save
        core_repo.save(&core).expect("save core");

        // Get
        let fetched = core_repo
            .get(core_id)
            .expect("get core")
            .expect("core exists");
        assert_eq!(fetched.id, core.id);
        assert_eq!(fetched.name, core.name);
        assert_eq!(fetched.executable, core.executable);
        assert_eq!(fetched.version, core.version);
        assert_eq!(fetched.major, core.major);

        // List
        let list = core_repo.list().expect("list cores");
        assert_eq!(list.len(), 1);

        // Delete
        core_repo.delete(core_id).expect("delete core");
        assert!(core_repo.get(core_id).expect("get core").is_none());
        assert_eq!(core_repo.list().expect("list cores").len(), 0);
    }

    #[test]
    fn test_sqlite_proxy_crud_and_all_outbound_types() {
        let storage = SqliteStorage::in_memory().expect("init in-memory sqlite");
        let proxy_repo = storage.proxies();

        let proxies = vec![
            ProxyProfile {
                id: ProxyId::new(),
                name: "Socks5 Proxy".to_string(),
                outbound: ProxyOutbound::Socks5(Socks5Outbound {
                    host: "127.0.0.1".to_string(),
                    port: 1080,
                    username: Some("user".to_string()),
                    password: Some("pass".to_string()),
                }),
            },
            ProxyProfile {
                id: ProxyId::new(),
                name: "HTTP Proxy".to_string(),
                outbound: ProxyOutbound::Http(HttpOutbound {
                    host: "10.0.0.1".to_string(),
                    port: 8080,
                    username: None,
                    password: None,
                }),
            },
            ProxyProfile {
                id: ProxyId::new(),
                name: "Shadowsocks Proxy".to_string(),
                outbound: ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                    host: "shadow.node.com".to_string(),
                    port: 8388,
                    password: "secret_password".to_string(),
                    method: "aes-256-gcm".to_string(),
                    stream: StreamSettings::plain(),
                }),
            },
            ProxyProfile {
                id: ProxyId::new(),
                name: "Vmess Proxy".to_string(),
                outbound: ProxyOutbound::Vmess(VmessOutbound {
                    host: "vmess.node.com".to_string(),
                    port: 443,
                    uuid: "550e8400-e29b-41d4-a716-446655440000".to_string(),
                    security: "auto".to_string(),
                    alter_id: 0,
                    stream: StreamSettings::plain(),
                }),
            },
            ProxyProfile {
                id: ProxyId::new(),
                name: "Vless Proxy".to_string(),
                outbound: ProxyOutbound::Vless(VlessOutbound {
                    host: "vless.node.com".to_string(),
                    port: 443,
                    uuid: "550e8400-e29b-41d4-a716-446655440001".to_string(),
                    flow: Some("xtls-rprx-vision".to_string()),
                    encryption: "none".to_string(),
                    stream: StreamSettings::plain(),
                }),
            },
            ProxyProfile {
                id: ProxyId::new(),
                name: "Trojan Proxy".to_string(),
                outbound: ProxyOutbound::Trojan(TrojanOutbound {
                    host: "trojan.node.com".to_string(),
                    port: 443,
                    password: "trojan_pass".to_string(),
                    stream: StreamSettings::plain(),
                }),
            },
        ];

        for proxy in &proxies {
            proxy_repo.save(proxy).expect("save proxy");
            let fetched = proxy_repo
                .get(proxy.id)
                .expect("get proxy")
                .expect("proxy exists");
            assert_eq!(fetched.id, proxy.id);
            assert_eq!(fetched.name, proxy.name);
            assert_eq!(fetched.outbound, proxy.outbound);
        }

        let list = proxy_repo.list().expect("list proxies");
        assert_eq!(list.len(), proxies.len());
    }

    #[test]
    fn test_sqlite_profile_crud_and_roundtrip() {
        let storage = SqliteStorage::in_memory().expect("init in-memory sqlite");
        let core_repo = storage.cores();
        let proxy_repo = storage.proxies();
        let profile_repo = storage.profiles();

        // Insert core first
        let core_id = CoreId::new();
        let core = BrowserCore {
            id: core_id,
            name: "Default Core".to_string(),
            executable: PathBuf::from("chrome/chrome.exe"),
            version: "128.0.0.0".to_string(),
            major: 128,
        };
        core_repo.save(&core).expect("save core");

        // Insert proxy
        let proxy_id = ProxyId::new();
        let proxy = ProxyProfile {
            id: proxy_id,
            name: "Primary Proxy".to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "127.0.0.1".to_string(),
                port: 1080,
                username: None,
                password: None,
            }),
        };
        proxy_repo.save(&proxy).expect("save proxy");

        let profile_id = ProfileId::new();
        let profile = BrowserProfile {
            id: profile_id,
            name: "Roundtrip Test Profile".to_string(),
            core_id,
            user_data_dir: PathBuf::from("data/profiles/roundtrip_test"),
            fingerprint: FingerprintProfile {
                seed: 987654,
                brand: BrowserBrand::Edge,
                brand_version: Some("128.0.0".to_string()),
                platform: Platform::Linux,
                platform_version: Some("Ubuntu 24.04".to_string()),
                language: "zh-CN".to_string(),
                accept_language: "zh-CN,zh;q=0.9,en;q=0.8".to_string(),
                timezone: "Asia/Shanghai".to_string(),
                hardware_concurrency: Some(16),
                webrtc_policy: WebRtcPolicy::DisableNonProxiedUdp,
                disabled_spoofing: vec![
                    SpoofingFeature::Canvas,
                    SpoofingFeature::Audio,
                    SpoofingFeature::Gpu,
                ],
            },
            proxy_id: Some(proxy_id),
            window: WindowProfile::new(2560, 1440),
            start_target: StartTarget::Url("https://fingerprint.com".to_string()),
        };

        // Insert
        profile_repo.insert(&profile).expect("insert profile");

        // Duplicate insert should return Conflict error
        assert!(matches!(
            profile_repo.insert(&profile),
            Err(StorageError::Conflict(_))
        ));

        // Get
        let fetched = profile_repo
            .get(profile_id)
            .expect("get profile")
            .expect("profile exists");
        assert_eq!(fetched.id, profile.id);
        assert_eq!(fetched.name, profile.name);
        assert_eq!(fetched.core_id, profile.core_id);
        assert_eq!(fetched.user_data_dir, profile.user_data_dir);
        assert_eq!(fetched.fingerprint, profile.fingerprint);
        assert_eq!(fetched.proxy_id, profile.proxy_id);
        assert_eq!(fetched.window, profile.window);
        assert_eq!(fetched.start_target, profile.start_target);

        // Update
        let mut updated = profile.clone();
        updated.name = "Renamed Profile".to_string();
        updated.window = WindowProfile::new(1920, 1080);
        profile_repo.update(&updated).expect("update profile");

        let fetched_updated = profile_repo
            .get(profile_id)
            .expect("get updated profile")
            .expect("profile exists");
        assert_eq!(fetched_updated.name, "Renamed Profile");
        assert_eq!(fetched_updated.window.width, 1920);

        // Delete
        profile_repo.delete(profile_id).expect("delete profile");
        assert!(profile_repo.get(profile_id).expect("get profile").is_none());
    }

    /// A replacement that fails part way through leaves the configuration it
    /// started from.
    ///
    /// The failure is a profile naming a core the batch does not hold, which the
    /// database refuses with a foreign key violation on the *second* insert: the
    /// first profile is already written when it happens. A sequence of repository
    /// calls would have committed that first profile and the deletions before it,
    /// which is the "neither the old configuration nor the new one" state the
    /// transaction exists to make impossible.
    #[test]
    fn a_replacement_that_fails_part_way_leaves_the_old_configuration() {
        use crate::ConfigurationRepository as _;

        let storage = SqliteStorage::in_memory().expect("init in-memory sqlite");
        let core_repo = storage.cores();
        let profile_repo = storage.profiles();

        // What is installed now, and what has to still be installed afterwards.
        let installed_core = BrowserCore {
            id: CoreId::new(),
            name: "Installed core".to_string(),
            executable: PathBuf::from("/opt/chromium/chrome"),
            version: "148.0.0.0".to_string(),
            major: 148,
        };
        core_repo.save(&installed_core).expect("save core");
        profile_repo
            .insert(&profile(&installed_core.id, "Installed profile"))
            .expect("save profile");

        // What the replacement would write: one profile for the new core and one
        // for a core that is not in the batch at all.
        let new_core = BrowserCore {
            id: CoreId::new(),
            name: "New core".to_string(),
            ..installed_core.clone()
        };
        let good = profile(&new_core.id, "New profile");
        let dangling = profile(&CoreId::new(), "Dangling profile");

        let error = storage
            .replace(&[new_core], &[], &[good, dangling])
            .expect_err("a profile without its core cannot be stored");
        assert!(
            matches!(error, StorageError::Dangling(_)),
            "a profile naming a core that is not stored is a dangling reference, not a duplicate: {error}"
        );

        // Everything the failure touched is as it was.
        assert_eq!(core_repo.list().expect("list").len(), 1);
        assert_eq!(core_repo.list().expect("list")[0].id, installed_core.id);
        let profiles = profile_repo.list().expect("list");
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "Installed profile");
    }

    /// A profile for a transaction test, with the fields the test does not care
    /// about filled in.
    fn profile(core_id: &CoreId, name: &str) -> BrowserProfile {
        BrowserProfile {
            id: ProfileId::new(),
            name: name.to_string(),
            core_id: *core_id,
            user_data_dir: PathBuf::from("data/profiles/test"),
            fingerprint: FingerprintProfile::new_random(7),
            proxy_id: None,
            window: WindowProfile::new(1280, 800),
            start_target: StartTarget::Blank,
        }
    }

    fn proxy(name: &str) -> ProxyProfile {
        ProxyProfile {
            id: ProxyId::new(),
            name: name.to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "127.0.0.1".to_string(),
                port: 1080,
                username: None,
                password: None,
            }),
        }
    }

    /// A database written before that rule keeps every row it had - including the
    /// profiles whose proxy was already `NULL`, which is a profile without a
    /// proxy and not a dangling reference.
    #[test]
    fn a_database_from_the_previous_schema_upgrades_without_losing_rows() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .expect("pragmas");
        conn.execute_batch(MIGRATIONS[0].1)
            .expect("the first schema");
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (1, '001_initial_schema', 0)",
            [],
        )
        .expect("record the first migration");
        conn.execute(
            "INSERT INTO cores VALUES ('core', 'Chromium', '/opt/chrome', '148', 148, 0, 0)",
            [],
        )
        .expect("seed a core");
        conn.execute(
            "INSERT INTO proxies VALUES ('proxy', 'Zurich', '{}', 0, 0)",
            [],
        )
        .expect("seed a proxy");
        conn.execute(
            "INSERT INTO profiles VALUES ('one', 'Work', 'core', '/data/one', '{}', 'proxy', 800, 600, 'null', 0, 0)",
            [],
        )
        .expect("seed a profile with a proxy");
        conn.execute(
            "INSERT INTO profiles VALUES ('two', 'Plain', 'core', '/data/two', '{}', NULL, 800, 600, 'null', 0, 0)",
            [],
        )
        .expect("seed a profile without one");

        run_migrations(&mut conn).expect("the upgrade runs");

        let with_proxy: Option<String> = conn
            .query_row(
                "SELECT proxy_id FROM profiles WHERE id = 'one'",
                [],
                |row| row.get(0),
            )
            .expect("the profile survived the rebuild");
        assert_eq!(with_proxy.as_deref(), Some("proxy"));
        let without: Option<String> = conn
            .query_row(
                "SELECT proxy_id FROM profiles WHERE id = 'two'",
                [],
                |row| row.get(0),
            )
            .expect("so did the one without a proxy");
        assert_eq!(without, None);
        assert!(
            conn.execute("DELETE FROM proxies WHERE id = 'proxy'", [])
                .is_err()
        );
        assert!(
            conn.execute("DELETE FROM proxies WHERE id = 'nonexistent'", [])
                .is_ok()
        );
    }

    /// A database from a newer build is refused rather than opened: this build
    /// does not know what the migrations it never heard of mean.
    #[test]
    fn a_database_from_a_newer_build_is_refused() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        conn.execute_batch(MIGRATIONS[0].1)
            .expect("the first schema");
        conn.execute(
            "INSERT INTO schema_migrations (version, name, applied_at) VALUES (99, 'future', 0)",
            [],
        )
        .expect("a newer schema");

        let error = run_migrations(&mut conn).expect_err("a newer schema is not this build's");
        assert!(
            matches!(
                error,
                StorageError::SchemaTooNew {
                    found: 99,
                    supported: 2
                }
            ),
            "{error}"
        );
    }

    /// The one operation both backends have, answered the same way by both.
    ///
    /// The application tests its strict restore against the memory backend and
    /// the program runs it against the database, so "the same" is the property
    /// that makes those tests evidence for anything.
    fn replaces_the_whole_configuration(
        configuration: &dyn ConfigurationRepository,
        cores: &dyn CoreRepository,
        proxies: &dyn ProxyRepository,
        profiles: &dyn ProfileRepository,
    ) {
        // What is there before, none of which may survive.
        let old_core = BrowserCore {
            id: CoreId::new(),
            name: "Old core".to_string(),
            executable: PathBuf::from("/opt/old/chrome"),
            version: "120.0.0.0".to_string(),
            major: 120,
        };
        cores.save(&old_core).expect("seed a core");
        let old_proxy = proxy("Old proxy");
        proxies.save(&old_proxy).expect("seed a proxy");
        let mut old_profile = profile(&old_core.id, "Old profile");
        old_profile.proxy_id = Some(old_proxy.id);
        profiles.insert(&old_profile).expect("seed a profile");

        let core = BrowserCore {
            id: CoreId::new(),
            name: "New core".to_string(),
            executable: PathBuf::from("/opt/new/chrome"),
            version: "148.0.0.0".to_string(),
            major: 148,
        };
        let proxy = proxy("New proxy");
        let mut work = profile(&core.id, "Work laptop");
        work.proxy_id = Some(proxy.id);

        configuration
            .replace(
                std::slice::from_ref(&core),
                std::slice::from_ref(&proxy),
                std::slice::from_ref(&work),
            )
            .expect("replacement");

        assert_eq!(cores.list().expect("cores"), vec![core]);
        assert_eq!(proxies.list().expect("proxies"), vec![proxy]);
        assert_eq!(profiles.list().expect("profiles"), vec![work]);
    }

    /// One body of assertions, run against both backends.
    ///
    /// The memory backend is what the application's tests run against, so a rule
    /// that lives only in the SQLite schema is a rule those tests do not exercise,
    /// and a rule that lives only in the memory backend is one the program does
    /// not have. This is the list of answers both have to give.
    fn a_repository_contract(
        cores: &dyn CoreRepository,
        proxies: &dyn ProxyRepository,
        profiles: &dyn ProfileRepository,
    ) {
        let core = BrowserCore {
            id: CoreId::new(),
            name: "Chromium".to_string(),
            executable: PathBuf::from("/opt/chromium/chrome"),
            version: "148.0.0.0".to_string(),
            major: 148,
        };
        cores.save(&core).expect("save a core");
        let proxy = proxy("Zurich exit");
        proxies.save(&proxy).expect("save a proxy");
        let mut work = profile(&core.id, "Work laptop");
        work.proxy_id = Some(proxy.id);
        profiles.insert(&work).expect("save a profile");

        // A taken identifier is a conflict, and the record already there is not
        // touched.
        let mut again = profile(&core.id, "Another");
        again.id = work.id;
        let error = profiles
            .insert(&again)
            .expect_err("one identifier, one profile");
        assert!(
            matches!(error, StorageError::Conflict(_)),
            "a taken identifier is a conflict: {error}"
        );
        assert_eq!(
            profiles.list().expect("profiles").len(),
            1,
            "and nothing was written"
        );

        // A reference that resolves to nothing is its own answer, and the message
        // says which reference it was: the two have different fixes.
        let no_core = profile(&CoreId::new(), "No core");
        let error = profiles
            .insert(&no_core)
            .expect_err("a profile without its core cannot be stored");
        assert!(
            matches!(&error, StorageError::Dangling(reason) if reason.contains("core")),
            "{error}"
        );

        let mut no_proxy = profile(&core.id, "No proxy");
        no_proxy.proxy_id = Some(ProxyId::new());
        let error = profiles
            .insert(&no_proxy)
            .expect_err("a profile naming a proxy that is not stored");
        assert!(
            matches!(&error, StorageError::Dangling(reason) if reason.contains("proxy")),
            "{error}"
        );

        // A reference in use is not silently dropped: the record that something
        // still names cannot be deleted, and the profile keeps its reference.
        let error = proxies
            .delete(proxy.id)
            .expect_err("a proxy a profile uses cannot be deleted");
        assert!(
            matches!(&error, StorageError::Conflict(reason) if reason.contains("Work laptop")),
            "{error}"
        );
        assert_eq!(
            profiles
                .get(work.id)
                .expect("get the profile")
                .expect("the profile is there")
                .proxy_id,
            Some(proxy.id),
            "a refused delete must not clear the reference on its way out"
        );
        let error = cores
            .delete(core.id)
            .expect_err("a core a profile launches with cannot be deleted");
        assert!(matches!(error, StorageError::Conflict(_)), "{error}");

        // And once nothing names them, both are deletable - the rule is about the
        // reference, not about the record.
        profiles.delete(work.id).expect("delete the profile");
        proxies.delete(proxy.id).expect("the proxy is free now");
        cores.delete(core.id).expect("and so is the core");
        assert!(cores.list().expect("cores").is_empty());
        assert!(proxies.list().expect("proxies").is_empty());
    }

    #[test]
    fn the_database_keeps_the_repository_contract() {
        let storage = SqliteStorage::in_memory().expect("init in-memory sqlite");
        let (cores, proxies, profiles) = (storage.cores(), storage.proxies(), storage.profiles());
        a_repository_contract(&cores, &proxies, &profiles);
    }

    /// The same body, against the backend the application's tests use - wired the
    /// way a configuration wires it, since without that this backend has no
    /// reference rules at all.
    #[test]
    fn the_memory_backend_keeps_the_repository_contract() {
        let cores = Arc::new(MemCoreRepository::new());
        let proxies = Arc::new(MemProxyRepository::new());
        let profiles = Arc::new(MemProfileRepository::new());
        let _configuration = MemConfiguration::new(
            Arc::clone(&cores),
            Arc::clone(&proxies),
            Arc::clone(&profiles),
        );
        a_repository_contract(cores.as_ref(), proxies.as_ref(), profiles.as_ref());
    }

    #[test]
    fn the_database_replaces_the_whole_configuration() {
        let storage = SqliteStorage::in_memory().expect("init in-memory sqlite");
        let (cores, proxies, profiles) = (storage.cores(), storage.proxies(), storage.profiles());
        replaces_the_whole_configuration(&storage, &cores, &proxies, &profiles);
    }

    #[test]
    fn the_memory_backend_replaces_the_whole_configuration() {
        let cores = Arc::new(MemCoreRepository::new());
        let proxies = Arc::new(MemProxyRepository::new());
        let profiles = Arc::new(MemProfileRepository::new());
        let configuration = MemConfiguration::new(
            Arc::clone(&cores),
            Arc::clone(&proxies),
            Arc::clone(&profiles),
        );
        replaces_the_whole_configuration(&configuration, &*cores, &*proxies, &*profiles);
    }
}
