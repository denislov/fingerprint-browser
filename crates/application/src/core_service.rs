//! The browser cores a profile can be launched with.
//!
//! A core carries the major that decides which fingerprint switches may be
//! claimed, so nothing here accepts a version the binary did not report. Adding
//! a core probes the executable and stores what came back; pointing a core at a
//! different binary probes that one rather than keeping the old major. A
//! version that cannot be read is refused when the user adds the core, because
//! they are right there and can pick another binary - unlike the bootstrap
//! discovery, which keeps such a core and says so, since there is nothing for
//! the user to correct at startup.
//!
//! Removing a core is refused while profiles still point at it, for the same
//! reason proxies are: `profiles.core_id` has no cascade, so the delete would
//! either fail at the database or leave a profile that cannot start.

use crate::error::AppError;
use domain::{BrowserCore, CoreId, validate_core};
use runtime::version::{DEFAULT_TIMEOUT as VERSION_TIMEOUT, VersionReport};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage::{CoreRepository, ProfileRepository};

/// Reads the version a binary reports. Injected so this can be tested without
/// spawning anything.
pub type VersionProbe = Box<dyn Fn(&Path) -> VersionReport + Send + Sync>;

pub trait CoreService: Send + Sync {
    /// Adds a core for `path`, named `name` or after the binary.
    fn add(&self, name: Option<String>, path: PathBuf) -> Result<BrowserCore, AppError>;
    /// Saves a renamed core, re-probing it when its executable changed.
    fn update(&self, core: BrowserCore) -> Result<BrowserCore, AppError>;
    /// Re-reads the version of a stored core's binary.
    fn redetect(&self, id: CoreId) -> Result<BrowserCore, AppError>;
    /// Stores a core exactly as given, without probing its binary.
    ///
    /// For import. The file records the version that *its* machine read from the
    /// binary at that path, so probing here would replace a real reading with
    /// whatever is at the same path now - and refusing when there is nothing
    /// there would drop a core the rest of the program already handles: a core
    /// whose binary is absent is shown as one that is not present, and
    /// [`CoreService::redetect`] is the existing way to read the version again.
    /// A version that was never read is still refused at launch rather than
    /// assumed, because `major` `0` has no capability table.
    fn insert(&self, core: BrowserCore) -> Result<(), AppError>;
    fn delete(&self, id: CoreId) -> Result<(), AppError>;
    fn get(&self, id: CoreId) -> Result<Option<BrowserCore>, AppError>;
    fn list(&self) -> Result<Vec<BrowserCore>, AppError>;
    /// Which profiles each core is used by, by name.
    fn usage(&self) -> Result<HashMap<CoreId, Vec<String>>, AppError>;
}

pub struct DefaultCoreService {
    cores: Arc<dyn CoreRepository>,
    profiles: Arc<dyn ProfileRepository>,
    probe: VersionProbe,
}

impl DefaultCoreService {
    pub fn new(cores: Arc<dyn CoreRepository>, profiles: Arc<dyn ProfileRepository>) -> Self {
        Self::with_probe(
            cores,
            profiles,
            Box::new(|path| VersionReport::probe(path, VERSION_TIMEOUT)),
        )
    }

    pub fn with_probe(
        cores: Arc<dyn CoreRepository>,
        profiles: Arc<dyn ProfileRepository>,
        probe: VersionProbe,
    ) -> Self {
        Self {
            cores,
            profiles,
            probe,
        }
    }

    /// The version a binary reports, or the reason it cannot be used.
    ///
    /// The banner is kept verbatim: it is what the engine said about itself.
    /// Major `0` is refused rather than stored, because it is not a generation
    /// and a core without a generation has no capability table to launch with.
    fn read_version(&self, path: &Path) -> Result<(String, u32), AppError> {
        if !path.is_file() {
            return Err(AppError::Conflict(format!(
                "there is no file at {}",
                path.display()
            )));
        }

        let report = (self.probe)(path);
        let major = report.major.filter(|major| *major > 0).ok_or_else(|| {
            AppError::Conflict(format!(
                "{} did not report a version when asked for --version; \
                 pick another binary, or set FP_BROWSER_CHROMIUM_MAJOR and restart",
                path.display()
            ))
        })?;

        Ok((
            report.banner.unwrap_or_else(|| format!("major {major}")),
            major,
        ))
    }

    fn users_of(&self, id: CoreId) -> Result<Vec<String>, AppError> {
        let mut users: Vec<String> = self
            .profiles
            .list()?
            .into_iter()
            .filter(|profile| profile.core_id == id)
            .map(|profile| profile.name)
            .collect();
        users.sort();
        Ok(users)
    }

    fn stored(&self, id: CoreId) -> Result<BrowserCore, AppError> {
        self.cores
            .get(id)?
            .ok_or_else(|| AppError::NotFound(format!("core {id}")))
    }
}

impl CoreService for DefaultCoreService {
    fn add(&self, name: Option<String>, path: PathBuf) -> Result<BrowserCore, AppError> {
        if let Some(existing) = self
            .cores
            .list()?
            .into_iter()
            .find(|core| core.executable == path)
        {
            return Err(AppError::Conflict(format!(
                "{} is already registered as {}",
                path.display(),
                existing.name
            )));
        }

        let (version, major) = self.read_version(&path)?;
        let core = BrowserCore {
            id: CoreId::new(),
            name: name
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| domain::suggested_name(&path, major)),
            executable: path,
            version,
            major,
        };
        validate_core(&core)?;
        self.cores.save(&core)?;
        Ok(core)
    }

    fn update(&self, core: BrowserCore) -> Result<BrowserCore, AppError> {
        let stored = self.stored(core.id)?;
        validate_core(&core)?;

        // A core pointed at another binary cannot keep the old major: the
        // capability table hangs off it.
        let updated = if core.executable != stored.executable {
            let (version, major) = self.read_version(&core.executable)?;
            BrowserCore {
                name: renamed_for(&stored, &core.name, &core.executable, major),
                version,
                major,
                ..core
            }
        } else {
            BrowserCore {
                name: core.name.trim().to_string(),
                ..core
            }
        };

        self.cores.save(&updated)?;
        Ok(updated)
    }

    fn redetect(&self, id: CoreId) -> Result<BrowserCore, AppError> {
        let stored = self.stored(id)?;
        let (version, major) = self.read_version(&stored.executable)?;

        // A name the user chose stays; an auto-generated one follows the major.
        let name = if stored.has_suggested_name() {
            domain::suggested_name(&stored.executable, major)
        } else {
            stored.name.clone()
        };

        let refreshed = BrowserCore {
            name,
            version,
            major,
            ..stored
        };
        self.cores.save(&refreshed)?;
        Ok(refreshed)
    }

    fn delete(&self, id: CoreId) -> Result<(), AppError> {
        let core = self.stored(id)?;
        let users = self.users_of(id)?;
        if !users.is_empty() {
            return Err(AppError::Conflict(format!(
                "{} is used by {}; point {} at another core first",
                core.name,
                users.join(", "),
                if users.len() == 1 { "it" } else { "them" },
            )));
        }
        self.cores.delete(id)?;
        Ok(())
    }

    fn insert(&self, core: BrowserCore) -> Result<(), AppError> {
        validate_core(&core)?;
        self.cores.save(&core)?;
        Ok(())
    }

    fn get(&self, id: CoreId) -> Result<Option<BrowserCore>, AppError> {
        Ok(self.cores.get(id)?)
    }

    fn list(&self) -> Result<Vec<BrowserCore>, AppError> {
        Ok(self.cores.list()?)
    }

    fn usage(&self) -> Result<HashMap<CoreId, Vec<String>>, AppError> {
        let mut usage: HashMap<CoreId, Vec<String>> = HashMap::new();
        for profile in self.profiles.list()? {
            usage.entry(profile.core_id).or_default().push(profile.name);
        }
        for names in usage.values_mut() {
            names.sort();
        }
        Ok(usage)
    }
}

/// What to call a core that is now pointed at `executable`.
///
/// Three cases, in order: a name the user typed in this save wins; otherwise an
/// auto-generated name follows the new binary and its major, because leaving
/// "chrome 148" on a 128 build is a claim the core cannot back; otherwise the
/// name the user chose earlier is kept.
fn renamed_for(stored: &BrowserCore, submitted: &str, executable: &Path, major: u32) -> String {
    let submitted = submitted.trim();
    if !submitted.is_empty() && submitted != stored.name {
        return submitted.to_string();
    }
    if submitted.is_empty() || stored.has_suggested_name() {
        return domain::suggested_name(executable, major);
    }
    stored.name.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        FingerprintProfile, ProfileId, ProxyId, StartTarget, WindowProfile, suggested_name,
    };
    use std::sync::Mutex;
    use storage::{MemCoreRepository, MemProfileRepository};

    struct Fixture {
        service: DefaultCoreService,
        cores: Arc<MemCoreRepository>,
        profiles: Arc<MemProfileRepository>,
        /// Every path the probe was asked about, in order.
        probed: Arc<Mutex<Vec<PathBuf>>>,
    }

    fn fixture_with(probe: impl Fn(&Path) -> VersionReport + Send + Sync + 'static) -> Fixture {
        let cores = Arc::new(MemCoreRepository::new());
        let profiles = Arc::new(MemProfileRepository::new());
        let probed = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&probed);
        let service = DefaultCoreService::with_probe(
            Arc::clone(&cores) as Arc<dyn CoreRepository>,
            Arc::clone(&profiles) as Arc<dyn ProfileRepository>,
            Box::new(move |path| {
                seen.lock().expect("probe log").push(path.to_path_buf());
                probe(path)
            }),
        );
        Fixture {
            service,
            cores,
            profiles,
            probed,
        }
    }

    /// A stand-in browser binary on disk.
    ///
    /// The file holds the banner the probe should read, so a test can also
    /// replace the file behind a path and make the answer change - which is the
    /// situation `redetect` exists for. The directory goes away with the guard:
    /// a run used to leave one behind per binary.
    struct TempBinary {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempBinary {
        fn new(dir: &str, banner: Option<&str>) -> Self {
            let dir = std::env::temp_dir().join(format!("fp-core-service-{dir}"));
            std::fs::create_dir_all(&dir).expect("temp dir");
            let path = dir.join("chrome");
            Self::write(&path, banner);
            Self { dir, path }
        }

        fn path(&self) -> &Path {
            &self.path
        }

        fn path_buf(&self) -> PathBuf {
            self.path.clone()
        }

        /// Replaces the file behind the same path, as a reinstall would.
        fn replace(&self, banner: Option<&str>) {
            Self::write(&self.path, banner);
        }

        /// Removes the binary, as an uninstall would.
        fn remove(&self) {
            std::fs::remove_file(&self.path).expect("remove binary");
        }

        fn write(path: &Path, banner: Option<&str>) {
            std::fs::write(path, banner.unwrap_or("").as_bytes()).expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
        }
    }

    impl Drop for TempBinary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn probe_from_file() -> impl Fn(&Path) -> VersionReport + Send + Sync {
        |path: &Path| {
            let banner = std::fs::read_to_string(path).unwrap_or_default();
            let banner = banner.trim();
            VersionReport::from_banner((!banner.is_empty()).then(|| banner.to_string()))
        }
    }

    fn fixture() -> Fixture {
        fixture_with(probe_from_file())
    }

    fn profile(core_id: CoreId, name: &str) -> domain::BrowserProfile {
        domain::BrowserProfile {
            id: ProfileId::new(),
            name: name.to_string(),
            core_id,
            user_data_dir: PathBuf::from("/tmp/none"),
            fingerprint: FingerprintProfile::new_random(7),
            proxy_id: None::<ProxyId>,
            window: WindowProfile::new(1280, 800),
            start_target: StartTarget::Blank,
        }
    }

    #[test]
    fn adding_a_core_stores_what_the_binary_reported() {
        let fixture = fixture();
        let binary = TempBinary::new("add", Some("Chromium 148.0.7778.215"));

        let core = fixture.service.add(None, binary.path_buf()).expect("add");

        assert_eq!(core.version, "Chromium 148.0.7778.215");
        assert_eq!(core.major, 148);
        assert_eq!(core.name, suggested_name(binary.path(), 148));
        assert_eq!(fixture.probed.lock().expect("log").len(), 1, "probed once");
        assert_eq!(fixture.cores.list().expect("list").len(), 1);
    }

    #[test]
    fn a_chosen_name_is_kept() {
        let fixture = fixture();
        let core = fixture
            .service
            .add(
                Some("  Work browser  ".to_string()),
                TempBinary::new("named", Some("Chromium 148.0.7778.215")).path_buf(),
            )
            .expect("add");
        assert_eq!(core.name, "Work browser");
    }

    #[test]
    fn a_binary_without_a_version_is_refused_with_the_way_out() {
        let fixture = fixture();
        let binary = TempBinary::new("silent", None);

        let error = fixture
            .service
            .add(None, binary.path_buf())
            .expect_err("no version");
        let message = error.to_string();
        assert!(message.contains("--version"), "{message}");
        assert!(message.contains("FP_BROWSER_CHROMIUM_MAJOR"), "{message}");
        assert!(
            message.split("; ").count() == 2,
            "the window renders one clause per line: {message}"
        );
        assert!(fixture.cores.list().expect("list").is_empty());
    }

    #[test]
    fn major_zero_is_not_a_version() {
        let fixture =
            fixture_with(|_| VersionReport::from_banner(Some("Chromium 0.0.0".to_string())));
        let error = fixture
            .service
            .add(
                None,
                TempBinary::new("zero", Some("Chromium 0.0.0")).path_buf(),
            )
            .expect_err("major 0 is refused");
        assert!(
            error.to_string().contains("did not report a version"),
            "{error}"
        );
        assert!(fixture.cores.list().expect("list").is_empty());
    }

    #[test]
    fn a_path_that_is_not_a_file_is_refused_before_the_probe() {
        let fixture = fixture();
        let error = fixture
            .service
            .add(None, PathBuf::from("/definitely/not/here/chrome"))
            .expect_err("no file");
        assert!(error.to_string().contains("no file"), "{error}");
        assert!(
            fixture.probed.lock().expect("log").is_empty(),
            "nothing was spawned for a path that cannot exist"
        );
    }

    #[test]
    fn the_same_binary_is_not_registered_twice() {
        let fixture = fixture();
        let binary = TempBinary::new("twice", Some("Chromium 148.0.7778.215"));
        fixture.service.add(None, binary.path_buf()).expect("add");
        let error = fixture
            .service
            .add(None, binary.path_buf())
            .expect_err("duplicate");
        assert!(error.to_string().contains("already registered"), "{error}");
        assert_eq!(fixture.cores.list().expect("list").len(), 1);
    }

    #[test]
    fn renaming_a_core_does_not_probe_again() {
        let fixture = fixture();
        let binary = TempBinary::new("rename", Some("Chromium 148.0.7778.215"));
        let mut core = fixture.service.add(None, binary.path_buf()).expect("add");
        fixture.probed.lock().expect("log").clear();

        core.name = "Renamed".to_string();
        let saved = fixture.service.update(core).expect("update");

        assert_eq!(saved.name, "Renamed");
        assert_eq!(saved.major, 148, "the major is untouched");
        assert!(fixture.probed.lock().expect("log").is_empty());
    }

    /// A core pointed at another binary must not keep the old capability table.
    #[test]
    fn pointing_a_core_at_another_binary_re_reads_the_version() {
        let fixture = fixture();
        let binary = TempBinary::new("repoint", Some("Chromium 148.0.7778.215"));
        let mut core = fixture.service.add(None, binary.path_buf()).expect("add");
        assert_eq!(core.major, 148);

        fixture.probed.lock().expect("log").clear();
        let older = TempBinary::new("repoint-old", Some("Chromium 128.0.0.0"));
        core.executable = older.path_buf();
        let saved = fixture.service.update(core).expect("update");

        assert_eq!(saved.version, "Chromium 128.0.0.0");
        assert_eq!(saved.major, 128);
        assert!(
            saved.has_suggested_name(),
            "an auto-generated name follows the major: {}",
            saved.name
        );
        assert_eq!(fixture.probed.lock().expect("log").len(), 1);
    }

    #[test]
    fn pointing_a_core_at_a_silent_binary_is_refused_and_changes_nothing() {
        let fixture = fixture();
        let binary = TempBinary::new("repoint-silent", Some("Chromium 148.0.7778.215"));
        let mut core = fixture.service.add(None, binary.path_buf()).expect("add");
        // The guard is held: dropping it here would remove the binary and the
        // refusal would be about a missing file instead of about the silence.
        let silent = TempBinary::new("repoint-silent2", None);
        core.executable = silent.path_buf();

        assert!(fixture.service.update(core.clone()).is_err());
        assert_eq!(
            fixture.service.get(core.id).expect("get").expect("stored"),
            fixture.cores.get(core.id).expect("get").expect("stored"),
        );
        let stored = fixture.service.get(core.id).expect("get").expect("stored");
        assert_eq!(stored.major, 148, "the stored major survives the refusal");
        assert!(stored.executable.ends_with("chrome"));
    }

    /// The same save, three ways of naming: a name typed now, an auto name that
    /// has to follow the new binary, and a name chosen earlier.
    #[test]
    fn a_re_pointed_core_is_renamed_by_its_own_rule() {
        let path = PathBuf::from("/tools/old");
        let stored = BrowserCore {
            id: CoreId::new(),
            name: suggested_name(&PathBuf::from("/tools/chrome"), 148),
            executable: PathBuf::from("/tools/chrome"),
            version: "Chromium 148.0.0.0".to_string(),
            major: 148,
        };
        assert!(stored.has_suggested_name());

        // An auto-generated name follows the new binary and its major.
        assert_eq!(renamed_for(&stored, &stored.name, &path, 128), "old 128");
        // A name typed in this save wins.
        assert_eq!(renamed_for(&stored, "My build", &path, 128), "My build");
        // A blank submission on a core that was named by hand keeps that name.
        let chosen = BrowserCore {
            name: "Mine".to_string(),
            ..stored.clone()
        };
        assert!(!chosen.has_suggested_name());
        assert_eq!(renamed_for(&chosen, "Mine", &path, 128), "Mine");
        assert_eq!(renamed_for(&chosen, "  ", &path, 128), "old 128");
    }

    #[test]
    fn updating_a_core_that_is_gone_is_not_found() {
        let fixture = fixture();
        let ghost = BrowserCore {
            id: CoreId::new(),
            name: "Ghost".to_string(),
            executable: PathBuf::from("chrome"),
            version: "Chromium 148.0.0.0".to_string(),
            major: 148,
        };
        assert!(matches!(
            fixture.service.update(ghost),
            Err(AppError::NotFound(_))
        ));
    }

    #[test]
    fn redetecting_re_reads_a_replaced_binary() {
        let fixture = fixture();
        let binary = TempBinary::new("redetect", Some("Chromium 148.0.7778.215"));
        let core = fixture.service.add(None, binary.path_buf()).expect("add");

        // The file behind the path is replaced by another build.
        binary.replace(Some("Chromium 128.0.0.0"));

        let refreshed = fixture.service.redetect(core.id).expect("redetect");
        assert_eq!(refreshed.major, 128);
        assert_eq!(refreshed.version, "Chromium 128.0.0.0");
        assert_eq!(refreshed.id, core.id);
        assert!(refreshed.has_suggested_name());
    }

    #[test]
    fn redetecting_keeps_a_chosen_name() {
        let fixture = fixture();
        let binary = TempBinary::new("redetect-named", Some("Chromium 148.0.7778.215"));
        let core = fixture
            .service
            .add(Some("Mine".to_string()), binary.path_buf())
            .expect("add");

        let refreshed = fixture.service.redetect(core.id).expect("redetect");
        assert_eq!(refreshed.name, "Mine");
        assert_eq!(refreshed.major, 148);
    }

    #[test]
    fn redetecting_a_missing_binary_is_refused_and_keeps_the_record() {
        let fixture = fixture();
        let binary = TempBinary::new("redetect-gone", Some("Chromium 148.0.7778.215"));
        let core = fixture.service.add(None, binary.path_buf()).expect("add");
        binary.remove();

        let error = fixture.service.redetect(core.id).expect_err("refused");
        assert!(error.to_string().contains("no file"), "{error}");
        assert_eq!(
            fixture
                .service
                .get(core.id)
                .expect("get")
                .expect("stored")
                .major,
            148
        );
    }

    #[test]
    fn a_core_in_use_cannot_be_deleted() {
        let fixture = fixture();
        let binary = TempBinary::new("inuse", Some("Chromium 148.0.7778.215"));
        let core = fixture.service.add(None, binary.path_buf()).expect("add");
        fixture
            .profiles
            .insert(&profile(core.id, "Profile 1"))
            .expect("insert");

        let error = fixture.service.delete(core.id).expect_err("refused");
        let message = error.to_string();
        assert!(message.contains(core.name.as_str()), "{message}");
        assert!(message.contains("Profile 1"), "{message}");
        assert!(fixture.service.get(core.id).expect("get").is_some());
    }

    #[test]
    fn an_unused_core_is_deleted() {
        let fixture = fixture();
        let binary = TempBinary::new("unused", Some("Chromium 148.0.7778.215"));
        let core = fixture.service.add(None, binary.path_buf()).expect("add");
        fixture.service.delete(core.id).expect("delete");
        assert!(fixture.service.list().expect("list").is_empty());
    }

    #[test]
    fn usage_names_the_profiles_per_core() {
        let fixture = fixture();
        let used_binary = TempBinary::new("usage", Some("Chromium 148.0.7778.215"));
        let used = fixture
            .service
            .add(None, used_binary.path_buf())
            .expect("add");
        let idle_binary = TempBinary::new("usage2", Some("Chromium 128.0.0.0"));
        let idle = fixture
            .service
            .add(None, idle_binary.path_buf())
            .expect("add");
        fixture
            .profiles
            .insert(&profile(used.id, "Profile 2"))
            .expect("insert");
        fixture
            .profiles
            .insert(&profile(used.id, "Profile 1"))
            .expect("insert");

        let usage = fixture.service.usage().expect("usage");
        assert_eq!(
            usage.get(&used.id).cloned().unwrap_or_default(),
            vec!["Profile 1".to_string(), "Profile 2".to_string()]
        );
        assert!(!usage.contains_key(&idle.id));
    }
}
