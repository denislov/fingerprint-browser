//! Taking a configuration backup: gathering what a configuration is made of,
//! and writing it to a path the caller chose.
//!
//! The document itself is [`crate::config_backup`]'s business. This module is the
//! two steps around it - reading the three lists out of their services, and
//! putting the result on disk - and it reports what it did in the terms a user
//! has to hear it in, because "credentials were left out" and "there were none to
//! leave out" are different sentences and only one of them is reassuring.

use crate::config_backup::{BackupError, ConfigBackup, ConfigSnapshot, Credentials, ExportOrigin};
use crate::core_service::CoreService;
use crate::error::AppError;
use crate::profile_service::ProfileService;
use crate::proxy_service::ProxyService;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Gathers the three lists a configuration is made of.
///
/// The three reads are not one transaction, so a profile created between them
/// could name a core the snapshot does not carry. The import rule - a profile is
/// imported only when its core is present - already covers that case, which is
/// why nothing here tries to close the window: an export is a snapshot of a
/// moment, and saying so is better than pretending otherwise.
pub fn read_configuration(
    profiles: &dyn ProfileService,
    cores: &dyn CoreService,
    proxies: &dyn ProxyService,
) -> Result<ConfigSnapshot, AppError> {
    Ok(ConfigSnapshot {
        cores: cores.list()?,
        proxies: proxies.list()?,
        profiles: profiles.list()?,
    })
}

/// What an export did, in the terms a user has to hear it in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    /// Where the file was written.
    pub path: PathBuf,
    /// Which choice was applied, so the caller can say so without re-deriving it.
    pub credentials: Credentials,
    pub cores: usize,
    pub proxies: usize,
    pub profiles: usize,
    /// How many proxies held something the file does not now carry.
    ///
    /// Zero for [`Credentials::Included`] by definition, and zero for an
    /// [`Credentials::Excluded`] export that had nothing to leave out. The two
    /// mean opposite things, which is why they are not collapsed into "no
    /// credentials were written".
    pub credentials_removed: usize,
    /// The size of the file, newline included.
    pub bytes: usize,
}

/// Writes the configuration backup for `snapshot` to `destination`.
///
/// An existing file at `destination` is written over rather than refused: the
/// default destination carries a timestamp, so the ordinary way to reach an
/// existing path is to have typed it on purpose - and refusing would break
/// "update the backup I keep at this path", which is the other ordinary way.
///
/// The document is serialised before the filesystem is touched, so a snapshot
/// that cannot be encoded does not leave an empty directory behind.
pub fn write_config_backup(
    snapshot: ConfigSnapshot,
    credentials: Credentials,
    origin: ExportOrigin,
    destination: &Path,
) -> Result<ExportReport, ExportError> {
    let credentials_removed = match credentials {
        Credentials::Included => 0,
        Credentials::Excluded => snapshot.proxies_carrying_credentials(),
    };

    let document = ConfigBackup::build(snapshot, credentials, origin);
    let contents = format!("{}\n", document.to_json()?);

    if let Some(parent) = destination.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| ExportError::Directory {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    std::fs::write(destination, &contents).map_err(|source| ExportError::Write {
        path: destination.to_path_buf(),
        source,
    })?;

    Ok(ExportReport {
        path: destination.to_path_buf(),
        credentials,
        cores: document.cores.len(),
        proxies: document.proxies.len(),
        profiles: document.profiles.len(),
        credentials_removed,
        bytes: contents.len(),
    })
}

/// Why a configuration backup could not be written.
///
/// The path is carried on both filesystem variants because the caller's next step
/// differs: a directory that could not be created is a different problem from a
/// file that could not be written, and neither is actionable without knowing
/// which path it was.
#[derive(Debug, Error)]
pub enum ExportError {
    #[error(transparent)]
    Document(#[from] BackupError),

    #[error("could not create {path}: {source}")]
    Directory { path: PathBuf, source: io::Error },

    #[error("could not write {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConfigSnapshot, DefaultCoreService, DefaultProfileService, DefaultProxyService, NewProfile,
        NewProxy,
    };
    use domain::{
        BrowserCore, CoreId, FingerprintProfile, ProxyOutbound, ProxyProfile, Socks5Outbound,
        StartTarget, WindowProfile,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use storage::CoreRepository as _;

    const SECRET: &str = "correct horse battery staple";

    /// A directory of its own under the system temporary directory, removed when
    /// the test ends.
    ///
    /// A test must never write into a real data directory, and an export test has
    /// to write somewhere, so it writes here.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-export-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self { dir }
        }

        fn join(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn origin() -> ExportOrigin {
        ExportOrigin {
            exported_at: "2026-09-20T16:04:00.000Z".to_string(),
            source_data_dir: "/home/alice/.local/share/FpBrowser".to_string(),
        }
    }

    /// A configuration with one core, one proxy holding a password, and one
    /// profile using both.
    fn populated() -> (ConfigSnapshot, BrowserCore, ProxyProfile) {
        let core = BrowserCore {
            id: CoreId::new(),
            name: "Fingerprint Chromium 148".to_string(),
            executable: PathBuf::from("/opt/chromium-148/chrome"),
            version: "148.0.7778.215".to_string(),
            major: 148,
        };
        let proxy = ProxyProfile {
            id: domain::ProxyId::new(),
            name: "Zurich exit".to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "203.0.113.10".to_string(),
                port: 1080,
                username: Some("alice".to_string()),
                password: Some(SECRET.to_string()),
            }),
        };
        let profile = domain::BrowserProfile {
            id: domain::ProfileId::new(),
            name: "Work laptop".to_string(),
            core_id: core.id,
            user_data_dir: PathBuf::from("/home/alice/.local/share/FpBrowser/profiles/one"),
            fingerprint: FingerprintProfile::new_random(4242),
            proxy_id: Some(proxy.id),
            window: WindowProfile::new(1440, 900),
            start_target: StartTarget::Blank,
        };
        (
            ConfigSnapshot {
                cores: vec![core.clone()],
                proxies: vec![proxy.clone()],
                profiles: vec![profile],
            },
            core,
            proxy,
        )
    }

    #[test]
    fn an_export_writes_the_document_to_the_path_it_was_given() {
        let scratch = Scratch::new("writes");
        let path = scratch.join("config.json");
        let (snapshot, _, _) = populated();

        let report = write_config_backup(snapshot.clone(), Credentials::Included, origin(), &path)
            .expect("export");

        assert_eq!(report.path, path);
        let text = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(
            ConfigBackup::from_json(&text).expect("parse"),
            ConfigBackup::build(snapshot, Credentials::Included, origin())
        );
    }

    #[test]
    fn an_export_creates_the_directory_it_was_given() {
        // The default destination is a directory the program has not written to
        // before, so creating it is part of the job rather than the caller's.
        let scratch = Scratch::new("creates");
        let path = scratch.join("nested/deeper/config.json");
        let (snapshot, _, _) = populated();

        write_config_backup(snapshot, Credentials::Included, origin(), &path).expect("export");

        assert!(path.exists(), "{} should exist", path.display());
    }

    #[test]
    fn an_export_reports_what_it_wrote() {
        let scratch = Scratch::new("reports");
        let path = scratch.join("config.json");
        let (snapshot, _, _) = populated();

        let report =
            write_config_backup(snapshot, Credentials::Included, origin(), &path).expect("export");

        assert_eq!(report.cores, 1);
        assert_eq!(report.proxies, 1);
        assert_eq!(report.profiles, 1);
        assert_eq!(
            report.bytes,
            std::fs::metadata(&path).expect("metadata").len() as usize
        );
    }

    #[test]
    fn an_export_that_left_credentials_out_says_how_many_it_left_out() {
        let scratch = Scratch::new("excluded");
        let path = scratch.join("config.json");
        let (snapshot, _, _) = populated();

        let report =
            write_config_backup(snapshot, Credentials::Excluded, origin(), &path).expect("export");

        assert_eq!(report.credentials, Credentials::Excluded);
        assert_eq!(report.credentials_removed, 1);

        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(
            !text.contains(SECRET),
            "an excluded export still wrote the password: {text}"
        );
        // Stripped, not dropped: the proxy is there to be pointed at again.
        let document = ConfigBackup::from_json(&text).expect("parse");
        assert_eq!(document.proxies.len(), 1);
        assert_eq!(document.proxies[0].outbound.host(), "203.0.113.10");
    }

    #[test]
    fn an_export_with_nothing_to_leave_out_reports_none() {
        // The opposite sentence to the test above, and the reason the count is
        // reported rather than inferred from the choice: excluding credentials
        // from a configuration that had none must not read as "some were
        // removed".
        let scratch = Scratch::new("nothing-to-strip");
        let path = scratch.join("config.json");
        let (mut snapshot, _, _) = populated();
        snapshot.proxies[0].outbound = ProxyOutbound::Socks5(Socks5Outbound {
            host: "10.0.0.1".to_string(),
            port: 1080,
            username: None,
            password: None,
        });

        let report =
            write_config_backup(snapshot, Credentials::Excluded, origin(), &path).expect("export");

        assert_eq!(report.credentials, Credentials::Excluded);
        assert_eq!(report.credentials_removed, 0);
    }

    #[test]
    fn an_empty_configuration_still_writes_a_readable_file() {
        // Someone with no profiles yet can still take a backup, and what they get
        // has to be a document rather than an empty or unreadable file.
        let scratch = Scratch::new("empty");
        let path = scratch.join("config.json");

        let report = write_config_backup(
            ConfigSnapshot::default(),
            Credentials::Excluded,
            origin(),
            &path,
        )
        .expect("export");

        assert_eq!(report.profiles, 0);
        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(ConfigBackup::from_json(&text).is_ok(), "{text}");
    }

    #[test]
    fn a_destination_that_cannot_be_created_is_reported_with_its_path() {
        // A parent that exists as a file rather than a directory: the second
        // export to the same mistyped location has to say which path failed.
        let scratch = Scratch::new("blocked-parent");
        let blocker = scratch.join("not-a-directory");
        std::fs::write(&blocker, b"").expect("write the blocker");

        let error = write_config_backup(
            ConfigSnapshot::default(),
            Credentials::Included,
            origin(),
            &blocker.join("config.json"),
        )
        .expect_err("a file cannot hold a directory");

        match error {
            ExportError::Directory { path, .. } => assert_eq!(path, blocker),
            other => panic!("expected a directory failure, got {other}"),
        }
    }

    #[test]
    fn a_destination_that_is_a_directory_is_reported_with_its_path() {
        let scratch = Scratch::new("blocked-file");
        let error = write_config_backup(
            ConfigSnapshot::default(),
            Credentials::Included,
            origin(),
            &scratch.dir,
        )
        .expect_err("a directory is not a file");

        match error {
            ExportError::Write { path, .. } => assert_eq!(path, scratch.dir),
            other => panic!("expected a write failure, got {other}"),
        }
    }

    #[test]
    fn an_export_writes_over_the_file_that_is_already_there() {
        // Deliberate, and the reason is in the function's documentation: refusing
        // would break updating a backup kept at one path, and the default path
        // carries a timestamp so it never lands on an old one by accident.
        let scratch = Scratch::new("overwrite");
        let path = scratch.join("config.json");
        std::fs::write(&path, b"a previous backup").expect("seed");
        let (snapshot, _, _) = populated();

        write_config_backup(snapshot, Credentials::Included, origin(), &path).expect("export");

        let text = std::fs::read_to_string(&path).expect("read back");
        assert!(!text.contains("a previous backup"), "{text}");
        assert!(ConfigBackup::from_json(&text).is_ok());
    }

    #[test]
    fn reading_the_configuration_gathers_all_three_lists() {
        let storage = storage::SqliteStorage::in_memory().expect("sqlite in memory");
        let profile_repo = Arc::new(storage.profiles());
        let core_repo = Arc::new(storage.cores());
        let proxy_repo = Arc::new(storage.proxies());

        let core = BrowserCore {
            id: CoreId::new(),
            name: "Core 148".to_string(),
            executable: PathBuf::from("/opt/chromium-148/chrome"),
            version: "148.0.7778.215".to_string(),
            major: 148,
        };
        core_repo.save(&core).expect("save core");

        let cores = DefaultCoreService::new(
            Arc::clone(&core_repo) as Arc<dyn storage::CoreRepository>,
            Arc::clone(&profile_repo) as Arc<dyn storage::ProfileRepository>,
        );
        let proxies = DefaultProxyService::new(
            Arc::clone(&proxy_repo) as Arc<dyn storage::ProxyRepository>,
            Arc::clone(&profile_repo) as Arc<dyn storage::ProfileRepository>,
        );
        let profiles = DefaultProfileService::new(
            Arc::clone(&profile_repo) as Arc<dyn storage::ProfileRepository>,
            PathBuf::from("data"),
        );

        let proxy = proxies
            .create(NewProxy {
                name: "Zurich exit".to_string(),
                outbound: ProxyOutbound::Socks5(Socks5Outbound {
                    host: "203.0.113.10".to_string(),
                    port: 1080,
                    username: None,
                    password: None,
                }),
            })
            .expect("create proxy");
        profiles
            .create(NewProfile {
                name: "Work laptop".to_string(),
                core_id: core.id,
                user_data_dir: None,
                fingerprint: None,
                proxy_id: Some(proxy.id),
                window: None,
                start_target: None,
            })
            .expect("create profile");

        let snapshot = read_configuration(&profiles, &cores, &proxies).expect("read");

        // All three, and the profile's references line up with what travelled
        // with it - that is the whole point of exporting them together.
        assert_eq!(snapshot.cores.len(), 1);
        assert_eq!(snapshot.proxies.len(), 1);
        assert_eq!(snapshot.profiles.len(), 1);
        assert_eq!(snapshot.profiles[0].core_id, snapshot.cores[0].id);
        assert_eq!(snapshot.profiles[0].proxy_id, Some(snapshot.proxies[0].id));
    }
}
