//! Making an installation be a configuration backup.
//!
//! Restore and import are not the same verb, and they must not share one rule.
//! Import means "add these to what I have" and never overwrites; restore means
//! "be this file" and is allowed to replace. That difference is the whole of this
//! module, and it is why restore does not use [`crate::import::plan_import`] or
//! [`crate::import::apply_import`]: an import that writes most of a file and
//! reports the rest is a good import, and a restore that does the same is an
//! installation that is neither what it was nor what the file asked for.
//!
//! So restore is strict in both halves. Every record in the file is validated
//! before anything is removed - the same rules a form is held to, plus the two
//! rules only a file can break, a repeated identifier and a reference to a record
//! the file does not hold - and the replacement itself is one transaction, so a
//! refused write leaves the old configuration exactly where it was.
//!
//! The precondition is a question about the *installation*, not about the file.
//! A restore onto an empty installation needs no confirmation, because there is
//! nothing it could destroy; a restore onto a populated one replaces live rows
//! with the file's, so it refuses unless the caller has said out loud that that
//! is what it meant ([`RestoreMode::Replace`]).
//!
//! Browser data is not touched here. Deleting a profile keeps its directory on
//! disk, so a restart of the same identifier finds its sessions where they were,
//! which is what makes a restored configuration line up with a browser-data
//! copy. See [`crate::browser_data`].

use crate::config_backup::{ConfigBackup, ConfigSnapshot, Credentials};
use crate::error::AppError;
use crate::import::{Counts, ImportNotes, Repointed};
use crate::profile_service::default_user_data_dir;
use domain::{
    BrowserCore, BrowserProfile, CoreId, ProxyId, ProxyProfile, validate_core, validate_profile,
    validate_proxy,
};
use std::collections::HashSet;
use storage::ConfigurationRepository;
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

/// What a restore will write, decided before anything is written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestorePlan {
    /// The file's cores, as they will be stored.
    pub cores: Vec<BrowserCore>,
    /// The file's proxies, as they will be stored.
    pub proxies: Vec<ProxyProfile>,
    /// The file's profiles, as they will be stored: the directory a profile names
    /// is resolved for this machine, exactly as an import resolves it.
    pub profiles: Vec<BrowserProfile>,
    /// What was decided on the way, in the same terms an import reports.
    pub notes: ImportNotes,
}

impl RestorePlan {
    /// How many of each kind it will write.
    pub fn counts(&self) -> Counts {
        Counts {
            cores: self.cores.len(),
            proxies: self.proxies.len(),
            profiles: self.profiles.len(),
        }
    }
}

/// What a restore did, in the terms a user has to hear it in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RestoreReport {
    /// What landed, by kind.
    pub added: Counts,
    /// What was taken away to make room, by kind. Zero for an empty install.
    pub removed: Counts,
    /// What the plan decided not to do.
    pub notes: ImportNotes,
    /// Records the database refused, by name.
    ///
    /// Always empty: a refusal is now an error that rolls the whole replacement
    /// back, so there is no such thing as a restore that half happened. The field
    /// stays because the sentence the window shows is built from it, and "nothing
    /// was refused" is what that sentence has to be able to say.
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
/// the way. The rest are the file's own faults, and they are variants for the
/// same reason: each of them tells the reader to do something different.
#[derive(Debug, Error)]
pub enum RestoreError {
    /// The precondition, with the counts the window needs to say what is in the
    /// way. The sentence itself is built where it is shown, because it has to
    /// name how much is being replaced and be readable in more than one
    /// language.
    #[error("this installation already holds a configuration")]
    NotEmpty { present: Counts },

    /// Two records of one kind share an identifier.
    ///
    /// Whichever of them was written second would be the one that survived, and
    /// nothing in the file says which of the two that is meant to be.
    #[error("the file holds two {kind}s with the same identifier")]
    Duplicate { kind: &'static str },

    /// A record the program would refuse if a form had produced it.
    #[error("{kind} {name:?} cannot be restored: {reason}")]
    Invalid {
        kind: &'static str,
        name: String,
        reason: String,
    },

    /// A profile names a core or a proxy the file does not hold.
    ///
    /// Restore replaces the whole configuration, so there is nothing else the
    /// name could resolve to - unlike an import, where the record may be one the
    /// installation already has. A profile stored without its engine is a profile
    /// that cannot be started, and clearing the reference silently would store a
    /// different configuration than the file describes.
    #[error("profile {name:?} names a {kind} the file does not hold")]
    Dangling { name: String, kind: &'static str },

    /// The file was exported without the credentials its proxies need.
    ///
    /// Its own message because the reader's next move is a different export, not
    /// a repair: the records are not damaged, they are missing the one field that
    /// makes them usable.
    #[error("{count} proxies in this file cannot be restored: it was exported without credentials")]
    CredentialsExcluded { count: usize },
}

/// Decides what restoring `document` would do to an installation holding
/// `present`, and refuses the whole file if any part of it could not be stored.
///
/// Always made against an *empty* snapshot: restore replaces, so every record in
/// the file is arriving and the file's copy of an identifier wins over a stored
/// one. `present` is asked for the precondition only.
///
/// Every check here is one that would otherwise be made by the database, in the
/// order the database would make it - after the old configuration had already
/// been deleted. Names and reasons are collected rather than short-circuited
/// where a person would want to fix more than one thing, but the answer is still
/// a refusal of the file as a whole.
pub fn plan_restore(
    document: &ConfigBackup,
    present: &ConfigSnapshot,
    data_dir: &std::path::Path,
    mode: RestoreMode,
) -> Result<RestorePlan, RestoreError> {
    if mode == RestoreMode::OnlyWhenEmpty && !present.is_empty() {
        return Err(RestoreError::NotEmpty {
            present: counts_of(present),
        });
    }

    if repeats(document.cores.iter().map(|core| core.id)) {
        return Err(RestoreError::Duplicate { kind: "core" });
    }
    if repeats(document.proxies.iter().map(|proxy| proxy.id)) {
        return Err(RestoreError::Duplicate { kind: "proxy" });
    }
    if repeats(document.profiles.iter().map(|profile| profile.id)) {
        return Err(RestoreError::Duplicate { kind: "profile" });
    }

    for core in &document.cores {
        validate_core(core).map_err(|error| RestoreError::Invalid {
            kind: "core",
            name: core.name.clone(),
            reason: error.to_string(),
        })?;
    }

    // The proxies first, and as a group: a backup exported without credentials
    // leaves the fields that hold them empty, which fails the same validation a
    // hand-edited file fails. The difference is what the reader has to do about
    // it, so it is a different answer.
    let mut unusable: Vec<(String, String)> = Vec::new();
    for proxy in &document.proxies {
        if let Err(error) = validate_proxy(proxy) {
            unusable.push((proxy.name.clone(), error.to_string()));
        }
    }
    if !unusable.is_empty() {
        if document.credentials == Credentials::Excluded {
            return Err(RestoreError::CredentialsExcluded {
                count: unusable.len(),
            });
        }
        let (name, reason) = unusable.remove(0);
        return Err(RestoreError::Invalid {
            kind: "proxy",
            name,
            reason,
        });
    }

    let held_cores: HashSet<CoreId> = document.cores.iter().map(|core| core.id).collect();
    let held_proxies: HashSet<ProxyId> = document.proxies.iter().map(|proxy| proxy.id).collect();

    let mut notes = ImportNotes {
        credentials_excluded: document.credentials == Credentials::Excluded,
        ..Default::default()
    };
    let mut profiles = Vec::with_capacity(document.profiles.len());
    for profile in &document.profiles {
        validate_profile(profile).map_err(|error| RestoreError::Invalid {
            kind: "profile",
            name: profile.name.clone(),
            reason: error.to_string(),
        })?;
        if !held_cores.contains(&profile.core_id) {
            return Err(RestoreError::Dangling {
                name: profile.name.clone(),
                kind: "core",
            });
        }
        if let Some(proxy_id) = profile.proxy_id
            && !held_proxies.contains(&proxy_id)
        {
            return Err(RestoreError::Dangling {
                name: profile.name.clone(),
                kind: "proxy",
            });
        }

        // The one path rule, the same one an import applies: a directory that is
        // not on this machine was never going to be right, and leaving it would
        // give a profile that claims sessions it does not have.
        let mut arriving = profile.clone();
        if !profile.user_data_dir.is_dir() {
            let to = default_user_data_dir(data_dir, profile.id);
            notes.repointed.push(Repointed {
                profile: profile.name.clone(),
                from: profile.user_data_dir.clone(),
                to: to.clone(),
            });
            arriving.user_data_dir = to;
        }
        profiles.push(arriving);
    }

    Ok(RestorePlan {
        cores: document.cores.clone(),
        proxies: document.proxies.clone(),
        profiles,
        notes,
    })
}

/// Makes the installation be the plan, in one transaction.
///
/// The removal and the writing are one operation at the storage layer, so this
/// either happens or it does not: an installation interrupted here is the old
/// configuration, never half of each. That is the whole reason a restore does not
/// go through the three services - they would remove, then write, committing
/// after every record.
pub fn apply_restore(
    plan: RestorePlan,
    present: &ConfigSnapshot,
    configuration: &dyn ConfigurationRepository,
) -> Result<RestoreReport, AppError> {
    configuration.replace(&plan.cores, &plan.proxies, &plan.profiles)?;

    Ok(RestoreReport {
        added: plan.counts(),
        removed: counts_of(present),
        notes: plan.notes,
        failed: Vec::new(),
    })
}

/// Whether any identifier appears twice.
fn repeats<T: Eq + std::hash::Hash>(ids: impl Iterator<Item = T>) -> bool {
    let mut seen = HashSet::new();
    ids.into_iter().any(|id| !seen.insert(id))
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
        CoreService, DefaultCoreService, DefaultProfileService, DefaultProxyService,
        ProfileService, read_config_backup, read_configuration, write_config_backup,
    };
    use domain::{
        BrowserCore, BrowserProfile, CoreId, FingerprintProfile, ProfileId, ProxyId, ProxyOutbound,
        ProxyProfile, ShadowsocksOutbound, Socks5Outbound, StartTarget, StreamSettings,
        WindowProfile,
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
        configuration: Arc<dyn ConfigurationRepository>,
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
                // The one transaction, over the same connection the three
                // services above write through: a test can watch what the
                // services see before and after a replacement.
                configuration: Arc::new(storage.clone()),
                scratch,
            }
        }

        fn present(&self) -> ConfigSnapshot {
            read_configuration(&self.profiles, &self.cores, &self.proxies).expect("read")
        }

        fn restore(&self, document: &ConfigBackup, mode: RestoreMode) -> RestoreReport {
            let present = self.present();
            let plan = plan_restore(document, &present, &self.scratch.dir, mode).expect("plan");
            apply_restore(plan, &present, self.configuration.as_ref()).expect("apply")
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

    /// A proxy that cannot be valid without a password, which is what makes an
    /// export that left credentials out unrestorable rather than merely less
    /// useful.
    fn shadowsocks_proxy(name: &str) -> ProxyProfile {
        ProxyProfile {
            id: ProxyId::new(),
            name: name.to_string(),
            outbound: ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                host: "203.0.113.10".to_string(),
                port: 8388,
                password: SECRET.to_string(),
                method: "aes-256-gcm".to_string(),
                stream: StreamSettings::plain(),
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

        let RestoreError::NotEmpty { present } = error else {
            panic!("a populated installation is refused for being populated: {error}");
        };
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

    /// An installation holding the whole configuration, and what it holds, so a
    /// refused restore can be shown to have changed nothing.
    fn installed(name: &str) -> (Fixture, ConfigSnapshot) {
        let fixture = Fixture::new(name);
        let (document, _, _, _) = whole();
        fixture.restore(&document, RestoreMode::OnlyWhenEmpty);
        let before = fixture.present();
        (fixture, before)
    }

    /// Plans a replacement with a broken file, and checks that the installation
    /// is exactly what it was.
    ///
    /// The assertion is the point of the whole function: a restore that fails has
    /// to fail before it removes anything, because "the old configuration is
    /// gone and the new one is not here" is the one state a restore must never
    /// leave behind.
    fn refused(
        fixture: &Fixture,
        document: &ConfigBackup,
        before: &ConfigSnapshot,
    ) -> RestoreError {
        let error = plan_restore(
            document,
            &fixture.present(),
            &fixture.scratch.dir,
            RestoreMode::Replace,
        )
        .expect_err("the file cannot become this installation");
        assert_eq!(
            &fixture.present(),
            before,
            "a refused restore changed the installation"
        );
        error
    }

    /// A record this program would refuse if a form had produced it takes the
    /// whole file with it, and nothing is removed to make room for a file that
    /// cannot be stored.
    #[test]
    fn a_file_with_a_record_this_program_refuses_changes_nothing() {
        let (fixture, before) = installed("refuses-record");
        let (mut document, _, _, _) = whole();
        document.cores[0].name = "   ".to_string();

        let error = refused(&fixture, &document, &before);
        assert!(
            matches!(error, RestoreError::Invalid { kind: "core", .. }),
            "{error}"
        );
    }

    /// Restore replaces the whole configuration, so a name that resolves to
    /// nothing in the file is a profile that cannot be stored - unlike an import,
    /// where the record may be one the installation already has.
    #[test]
    fn a_file_whose_profile_names_a_core_it_does_not_hold_changes_nothing() {
        let (fixture, before) = installed("dangling-core");
        let (mut document, _, _, _) = whole();
        document.cores.clear();

        let error = refused(&fixture, &document, &before);
        assert!(
            matches!(error, RestoreError::Dangling { kind: "core", .. }),
            "{error}"
        );
    }

    #[test]
    fn a_file_whose_profile_names_a_proxy_it_does_not_hold_changes_nothing() {
        let (fixture, before) = installed("dangling-proxy");
        let (mut document, _, proxy, _) = whole();
        document.proxies.clear();

        let error = refused(&fixture, &document, &before);
        assert!(
            matches!(error, RestoreError::Dangling { kind: "proxy", .. }),
            "{error}"
        );
        assert_eq!(
            document.profiles[0].proxy_id,
            Some(proxy.id),
            "the file really does name the proxy it does not hold"
        );
    }

    /// Two records with one identifier cannot both be stored, and the file does
    /// not say which of them is the one that was meant.
    #[test]
    fn a_file_that_holds_the_same_identifier_twice_changes_nothing() {
        let (fixture, before) = installed("duplicate");
        let (mut document, core, _, _) = whole();
        document.cores.push(core);

        let error = refused(&fixture, &document, &before);
        assert!(
            matches!(error, RestoreError::Duplicate { kind: "core" }),
            "{error}"
        );
    }

    /// A backup exported without credentials cannot be this installation: for
    /// some outbounds the password is what makes the record valid at all. The
    /// refusal says that rather than "invalid proxy", because the reader's next
    /// move is a different export, not a repair.
    #[test]
    fn a_file_exported_without_credentials_changes_nothing() {
        let (fixture, before) = installed("no-credentials");
        let core = core("Fingerprint Chromium 148");
        let proxy = shadowsocks_proxy("Zurich exit");
        let profile = profile("Work laptop", core.id, Some(proxy.id));
        let document = ConfigBackup::build(
            ConfigSnapshot {
                cores: vec![core],
                proxies: vec![proxy],
                profiles: vec![profile],
            },
            Credentials::Excluded,
            ExportOrigin {
                exported_at: "2026-09-20T16:04:00.000Z".to_string(),
                source_data_dir: "/home/alice/.local/share/FpBrowser".to_string(),
            },
        );
        assert!(
            !document.proxies[0].outbound.carries_credentials(),
            "the export is what leaves the password out"
        );

        let error = refused(&fixture, &document, &before);
        assert!(
            matches!(error, RestoreError::CredentialsExcluded { count: 1 }),
            "{error}"
        );
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
