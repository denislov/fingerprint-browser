//! Reading a configuration backup back into the database.
//!
//! The document is [`crate::config_backup`]'s business and taking one is
//! [`crate::export`]'s. This module is the other direction, and it is the one
//! with the rules, because a file names identifiers, paths and cores that belong
//! to the machine it was written on.
//!
//! Two halves, split the same way the export is. [`plan_import`] decides, before
//! anything is written, what a document would do to an installation that already
//! holds something: which identifiers are free, which profiles name a core that
//! is not here, which directories have to move. It is a function of the
//! document, the three lists and the data directory, so the decision table can
//! be read and tested as a table. [`apply_import`] then writes what the plan
//! asks for and reports what landed.
//!
//! What is deliberately absent: restore. Restore is import plus a precondition -
//! an empty configuration, or a confirmation to replace one - and that
//! precondition is a question about the installation rather than about the file.

use crate::config_backup::{BackupError, ConfigBackup, ConfigSnapshot};
use crate::core_service::CoreService;
use crate::error::AppError;
use crate::profile_service::{ProfileService, default_user_data_dir};
use crate::proxy_service::ProxyService;
use domain::{BrowserCore, BrowserProfile, CoreId, ProfileId, ProxyId, ProxyProfile};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Reads a configuration backup off the disk.
///
/// The two failures are kept apart because they call for different next steps,
/// which is the same reason [`BackupError`] keeps three: a path with nothing at
/// it is a path to correct, and a file that is not one of ours is a different
/// file to pick.
pub fn read_config_backup(path: &Path) -> Result<ConfigBackup, ImportError> {
    let text = std::fs::read_to_string(path).map_err(|source| ImportError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(ConfigBackup::from_json(&text)?)
}

/// Three counts, so a report can say "2 cores and 5 profiles" without three
/// parallel fields at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counts {
    pub cores: usize,
    pub proxies: usize,
    pub profiles: usize,
}

impl Counts {
    pub fn total(&self) -> usize {
        self.cores + self.proxies + self.profiles
    }
}

/// A profile whose recorded directory is not on this machine, and the one it was
/// given instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repointed {
    pub profile: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

/// What an import decided *not* to do, and why.
///
/// Every field here is a decision about the database the plan was made against,
/// so the writer cannot change one of them: it writes what it is given and adds
/// only what it learns while writing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportNotes {
    /// Items the database already holds under the identifier the file names.
    ///
    /// Being counted here is the ordinary outcome of importing a file twice, and
    /// is not by itself a problem - see [`ImportNotes::needs_attention`].
    pub kept: Counts,
    /// Kept items whose stored copy is not the file's copy, by name.
    ///
    /// Import never overwrites, so this list is the only way a reader finds out
    /// that their file and their installation disagree. Without it, "kept" reads
    /// as agreement, and someone who edited a proxy in the file and imported it
    /// to apply the change would be told nothing.
    pub differing: Vec<String>,
    /// Profiles skipped because their core is neither in the file nor in the
    /// database, by name.
    pub missing_core: Vec<String>,
    /// Profiles imported with no proxy because the proxy they name is neither in
    /// the file nor in the database, by name.
    pub missing_proxy: Vec<String>,
    /// Profiles whose directory had to move, and where it went.
    ///
    /// Named rather than flagged, for the reason [`ImportNotes::needs_attention`]
    /// gives: importing onto another machine moves every directory there is, so
    /// this is a thing to report and not a thing to alarm about.
    pub repointed: Vec<Repointed>,
    /// Whether the file was written without the proxy credentials it could have
    /// carried.
    ///
    /// Recorded in the file rather than inferred, because an import cannot tell
    /// a proxy that never had a password from one whose password was stripped:
    /// SOCKS5 and HTTP authenticate optionally, so both validate. What it can do
    /// is pass the file's own statement on, so the reader is not left wondering
    /// why a proxy they imported does not connect.
    pub credentials_excluded: bool,
}

impl ImportNotes {
    /// Whether the import did less than the file asked for.
    ///
    /// An import that added nothing and left everything as it was is the
    /// ordinary result of importing the same file twice, and is not a problem.
    /// Nor is a profile whose directory had to move: that is what importing onto
    /// a machine that never had those directories *does*, and raising an alarm
    /// for the expected outcome of the feature would train the reader to dismiss
    /// the alarm. The move is still reported - by name, in the summary sentence -
    /// which is the right loudness for something the reader wants to know and
    /// does not have to fix.
    ///
    /// What is left is the import having done *less* than it was asked: a kept
    /// item that differs, or a profile whose core or proxy is not here.
    pub fn needs_attention(&self) -> bool {
        !self.differing.is_empty()
            || !self.missing_core.is_empty()
            || !self.missing_proxy.is_empty()
    }
}

/// What an import will do, decided before anything is written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportPlan {
    /// Cores to store, in the order the file lists them.
    pub cores: Vec<BrowserCore>,
    /// Proxies to store.
    pub proxies: Vec<ProxyProfile>,
    /// Profiles to insert, with their directories already resolved for this
    /// machine and their absent proxies already cleared.
    pub profiles: Vec<BrowserProfile>,
    /// What was left out, and why.
    pub notes: ImportNotes,
}

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImportReport {
    /// What landed, by kind.
    ///
    /// Counted from the writes rather than from the plan: a record the database
    /// refuses is not added, and a report that counted intentions would be wrong
    /// exactly when someone needed it.
    pub added: Counts,
    /// What the plan decided not to do.
    pub notes: ImportNotes,
    /// Records the database refused, named, in the order they were attempted.
    pub failed: Vec<String>,
}

impl ImportReport {
    /// Whether the outcome is worse than the file asked for.
    pub fn needs_attention(&self) -> bool {
        !self.failed.is_empty() || self.notes.needs_attention()
    }
}

/// Decides what `document` would do to an installation holding `present`.
///
/// Pure except for one question it has to put to the filesystem: whether a
/// profile's recorded directory is on this machine. Everything else is a
/// comparison against `present`, which is what lets the rules be read here
/// rather than hunted for in the writer.
///
/// The file is untrusted input. A well-formed export cannot name a core that is
/// not in it, but a document is a file a person may edit, and "the format
/// forbids it" is not the same as "it cannot arrive".
pub fn plan_import(
    document: &ConfigBackup,
    present: &ConfigSnapshot,
    data_dir: &Path,
) -> ImportPlan {
    let held_cores: HashSet<CoreId> = present.cores.iter().map(|core| core.id).collect();
    let held_proxies: HashSet<ProxyId> = present.proxies.iter().map(|proxy| proxy.id).collect();
    let held_profiles: HashSet<ProfileId> =
        present.profiles.iter().map(|profile| profile.id).collect();

    // The identifiers a profile may name, which is not the set that is already
    // here: a core or a proxy can arrive with the file it is named by.
    let arriving_cores: HashSet<CoreId> = document
        .cores
        .iter()
        .map(|core| core.id)
        .filter(|id| !held_cores.contains(id))
        .collect();
    let arriving_proxies: HashSet<ProxyId> = document
        .proxies
        .iter()
        .map(|proxy| proxy.id)
        .filter(|id| !held_proxies.contains(id))
        .collect();

    let mut notes = ImportNotes {
        credentials_excluded: document.credentials == crate::config_backup::Credentials::Excluded,
        ..Default::default()
    };
    let mut cores = Vec::new();
    let mut proxies = Vec::new();
    let mut profiles = Vec::new();

    for core in &document.cores {
        if held_cores.contains(&core.id) {
            notes.kept.cores += 1;
            if !present.cores.iter().any(|held| held == core) {
                notes.differing.push(core.name.clone());
            }
            continue;
        }
        cores.push(core.clone());
    }

    for proxy in &document.proxies {
        if held_proxies.contains(&proxy.id) {
            notes.kept.proxies += 1;
            if !present.proxies.iter().any(|held| held == proxy) {
                notes.differing.push(proxy.name.clone());
            }
            continue;
        }
        proxies.push(proxy.clone());
    }

    for profile in &document.profiles {
        if held_profiles.contains(&profile.id) {
            notes.kept.profiles += 1;
            if !present
                .profiles
                .iter()
                .any(|held| profile_matches(profile, held))
            {
                notes.differing.push(profile.name.clone());
            }
            continue;
        }

        // A profile without its core is not imported at all: re-pointing it at
        // another core would change the engine it runs on, and the engine is
        // what the fingerprint claims to be.
        if !(held_cores.contains(&profile.core_id) || arriving_cores.contains(&profile.core_id)) {
            notes.missing_core.push(profile.name.clone());
            continue;
        }

        let mut arriving = profile.clone();

        // A profile without its proxy is imported with none. The database cannot
        // hold a dangling reference anyway - `profiles.proxy_id` is a foreign
        // key with `ON DELETE SET NULL` - so the alternative to clearing it is a
        // write that fails.
        if let Some(proxy_id) = profile.proxy_id
            && !(held_proxies.contains(&proxy_id) || arriving_proxies.contains(&proxy_id))
        {
            notes.missing_proxy.push(profile.name.clone());
            arriving.proxy_id = None;
        }

        // The one path rule. A directory that is not on this machine was never
        // going to be right; leaving it would give a profile that claims
        // sessions it does not have.
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

    ImportPlan {
        cores,
        proxies,
        profiles,
        notes,
    }
}

/// Writes what the plan asks for, in dependency order, and reports what landed.
///
/// Cores first, then proxies, then profiles, because `profiles.core_id` and
/// `profiles.proxy_id` are foreign keys and the database enforces them
/// (`PRAGMA foreign_keys = ON`). After each of the first two phases the
/// identifiers are read back out of the database, so the profile phase decides
/// against what is stored rather than against what the plan meant to store. A
/// core the database refused therefore takes its profiles with it as *skipped*,
/// and a proxy it refused leaves its profiles *imported with no proxy* - which
/// are the same two answers the rules already give when the file itself is the
/// reason, said about a write that did not happen.
///
/// An item the database refuses does not stop the rest. That is the difference
/// between this and the "one transaction" the design first asked for, and it is
/// the better of the two: a transaction would let one unreadable proxy take an
/// otherwise good import down with it, when what the rules ask for is a profile
/// imported without that proxy and a sentence saying so. What the transaction
/// was protecting against - a profile pointing at a core that is not there - is
/// what the ordering and the read-back are for.
///
/// Fails only when the database cannot be read at all, which is not a property
/// of the file and leaves nothing useful to report item by item.
pub fn apply_import(
    plan: ImportPlan,
    cores: &dyn CoreService,
    proxies: &dyn ProxyService,
    profiles: &dyn ProfileService,
) -> Result<ImportReport, AppError> {
    let mut report = ImportReport {
        notes: plan.notes,
        ..Default::default()
    };

    for core in &plan.cores {
        match cores.insert(core.clone()) {
            Ok(()) => report.added.cores += 1,
            Err(error) => report.failed.push(format!("core {}: {error}", core.name)),
        }
    }
    let here: HashSet<CoreId> = cores.list()?.into_iter().map(|core| core.id).collect();

    for proxy in &plan.proxies {
        match proxies.insert(proxy.clone()) {
            Ok(()) => report.added.proxies += 1,
            Err(error) => report.failed.push(format!("proxy {}: {error}", proxy.name)),
        }
    }
    let here_proxies: HashSet<ProxyId> =
        proxies.list()?.into_iter().map(|proxy| proxy.id).collect();

    for profile in plan.profiles {
        if !here.contains(&profile.core_id) {
            report.notes.missing_core.push(profile.name);
            continue;
        }

        let mut arriving = profile;
        if let Some(proxy_id) = arriving.proxy_id
            && !here_proxies.contains(&proxy_id)
        {
            report.notes.missing_proxy.push(arriving.name.clone());
            arriving.proxy_id = None;
        }

        let name = arriving.name.clone();
        match profiles.insert(arriving) {
            Ok(()) => report.added.profiles += 1,
            Err(error) => report.failed.push(format!("profile {name}: {error}")),
        }
    }

    Ok(report)
}

/// Whether a stored profile is the file's profile.
///
/// Every field but the directory. A profile kept by identifier was re-pointed
/// when it was first imported, so its stored directory is *expected* to differ
/// from the one in the file it came from - comparing that field would make the
/// ordinary case of importing the same file twice after moving machines read as
/// a disagreement, and a report that cries wolf is worse than no report.
fn profile_matches(file: &BrowserProfile, stored: &BrowserProfile) -> bool {
    let mut comparable = file.clone();
    comparable.user_data_dir = stored.user_data_dir.clone();
    &comparable == stored
}

/// Why a configuration backup could not be read off the disk.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("could not read {path}: {source}")]
    Read { path: PathBuf, source: io::Error },

    #[error(transparent)]
    Document(#[from] BackupError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_backup::{Credentials, ExportOrigin};
    use crate::{
        DefaultCoreService, DefaultProfileService, DefaultProxyService, read_configuration,
        write_config_backup,
    };
    use domain::{
        FingerprintProfile, ProxyOutbound, ShadowsocksOutbound, Socks5Outbound, StartTarget,
        WindowProfile,
    };
    use std::sync::Arc;

    const SECRET: &str = "correct horse battery staple";

    /// A directory of its own under the system temporary directory, removed when
    /// the test ends.
    ///
    /// The path rule asks the filesystem one question, so a test of it needs a
    /// filesystem that is not the machine's own.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-import-{name}"));
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

    /// The three services over one in-memory database, which is what an import
    /// actually writes into.
    struct Fixture {
        scratch: Scratch,
        cores: DefaultCoreService,
        proxies: DefaultProxyService,
        profiles: DefaultProfileService,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let scratch = Scratch::new(name);
            let storage = storage::SqliteStorage::in_memory().expect("sqlite in memory");
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

        fn plan(&self, document: &ConfigBackup) -> ImportPlan {
            plan_import(document, &self.present(), &self.scratch.dir)
        }

        fn import(&self, document: &ConfigBackup) -> ImportReport {
            apply_import(
                self.plan(document),
                &self.cores,
                &self.proxies,
                &self.profiles,
            )
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

    fn open_proxy(name: &str) -> ProxyProfile {
        ProxyProfile {
            id: ProxyId::new(),
            name: name.to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "203.0.113.10".to_string(),
                port: 1080,
                username: None,
                password: None,
            }),
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

    fn profile(
        name: &str,
        core_id: CoreId,
        proxy_id: Option<ProxyId>,
        dir: &str,
    ) -> BrowserProfile {
        BrowserProfile {
            id: ProfileId::new(),
            name: name.to_string(),
            core_id,
            user_data_dir: PathBuf::from(dir),
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

    /// A file holding one core, one proxy and one profile that uses both, with
    /// the profile's directory a path that exists on no machine.
    fn whole() -> (ConfigBackup, BrowserCore, ProxyProfile, BrowserProfile) {
        let core = core("Fingerprint Chromium 148");
        let proxy = secret_proxy("Zurich exit");
        let profile = profile(
            "Work laptop",
            core.id,
            Some(proxy.id),
            "/home/alice/.local/share/FpBrowser/profiles/one",
        );
        let document = document(ConfigSnapshot {
            cores: vec![core.clone()],
            proxies: vec![proxy.clone()],
            profiles: vec![profile.clone()],
        });
        (document, core, proxy, profile)
    }

    #[test]
    fn a_backup_is_read_back_from_the_path_it_was_written_to() {
        // The two halves of the exchange in one test, because they are one
        // promise: what an export writes is what an import reads.
        let scratch = Scratch::new("round-trip");
        let path = scratch.dir.join("config.json");
        let (written, _, _, _) = whole();
        write_config_backup(
            ConfigSnapshot {
                cores: written.cores.clone(),
                proxies: written.proxies.clone(),
                profiles: written.profiles.clone(),
            },
            Credentials::Included,
            ExportOrigin {
                exported_at: written.exported_at.clone(),
                source_data_dir: written.source_data_dir.clone(),
            },
            &path,
        )
        .expect("export");

        assert_eq!(read_config_backup(&path).expect("import"), written);
    }

    #[test]
    fn a_path_with_nothing_at_it_is_refused_by_name() {
        let scratch = Scratch::new("no-file");
        let path = scratch.dir.join("absent.json");

        match read_config_backup(&path) {
            Err(ImportError::Read { path: named, .. }) => assert_eq!(named, path),
            other => panic!("expected a read failure, got {other:?}"),
        }
    }

    #[test]
    fn a_file_that_is_not_one_of_ours_is_refused_as_a_document() {
        let scratch = Scratch::new("foreign");
        let path = scratch.dir.join("other.json");
        std::fs::write(&path, r#"{"format": "some-other-tool/v3", "version": 1}"#).expect("write");

        match read_config_backup(&path) {
            Err(ImportError::Document(BackupError::UnknownFormat { found })) => {
                assert_eq!(found, "some-other-tool/v3")
            }
            other => panic!("expected a foreign document, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_installation_is_planned_to_take_everything() {
        let fixture = Fixture::new("empty-install");
        let (document, _, _, _) = whole();

        let plan = fixture.plan(&document);

        assert_eq!(plan.cores.len(), 1);
        assert_eq!(plan.proxies.len(), 1);
        assert_eq!(plan.profiles.len(), 1);
        assert_eq!(plan.notes.kept.total(), 0);
        // Every directory moved, because this machine never had the one the file
        // records - which is what an import onto another machine does.
        assert_eq!(plan.notes.repointed.len(), 1);
        assert!(!plan.notes.needs_attention(), "{:?}", plan.notes);
    }

    #[test]
    fn an_identifier_that_is_taken_is_kept_rather_than_overwritten() {
        let fixture = Fixture::new("taken");
        let (document, core, _, _) = whole();
        fixture.cores.insert(core.clone()).expect("seed the core");

        let plan = fixture.plan(&document);

        // Not in the plan at all: the writer never sees a record it could
        // overwrite, which is what makes "never overwrites" a property of the
        // plan rather than a promise about the writer.
        assert!(plan.cores.is_empty());
        assert_eq!(plan.notes.kept.cores, 1);
        // And it is the same core, so there is nothing to tell the reader.
        assert!(plan.notes.differing.is_empty(), "{:?}", plan.notes);
    }

    #[test]
    fn a_kept_item_that_differs_is_named() {
        // The failure this exists to prevent: someone edits a proxy in the file,
        // imports it expecting the change to apply, and is told nothing because
        // the identifier was already there.
        let fixture = Fixture::new("differs");
        let (document, core, proxy, _) = whole();
        fixture.cores.insert(core).expect("seed the core");
        fixture.proxies.insert(proxy).expect("seed the proxy");

        let mut edited = document.clone();
        edited.proxies[0].outbound = ProxyOutbound::Socks5(Socks5Outbound {
            host: "198.51.100.7".to_string(),
            port: 1080,
            username: Some("alice".to_string()),
            password: Some(SECRET.to_string()),
        });

        let plan = fixture.plan(&edited);

        // The core is the same one and is not named; the proxy is not, and is.
        assert_eq!(plan.notes.kept.cores, 1);
        assert_eq!(plan.notes.kept.proxies, 1);
        assert_eq!(plan.notes.differing, vec!["Zurich exit".to_string()]);
        assert!(plan.notes.needs_attention());
    }

    #[test]
    fn a_repointed_profile_kept_by_identifier_is_not_reported_as_differing() {
        // The false alarm the directory exception exists to stop. A profile that
        // was imported before carries *this* machine's directory, so the file's
        // copy always differs in that one field; the second import has to read as
        // agreement, or every re-import of a moved configuration would warn.
        let fixture = Fixture::new("kept-identical");
        let (document, _, _, _) = whole();
        let first = fixture.import(&document);
        assert_eq!(first.added.profiles, 1);

        let second = fixture.import(&document);

        assert_eq!(second.added.total(), 0);
        assert_eq!(second.notes.kept.total(), 3);
        assert!(second.notes.differing.is_empty(), "{:?}", second.notes);
        assert!(
            !second.needs_attention(),
            "importing the same file twice is not a problem: {:?}",
            second.notes
        );
    }

    #[test]
    fn a_second_import_adds_nothing_at_all() {
        // Idempotence, which is the property that lets an import be the thing
        // someone reaches for when they are unsure: running it twice does not
        // duplicate a proxy or a profile.
        let fixture = Fixture::new("idempotent");
        let (document, _, _, _) = whole();

        fixture.import(&document);
        let before = fixture.present();
        let again = fixture.import(&document);
        let after = fixture.present();

        assert_eq!(again.added.total(), 0);
        assert_eq!(after, before);
    }

    #[test]
    fn a_profile_whose_core_is_missing_is_skipped_and_named() {
        // The file names a core it does not carry and this machine does not
        // have: an edited or damaged document, which is why it is handled rather
        // than assumed away.
        let fixture = Fixture::new("no-core");
        let (document, _, _, _) = whole();
        let mut orphans = document.clone();
        orphans.cores.clear();

        let plan = fixture.plan(&orphans);

        assert!(plan.profiles.is_empty());
        assert_eq!(plan.notes.missing_core, vec!["Work laptop".to_string()]);
        assert_eq!(plan.notes.kept.total(), 0);
        assert!(plan.notes.needs_attention());
    }

    #[test]
    fn a_profile_whose_core_arrives_with_the_file_is_planned() {
        // The other side of the rule: the core is not in the database, but it is
        // in the same document, so the profile is not an orphan.
        let fixture = Fixture::new("core-arrives");
        let (document, _, _, _) = whole();

        let plan = fixture.plan(&document);

        assert_eq!(plan.profiles.len(), 1);
        assert!(plan.notes.missing_core.is_empty());
    }

    #[test]
    fn a_profile_whose_proxy_is_missing_is_planned_with_no_proxy() {
        // Imported, not skipped: the proxy is how the profile reaches the
        // network, not what it is. It arrives direct and the report says so -
        // the same treatment an unreadable journal record gets.
        let fixture = Fixture::new("no-proxy");
        let (document, _, _, _) = whole();
        let mut orphans = document.clone();
        orphans.proxies.clear();

        let plan = fixture.plan(&orphans);

        assert_eq!(plan.profiles.len(), 1);
        assert_eq!(plan.profiles[0].proxy_id, None);
        assert_eq!(plan.notes.missing_proxy, vec!["Work laptop".to_string()]);
        assert!(plan.notes.missing_core.is_empty());
    }

    #[test]
    fn a_directory_that_is_on_this_machine_is_kept_as_written() {
        // The same path on a machine that has it is the path that should be
        // used: re-pointing it would abandon browser data that is actually here.
        let fixture = Fixture::new("dir-present");
        let mine = fixture.scratch.dir.join("profiles").join("one");
        std::fs::create_dir_all(&mine).expect("create the directory");
        let (mut document, _, _, _) = whole();
        document.profiles[0].user_data_dir = mine.clone();

        let plan = fixture.plan(&document);

        assert_eq!(plan.profiles[0].user_data_dir, mine);
        assert!(plan.notes.repointed.is_empty());
    }

    #[test]
    fn a_directory_that_is_not_here_is_repointed_at_the_local_default() {
        let fixture = Fixture::new("dir-absent");
        let (document, _, _, profile) = whole();

        let plan = fixture.plan(&document);

        assert_eq!(plan.notes.repointed.len(), 1);
        assert_eq!(plan.notes.repointed[0].profile, "Work laptop");
        assert_eq!(plan.notes.repointed[0].from, profile.user_data_dir);
        assert_eq!(
            plan.notes.repointed[0].to,
            default_user_data_dir(&fixture.scratch.dir, profile.id)
        );
        assert_eq!(plan.profiles[0].user_data_dir, plan.notes.repointed[0].to);
    }

    #[test]
    fn a_repointed_directory_is_the_one_that_gets_written() {
        // The plan's path has to be the path the database ends up with, or the
        // report would name a file that no profile points at.
        let fixture = Fixture::new("dir-written");
        let (document, _, _, _) = whole();

        fixture.import(&document);

        let stored = fixture.present().profiles;
        assert_eq!(stored.len(), 1);
        assert_eq!(
            stored[0].user_data_dir,
            default_user_data_dir(&fixture.scratch.dir, stored[0].id)
        );
    }

    #[test]
    fn an_import_writes_cores_proxies_and_profiles_that_line_up() {
        let fixture = Fixture::new("writes");
        let (document, core, proxy, _) = whole();

        let report = fixture.import(&document);

        assert_eq!(
            report.added,
            Counts {
                cores: 1,
                proxies: 1,
                profiles: 1
            }
        );
        assert!(report.failed.is_empty(), "{:?}", report.failed);

        let stored = fixture.present();
        assert_eq!(stored.profiles[0].core_id, core.id);
        assert_eq!(stored.profiles[0].proxy_id, Some(proxy.id));
    }

    #[test]
    fn an_import_keeps_the_credentials_the_file_carried() {
        // "Included" means included: an import that quietly stripped what it was
        // given would make a backup useless for the one thing it is best at.
        let fixture = Fixture::new("keeps-secrets");
        let (document, _, _, _) = whole();

        fixture.import(&document);

        let stored = fixture.present().proxies;
        assert!(stored[0].outbound.carries_credentials());
    }

    #[test]
    fn a_proxy_the_database_refuses_leaves_its_profile_direct_and_says_so() {
        // The case the design calls "nothing new was needed for that": a
        // Shadowsocks proxy whose password was left out of the file does not
        // validate, so the write fails - and the rule for a profile whose proxy
        // is not there covers the profile, which is imported with no proxy.
        let fixture = Fixture::new("refused-proxy");
        let core = core("Fingerprint Chromium 148");
        let stripped = ProxyProfile {
            id: ProxyId::new(),
            name: "Relay".to_string(),
            outbound: ProxyOutbound::Shadowsocks(ShadowsocksOutbound {
                host: "203.0.113.11".to_string(),
                port: 8388,
                password: String::new(),
                method: "aes-256-gcm".to_string(),
                stream: Default::default(),
            }),
        };
        let profile = profile("Work laptop", core.id, Some(stripped.id), "/nowhere/one");
        let document = document(ConfigSnapshot {
            cores: vec![core.clone()],
            proxies: vec![stripped.clone()],
            profiles: vec![profile.clone()],
        });

        let report = fixture.import(&document);

        assert_eq!(report.added.cores, 1);
        assert_eq!(report.added.proxies, 0);
        assert_eq!(report.added.profiles, 1);
        assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
        assert!(report.failed[0].contains("Relay"), "{:?}", report.failed);
        assert_eq!(report.notes.missing_proxy, vec!["Work laptop".to_string()]);

        let stored = fixture.present();
        assert_eq!(stored.proxies.len(), 0);
        assert_eq!(stored.profiles[0].proxy_id, None);
    }

    #[test]
    fn a_core_the_database_refuses_takes_its_profiles_with_it() {
        // The other half, and the reason the identifier sets are read back after
        // each phase rather than counted from the plan: the plan said the core
        // would be there, the database refused it, and the profile must not be
        // inserted pointing at nothing.
        let fixture = Fixture::new("refused-core");
        let nameless = core("   ");
        let profile = profile("Work laptop", nameless.id, None, "/nowhere/one");
        let document = document(ConfigSnapshot {
            cores: vec![nameless],
            proxies: Vec::new(),
            profiles: vec![profile],
        });

        // The plan expected both: it is the write that changed the answer, not
        // the plan, which is exactly what the read-back is for.
        let plan = fixture.plan(&document);
        assert_eq!(plan.cores.len(), 1);
        assert_eq!(plan.profiles.len(), 1);

        let report =
            apply_import(plan, &fixture.cores, &fixture.proxies, &fixture.profiles).expect("apply");

        assert_eq!(report.added.cores, 0);
        assert_eq!(report.added.profiles, 0);
        assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
        assert_eq!(report.notes.missing_core, vec!["Work laptop".to_string()]);
        assert!(fixture.present().profiles.is_empty());
    }

    #[test]
    fn an_item_the_database_refuses_does_not_stop_the_rest() {
        let fixture = Fixture::new("carries-on");
        let (document, _, _, _) = whole();
        let mut partly_bad = document.clone();
        partly_bad.cores.push(core("   "));
        let extra_proxy = open_proxy("Office");
        partly_bad.proxies.push(extra_proxy.clone());

        let report = fixture.import(&partly_bad);

        assert_eq!(report.added.cores, 1);
        assert_eq!(report.added.proxies, 2);
        assert_eq!(report.added.profiles, 1);
        assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
        assert!(
            fixture
                .present()
                .proxies
                .iter()
                .any(|p| p.id == extra_proxy.id)
        );
    }

    #[test]
    fn an_excluded_file_is_reported_as_having_no_credentials_to_restore() {
        // Recorded in the file rather than inferred, and passed on: a SOCKS5
        // proxy with its password stripped validates exactly like one that never
        // had a password, so this note is the only thing that tells the reader
        // why the proxy they just imported does not connect.
        let fixture = Fixture::new("excluded-note");
        let (document, _, _, _) = whole();
        let mut stripped = document.clone();
        stripped.credentials = Credentials::Excluded;
        stripped.proxies[0].outbound = ProxyOutbound::Socks5(Socks5Outbound {
            host: "203.0.113.10".to_string(),
            port: 1080,
            username: None,
            password: None,
        });

        let plan = fixture.plan(&stripped);

        assert!(plan.notes.credentials_excluded);
        // Not folded into `needs_attention`: the import did exactly what the file
        // asked, and the note is there for the sentence after it.
        assert!(!plan.notes.needs_attention(), "{:?}", plan.notes);
    }

    #[test]
    fn an_included_file_is_not_reported_that_way() {
        let fixture = Fixture::new("included-note");
        let (document, _, _, _) = whole();

        assert!(!fixture.plan(&document).notes.credentials_excluded);
    }

    #[test]
    fn an_empty_document_leaves_everything_where_it_was() {
        let fixture = Fixture::new("empty-document");
        let empty = document(ConfigSnapshot::default());

        let report = fixture.import(&empty);

        assert_eq!(report.added.total(), 0);
        assert!(report.failed.is_empty());
        assert!(!report.needs_attention());
    }
}
