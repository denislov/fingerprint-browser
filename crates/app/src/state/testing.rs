use domain::{ProfileId, RuntimeState};
use runtime::{RuntimeCommandError, RuntimeFacade, RuntimeSnapshot, StartParams};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use storage::{MemCoreRepository, MemProfileRepository};

/// Synchronous stand-in for the supervisor channel façade.
///
/// Commands take effect immediately so tests assert the UI contract without
/// depending on process startup.
#[derive(Default)]
pub struct FakeRuntime {
    snapshots: RwLock<HashMap<ProfileId, RuntimeSnapshot>>,
    pub commands: Mutex<Vec<String>>,
    /// Whether a start publishes the state it produces.
    ///
    /// A real runtime answers a start a tick after the command is queued, and
    /// the ticket's window between the two is what a start's lease covers. A
    /// test that needs that window turns the answer off and leaves the
    /// snapshot stopped.
    silent: std::sync::atomic::AtomicBool,
}

impl FakeRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stops publishing the state of a start, so the snapshot stays stopped:
    /// the command is queued and nothing has answered.
    pub fn answer_nothing(&self) {
        self.silent.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn set_state(&self, id: ProfileId, state: RuntimeState) {
        let mut snapshots = self.snapshots.write().expect("snapshot lock");
        let snapshot = snapshots
            .entry(id)
            .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
        snapshot.state = state;
    }

    /// Publishes the debug port a running browser exposes.
    pub fn set_cdp_port(&self, id: ProfileId, port: u16) {
        let mut snapshots = self.snapshots.write().expect("snapshot lock");
        let snapshot = snapshots
            .entry(id)
            .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
        snapshot.cdp_port = Some(port);
    }

    /// Publishes the loopback SOCKS port a running engine listens on.
    pub fn set_socks_port(&self, id: ProfileId, port: u16) {
        let mut snapshots = self.snapshots.write().expect("snapshot lock");
        let snapshot = snapshots
            .entry(id)
            .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
        snapshot.socks_port = Some(port);
    }

    /// Publishes the launch line a running browser was started with.
    #[cfg(test)]
    pub fn set_args(&self, id: ProfileId, args: Vec<String>) {
        let mut snapshots = self.snapshots.write().expect("snapshot lock");
        let snapshot = snapshots
            .entry(id)
            .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
        snapshot.effective_args = args;
    }

    /// Publishes a diagnostic the way the supervisor's warning event does.
    pub fn set_warning(&self, id: ProfileId, message: &str) {
        let mut snapshots = self.snapshots.write().expect("snapshot lock");
        let snapshot = snapshots
            .entry(id)
            .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
        snapshot.last_warning = Some(message.to_string());
    }

    /// The snapshot a test wants to inspect without going through the UI.
    #[cfg(test)]
    pub fn snapshot_of(&self, id: ProfileId) -> Option<RuntimeSnapshot> {
        self.snapshot(id)
    }

    fn record(&self, command: &str) {
        self.commands
            .lock()
            .expect("command lock")
            .push(command.to_string());
    }
}

fn snapshot(id: ProfileId, state: RuntimeState) -> RuntimeSnapshot {
    RuntimeSnapshot {
        profile_id: id,
        state,
        browser_pid: None,
        xray_pid: None,
        cdp_port: None,
        socks_port: None,
        started_at: None,
        effective_args: Vec::new(),
        last_error: None,
        last_warning: None,
        dropped_events: 0,
    }
}

impl RuntimeFacade for FakeRuntime {
    fn start(&self, params: StartParams) -> Result<(), RuntimeCommandError> {
        let id = params.profile_id();
        if !self.silent.load(std::sync::atomic::Ordering::SeqCst) {
            self.set_state(id, RuntimeState::Running);
        }
        self.record(&format!("start:{id}"));
        Ok(())
    }

    fn stop(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError> {
        self.set_state(profile_id, RuntimeState::Stopped);
        self.record(&format!("stop:{profile_id}"));
        Ok(())
    }

    fn restart(&self, params: StartParams) -> Result<(), RuntimeCommandError> {
        let id = params.profile_id();
        if !self.silent.load(std::sync::atomic::Ordering::SeqCst) {
            self.set_state(id, RuntimeState::Running);
        }
        self.record(&format!("restart:{id}"));
        Ok(())
    }

    fn release_all(&self) -> Result<(), RuntimeCommandError> {
        self.record("release-all");
        Ok(())
    }

    fn snapshot(&self, profile_id: ProfileId) -> Option<RuntimeSnapshot> {
        self.snapshots
            .read()
            .expect("snapshot lock")
            .get(&profile_id)
            .cloned()
    }
}

/// The core service over in-memory storage, with a probe that reads the
/// banner out of the file instead of spawning anything.
///
/// Tests that need a specific version write the banner into the file they
/// pass to `add`.
pub fn core_service(
    cores: Arc<MemCoreRepository>,
    profiles: Arc<MemProfileRepository>,
) -> Arc<dyn application::CoreService> {
    Arc::new(application::DefaultCoreService::with_probe(
        cores,
        profiles,
        Box::new(|path: &std::path::Path| {
            let banner = std::fs::read_to_string(path).unwrap_or_default();
            let banner = banner.trim();
            runtime::version::VersionReport::from_banner(
                (!banner.is_empty()).then(|| banner.to_string()),
            )
        }),
    ))
}

/// Settings for a test: nothing set, in a config file under the temp dir.
pub fn settings() -> crate::settings::Settings {
    settings_at(
        &std::env::temp_dir()
            .join("fp-app-settings-test")
            .join("config.json"),
    )
}

/// Settings read from a specific config file, for a test that saves one.
pub fn settings_at(config: &std::path::Path) -> crate::settings::Settings {
    let (settings, _) = crate::settings::Settings::load(crate::settings::Environment {
        config: Some(config.to_string_lossy().to_string()),
        ..crate::settings::Environment::default()
    });
    settings
}

/// A browser binary on disk: the file holds the version it reports.
///
/// The directory goes away with the guard, so a test run leaves nothing
/// behind.
pub struct CoreBinary {
    dir: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl CoreBinary {
    pub fn new(dir: &str, banner: Option<&str>) -> Self {
        let dir = std::env::temp_dir().join(format!("fp-app-core-{dir}"));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("chrome");
        Self::write(&path, banner);
        Self { dir, path }
    }

    pub fn path_buf(&self) -> std::path::PathBuf {
        self.path.clone()
    }

    /// Replaces the file behind the same path, as a reinstall would.
    pub fn replace(&self, banner: Option<&str>) {
        Self::write(&self.path, banner);
    }

    fn write(path: &std::path::Path, banner: Option<&str>) {
        std::fs::write(path, banner.unwrap_or("")).expect("write binary");
    }
}

impl Drop for CoreBinary {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn core(id: domain::CoreId) -> domain::BrowserCore {
    domain::BrowserCore {
        id,
        name: "Test Core 144".to_string(),
        executable: std::path::PathBuf::from("chrome"),
        version: "144.0.0.0".to_string(),
        major: 144,
    }
}
