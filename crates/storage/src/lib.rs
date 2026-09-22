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
    use domain::{
        BrowserBrand, BrowserCore, BrowserProfile, CoreId, FingerprintProfile, HttpOutbound,
        Platform, ProfileId, ProxyId, ProxyOutbound, ProxyProfile, ShadowsocksOutbound,
        Socks5Outbound, SpoofingFeature, StartTarget, StreamSettings, TrojanOutbound,
        VlessOutbound, VmessOutbound, WebRtcPolicy, WindowProfile,
    };
    use std::path::PathBuf;

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
}
