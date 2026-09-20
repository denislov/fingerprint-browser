//! Making an installation be a configuration backup.
//!
//! Restore and import are not the same verb, and they must not share one rule.
//! Import means "add these to what I have" and never overwrites; restore means
//! "be this file" and is allowed to replace. That difference is the whole of this
//! module: it is [`crate::import`] plus a precondition and the removal of what is
//! already here.
//!
//! The precondition is a question about the *installation*, not about the file.
//! A restore onto an empty installation needs no confirmation, because there is
//! nothing it could destroy; a restore onto a populated one replaces live rows
//! with the file's, so it refuses unless the caller has said out loud that that
//! is what it meant ([`RestoreMode::Replace`]).
//!
//! The removal is ordered the way the writes are, only backwards: profiles
//! first, then proxies, then cores, because a profile is what references a core
//! and a proxy. Deleting in that order is also what keeps the services' own
//! rules - a proxy still assigned cannot be deleted - from refusing the
//! removal: by the time a proxy is reached, no profile names it.
//!
//! Browser data is not touched here. Deleting a profile keeps its directory on
//! disk, so a restart of the same identifier finds its sessions where they were,
//! which is what makes a restored configuration line up with a browser-data
//! copy. See [`crate::browser_data`].

use crate::config_backup::{ConfigBackup, ConfigSnapshot};
use crate::core_service::CoreService;
use crate::error::AppError;
use crate::import::{Counts, ImportNotes, ImportPlan, ImportReport, apply_import, plan_import};
use crate::profile_service::{DeleteMode, ProfileService};
use crate::proxy_service::ProxyService;
use thiserror::Error;

/// Whether a restore may replace a populated installation.
///
/// The two are a deliberate choice at the call site rather than a flag read from
/// the file: the file cannot know what it is about to overwrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreMode {
    /// Refuse unless the installation holds no cores, proxies or profiles.
    ///
    /// This is the safe default, and the one the first restore of a fresh
    /// installation takes: there is nothing to lose, so nothing is asked.
    OnlyWhenEmpty,
    /// Replace whatever is here with the file's configuration, after the caller
    /// has confirmed it.
    Replace,
}

/// What a restore did, in the terms a user has to hear it in.
///
/// Counts and notes are the import's, because the writing half *is* an import;
/// what restore adds is [`RestoreReport::removed`], so the sentence can say what
/// was replaced rather than only what arrived.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestoreReport {
    /// What landed, by kind.
    pub added: Counts,
    /// What was taken away to make room, by kind. Zero for an empty install.
    pub removed: Counts,
    /// What the plan decided not to do.
    pub notes: ImportNotes,
    /// Records the database refused, named, removals and additions together.
    pub failed: Vec<String>,
}

impl RestoreReport {
    /// Whether the outcome is worse than the file asked for.
    pub fn needs_attention(&self) -> bool {
        !self.failed.is_empty() || self.notes.needs_attention()
    }
}

/// Why a restore could not be started.
///
/// The precondition is a variant rather than a boolean because the message it
/// carries is the sentence the window shows, and it has to name how much is in
/// the way.
#[derive(Debug, Error)]
pub enum RestoreError {
    /// The precondition, with the counts the window needs to say what is in the
    /// way. The sentence itself is built where it is shown, because it has to
    /// name how much is being replaced and be readable in more than one
    /// language.
    #[error("this installation already holds a configuration")]
    NotEmpty { present: Counts },
}

/// Decides what restoring `document` would do to an installation holding
/// `present`.
///
/// The plan is always made against an *empty* snapshot, whether or not `present`
/// is: restore replaces, so every record in the file is arriving and the file's
/// copy of an identifier wins over a stored one. That is the difference from
/// [`plan_import`], which would keep a stored record and never overwrite it.
///
/// `OnlyWhenEmpty` turns the precondition into this function's first line rather
/// than leaving it to the caller, so a mistaken restore cannot get as far as a
/// writer.
pub fn plan_restore(
    document: &ConfigBackup,
    present: &ConfigSnapshot,
    data_dir: &std::path::Path,
    mode: RestoreMode,
) -> Result<ImportPlan, RestoreError> {
    if mode == RestoreMode::OnlyWhenEmpty && !present.is_empty() {
        return Err(RestoreError::NotEmpty {
            present: counts_of(present),
        });
    }

    // Empty on purpose: the plan describes a file becoming the configuration,
    // and nothing about what is being replaced.
    Ok(plan_import(document, &ConfigSnapshot::default(), data_dir))
}

/// Removes what is present, then writes what the plan asks for.
///
/// The removal is ordered profiles, proxies, cores - the reverse of the write
/// order - so that the referential rules the services enforce never refuse it.
/// A removal the database refuses does not stop the restore: it is recorded, and
/// the write phase still runs, exactly as an item the database refuses during an
/// import does not stop the rest.
///
/// Fails only when the removal cannot be attempted at all or the write phase
/// cannot read the database; what landed is counted from the writes.
pub fn apply_restore(
    plan: ImportPlan,
    present: &ConfigSnapshot,
    cores: &dyn CoreService,
    proxies: &dyn ProxyService,
    profiles: &dyn ProfileService,
) -> Result<RestoreReport, AppError> {
    let mut report = RestoreReport::default();

    for profile in &present.profiles {
        match profiles.delete(profile.id, DeleteMode::KeepUserData) {
            Ok(()) => report.removed.profiles += 1,
            Err(error) => report
                .failed
                .push(format!("profile {}: {error}", profile.name)),
        }
    }
    for proxy in &present.proxies {
        match proxies.delete(proxy.id) {
            Ok(()) => report.removed.proxies += 1,
            Err(error) => report.failed.push(format!("proxy {}: {error}", proxy.name)),
        }
    }
    for core in &present.cores {
        match cores.delete(core.id) {
            Ok(()) => report.removed.cores += 1,
            Err(error) => report.failed.push(format!("core {}: {error}", core.name)),
        }
    }

    let ImportReport {
        added,
        notes,
        failed,
    } = apply_import(plan, cores, proxies, profiles)?;

    report.added = added;
    report.notes = notes;
    report.failed.extend(failed);
    Ok(report)
}

/// How many of each kind a snapshot holds.
fn counts_of(snapshot: &ConfigSnapshot) -> Counts {
    Counts {
        cores: snapshot.cores.len(),
        proxies: snapshot.proxies.len(),
        profiles: snapshot.profiles.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_backup::{Credentials, ExportOrigin};
    use crate::{
        DefaultCoreService, DefaultProfileService, DefaultProxyService, read_config_backup,
        read_configuration, write_config_backup,
    };
    use domain::{
        BrowserCore, BrowserProfile, CoreId, FingerprintProfile, ProfileId, ProxyId, ProxyOutbound,
        ProxyProfile, Socks5Outbound, StartTarget, WindowProfile,
    };
    use std::path::PathBuf;
    use std::sync::Arc;
    use storage::SqliteStorage;

    const SECRET: &str = "correct horse battery staple";

    /// A directory of its own under the system temporary directory, removed when
    /// the test ends.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-restore-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Self { dir }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    struct Fixture {
        scratch: Scratch,
        cores: DefaultCoreService,
        proxies: DefaultProxyService,
        profiles: DefaultProfileService,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let scratch = Scratch::new(name);
            let storage = SqliteStorage::in_memory().expect("sqlite in memory");
            let profile_repo: Arc<dyn storage::ProfileRepository> = Arc::new(storage.profiles());
            let core_repo: Arc<dyn storage::CoreRepository> = Arc::new(storage.cores());
            let proxy_repo: Arc<dyn storage::ProxyRepository> = Arc::new(storage.proxies());

            Self {
                cores: DefaultCoreService::new(core_repo, Arc::clone(&profile_repo)),
                proxies: DefaultProxyService::new(proxy_repo, Arc::clone(&profile_repo)),
                profiles: DefaultProfileService::new(
                    Arc::clone(&profile_repo),
                    scratch.dir.clone(),
                ),
                scratch,
            }
        }

        fn present(&self) -> ConfigSnapshot {
            read_configuration(&self.profiles, &self.cores, &self.proxies).expect("read")
        }

        fn restore(&self, document: &ConfigBackup, mode: RestoreMode) -> RestoreReport {
            let present = self.present();
            let plan = plan_restore(document, &present, &self.scratch.dir, mode).expect("plan");
            apply_restore(plan, &present, &self.cores, &self.proxies, &self.profiles)
                .expect("apply")
        }
    }

    fn core(name: &str) -> BrowserCore {
        BrowserCore {
            id: CoreId::new(),
            name: name.to_string(),
            executable: PathBuf::from("/opt/chromium-148/chrome"),
            version: "148.0.7778.215".to_string(),
            major: 148,
        }
    }

    fn secret_proxy(name: &str) -> ProxyProfile {
        ProxyProfile {
            id: ProxyId::new(),
            name: name.to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "203.0.113.10".to_string(),
                port: 1080,
                username: Some("alice".to_string()),
                password: Some(SECRET.to_string()),
            }),
        }
    }

    fn profile(name: &str, core_id: CoreId, proxy_id: Option<ProxyId>) -> BrowserProfile {
        BrowserProfile {
            id: ProfileId::new(),
            name: name.to_string(),
            core_id,
            user_data_dir: PathBuf::from("/home/alice/.local/share/FpBrowser/profiles/one"),
            fingerprint: FingerprintProfile::new_random(4242),
            proxy_id,
            window: WindowProfile::new(1440, 900),
            start_target: StartTarget::Blank,
        }
    }

    fn document(snapshot: ConfigSnapshot) -> ConfigBackup {
        ConfigBackup::build(
            snapshot,
            Credentials::Included,
            ExportOrigin {
                exported_at: "2026-09-20T16:04:00.000Z".to_string(),
                source_data_dir: "/home/alice/.local/share/FpBrowser".to_string(),
            },
        )
    }

    /// A file holding one core, one proxy and one profile that uses both.
    fn whole() -> (ConfigBackup, BrowserCore, ProxyProfile, BrowserProfile) {
        let core = core("Fingerprint Chromium 148");
        let proxy = secret_proxy("Zurich exit");
        let profile = profile("Work laptop", core.id, Some(proxy.id));
        let document = document(ConfigSnapshot {
            cores: vec![core.clone()],
            proxies: vec![proxy.clone()],
            profiles: vec![profile.clone()],
        });
        (document, core, proxy, profile)
    }

    #[test]
    fn a_restore_onto_an_empty_installation_adds_everything_and_removes_nothing() {
        let fixture = Fixture::new("empty");
        let (document, _, _, _) = whole();

        let report = fixture.restore(&document, RestoreMode::OnlyWhenEmpty);

        assert_eq!(report.added.total(), 3);
        assert_eq!(report.removed.total(), 0, "nothing was there to remove");
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(!report.needs_attention());
    }

    /// The precondition, which is the whole difference from an import: a
    /// populated installation is never replaced without being asked.
    #[test]
    fn a_restore_onto_a_populated_installation_is_refused_and_changes_nothing() {
        let fixture = Fixture::new("populated");
        let (document, _, _, _) = whole();
        fixture.restore(&document, RestoreMode::OnlyWhenEmpty);
        let before = fixture.present();

        let mut other = document.clone();
        other.profiles.clear();
        other.proxies.clear();
        other.cores = vec![core("Another")];

        let error = plan_restore(
            &other,
            &fixture.present(),
            &fixture.scratch.dir,
            RestoreMode::OnlyWhenEmpty,
        )
        .expect_err("a populated installation cannot be quietly replaced");

        let RestoreError::NotEmpty { present } = error;
        assert_eq!(present.cores, 1);
        assert_eq!(present.proxies, 1);
        assert_eq!(present.profiles, 1);
        assert_eq!(fixture.present(), before, "nothing was written");
    }

    /// Restore means "be this file": a record that is here and not in the file
    /// is taken away, which import would never do.
    #[test]
    fn a_confirmed_restore_replaces_the_configuration_with_the_file() {
        let fixture = Fixture::new("replaces");
        let (document, _, _, _) = whole();

        // A second core and a profile of its own, none of which the file names.
        let extra_core = core("Extra");
        fixture.cores.insert(extra_core.clone()).expect("seed core");
        fixture
            .profiles
            .insert(profile("Extra profile", extra_core.id, None))
            .expect("seed profile");
        fixture.restore(&document, RestoreMode::Replace);

        // The file wins: the extra core and profile are gone, and the file's
        // three records are here.
        let after = fixture.present();
        assert_eq!(after.cores.len(), 1, "{after:?}");
        assert_eq!(after.cores[0].id, document.cores[0].id);
        assert_eq!(after.proxies.len(), 1);
        assert_eq!(after.profiles.len(), 1);
        assert_eq!(after.profiles[0].name, "Work laptop");

        let report = fixture.restore(&document, RestoreMode::Replace);
        assert_eq!(report.removed.total(), 3, "the file's own records replaced");
        assert_eq!(report.added.total(), 3);
    }

    /// The file's copy of an identifier wins over the stored one, unlike import,
    /// which keeps the stored record untouched.
    #[test]
    fn a_restore_overwrites_a_record_whose_identifier_is_already_taken() {
        let fixture = Fixture::new("overwrites");
        let (document, core, _, _) = whole();
        let mut renamed = core.clone();
        renamed.name = "Renamed".to_string();
        fixture
            .cores
            .insert(renamed)
            .expect("seed the same identifier");

        let report = fixture.restore(&document, RestoreMode::Replace);

        assert!(report.failed.is_empty(), "{:?}", report.failed);
        let stored = fixture.present().cores;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, core.name, "the file's copy won");
    }

    /// A restored profile keeps its identifier, so the directory named after it
    /// lines up with a browser-data copy - and a directory that is not on this
    /// machine is re-pointed exactly as an import does.
    #[test]
    fn a_restored_profile_keeps_its_identifier_and_repoints_its_directory() {
        let fixture = Fixture::new("identity");
        let (document, _, _, profile) = whole();

        let report = fixture.restore(&document, RestoreMode::OnlyWhenEmpty);

        assert_eq!(report.notes.repointed.len(), 1);
        let stored = fixture.present().profiles;
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, profile.id, "the identifier is kept");
        assert_eq!(
            stored[0].user_data_dir,
            crate::default_user_data_dir(&fixture.scratch.dir, profile.id)
        );
    }

    #[test]
    fn a_restore_keeps_the_credentials_the_file_carried() {
        let fixture = Fixture::new("credentials");
        let (document, _, _, _) = whole();

        fixture.restore(&document, RestoreMode::Replace);

        let stored = fixture.present().proxies;
        assert!(stored[0].outbound.carries_credentials());
    }

    #[test]
    fn a_restore_reads_the_file_from_the_path_it_was_written_to() {
        let fixture = Fixture::new("round-trip");
        let path = fixture.scratch.dir.join("config.json");
        let (document, _, _, _) = whole();
        write_config_backup(
            ConfigSnapshot {
                cores: document.cores.clone(),
                proxies: document.proxies.clone(),
                profiles: document.profiles.clone(),
            },
            Credentials::Included,
            ExportOrigin {
                exported_at: document.exported_at.clone(),
                source_data_dir: document.source_data_dir.clone(),
            },
            &path,
        )
        .expect("export");

        let read = read_config_backup(&path).expect("read the file back");
        let report = fixture.restore(&read, RestoreMode::OnlyWhenEmpty);

        assert_eq!(report.added.total(), 3);
    }

    /// An empty installation is `is_empty`, which is what lets restore skip the
    /// confirmation and an import report it as free.
    #[test]
    fn an_empty_snapshot_is_empty() {
        assert!(ConfigSnapshot::default().is_empty());
        let (_, core, _, _) = whole();
        assert!(
            !ConfigSnapshot {
                cores: vec![core],
                ..Default::default()
            }
            .is_empty()
        );
    }
}
