//! View-facing application state.
//!
//! [`AppState`] is intentionally free of GPUI types so it can be unit tested
//! without a window. The view layer renders it and forwards user actions back
//! into it. Runtime state is never owned here: every read goes through
//! [`RuntimeService::snapshot`], which is the documented reconciliation path.

use application::{AppError, NewProfile, ProfileService, RuntimeService};
use domain::{BrowserProfile, CoreId, ProfileId, ProxyId, RuntimeState};
use runtime::RuntimeSnapshot;
use std::collections::HashMap;
use std::sync::Arc;
use storage::{CoreRepository, ProxyRepository};

/// A user-facing message shown in the window banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub error: bool,
    pub message: String,
}

impl Notice {
    fn error(message: impl Into<String>) -> Self {
        Self {
            error: true,
            message: message.into(),
        }
    }

    fn info(message: impl Into<String>) -> Self {
        Self {
            error: false,
            message: message.into(),
        }
    }
}

/// One row of the profiles list with the display names already resolved.
#[derive(Clone)]
pub struct ProfileRow {
    pub profile: BrowserProfile,
    pub core_name: String,
    pub proxy_name: Option<String>,
    pub snapshot: Option<RuntimeSnapshot>,
}

impl ProfileRow {
    /// Snapshot state, or `Stopped` when the profile was never started.
    pub fn state(&self) -> RuntimeState {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.state.clone())
            .unwrap_or(RuntimeState::Stopped)
    }

    pub fn state_label(&self) -> &'static str {
        match self.state() {
            RuntimeState::Stopped => "Stopped",
            RuntimeState::Starting => "Starting",
            RuntimeState::Running => "Running",
            RuntimeState::Stopping => "Stopping",
            RuntimeState::Failed { .. } => "Failed",
            RuntimeState::Crashed { .. } => "Crashed",
        }
    }

    /// A failure message carried by the state itself, if any.
    pub fn state_message(&self) -> Option<&str> {
        match self.snapshot.as_ref().map(|snapshot| &snapshot.state) {
            Some(RuntimeState::Failed { message } | RuntimeState::Crashed { message }) => {
                Some(message.as_str())
            }
            _ => None,
        }
    }

    pub fn can_start(&self) -> bool {
        matches!(
            self.state(),
            RuntimeState::Stopped | RuntimeState::Failed { .. } | RuntimeState::Crashed { .. }
        )
    }

    pub fn can_stop(&self) -> bool {
        self.state().is_active()
    }

    /// Restart is rejected by the supervisor only while another transition owns
    /// the profile, so mirror exactly that rule here.
    pub fn can_restart(&self) -> bool {
        !matches!(
            self.state(),
            RuntimeState::Starting | RuntimeState::Stopping
        )
    }

    pub fn last_error(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.last_error.as_deref())
    }

    pub fn last_warning(&self) -> Option<&str> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.last_warning.as_deref())
    }

    pub fn browser_pid(&self) -> Option<u32> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.browser_pid)
    }

    pub fn xray_pid(&self) -> Option<u32> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.xray_pid)
    }

    pub fn cdp_port(&self) -> Option<u16> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.cdp_port)
    }

    pub fn socks_port(&self) -> Option<u16> {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.socks_port)
    }

    pub fn effective_args(&self) -> &[String] {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.effective_args.as_slice())
            .unwrap_or(&[])
    }

    pub fn dropped_events(&self) -> u64 {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.dropped_events)
            .unwrap_or(0)
    }
}

pub struct AppState {
    profiles: Arc<dyn ProfileService>,
    runtime: Arc<RuntimeService>,
    cores: Arc<dyn CoreRepository>,
    proxies: Arc<dyn ProxyRepository>,
    rows: Vec<ProfileRow>,
    selected: Option<ProfileId>,
    notice: Option<Notice>,
}

impl AppState {
    pub fn new(
        profiles: Arc<dyn ProfileService>,
        runtime: Arc<RuntimeService>,
        cores: Arc<dyn CoreRepository>,
        proxies: Arc<dyn ProxyRepository>,
    ) -> Self {
        Self {
            profiles,
            runtime,
            cores,
            proxies,
            rows: Vec::new(),
            selected: None,
            notice: None,
        }
    }

    /// Reload profiles from storage and reconcile every runtime snapshot.
    ///
    /// Failures are recorded in the banner as well as returned, because every
    /// caller in the UI wants them shown.
    pub fn load(&mut self) -> Result<(), AppError> {
        let result = self.load_rows();
        if let Err(error) = &result {
            self.notice = Some(Notice::error(error.to_string()));
        }
        result
    }

    fn load_rows(&mut self) -> Result<(), AppError> {
        let runtime = Arc::clone(&self.runtime);
        let cores: HashMap<CoreId, String> = self
            .cores
            .list()?
            .into_iter()
            .map(|core| (core.id, core.name))
            .collect();
        let proxies: HashMap<ProxyId, String> = self
            .proxies
            .list()?
            .into_iter()
            .map(|proxy| (proxy.id, proxy.name))
            .collect();

        let mut profiles = self.profiles.list()?;
        profiles.sort_by_key(|profile| profile.name.to_lowercase());

        self.rows = profiles
            .into_iter()
            .map(|profile| {
                let core_name = cores
                    .get(&profile.core_id)
                    .cloned()
                    .unwrap_or_else(|| "(missing core)".to_string());
                let proxy_name = profile.proxy_id.and_then(|id| proxies.get(&id).cloned());
                let snapshot = runtime.snapshot(profile.id);
                ProfileRow {
                    profile,
                    core_name,
                    proxy_name,
                    snapshot,
                }
            })
            .collect();

        if let Some(selected) = self.selected
            && !self.rows.iter().any(|row| row.profile.id == selected)
        {
            self.selected = None;
        }

        Ok(())
    }

    /// Reconcile snapshots only; used by the periodic tick and after commands.
    pub fn refresh_runtime(&mut self) {
        let runtime = Arc::clone(&self.runtime);
        for row in &mut self.rows {
            row.snapshot = runtime.snapshot(row.profile.id);
        }
    }

    pub fn rows(&self) -> &[ProfileRow] {
        &self.rows
    }

    pub fn selected_id(&self) -> Option<ProfileId> {
        self.selected
    }

    pub fn selected(&self) -> Option<&ProfileRow> {
        let selected = self.selected?;
        self.rows.iter().find(|row| row.profile.id == selected)
    }

    pub fn select(&mut self, id: ProfileId) {
        self.selected = Some(id);
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// Surface a bootstrap-level message (core discovery, version detection).
    pub fn push_notice(&mut self, message: impl Into<String>, error: bool) {
        self.notice = Some(if error {
            Notice::error(message)
        } else {
            Notice::info(message)
        });
    }

    pub fn dismiss_notice(&mut self) {
        self.notice = None;
    }

    /// Placeholder naming until the Profile Editor page exists.
    pub fn next_profile_name(&self) -> String {
        format!("Profile {}", self.rows.len() + 1)
    }

    pub fn has_core(&self) -> bool {
        self.cores
            .list()
            .map(|cores| !cores.is_empty())
            .unwrap_or(false)
    }

    fn default_core_id(&self) -> Result<CoreId, AppError> {
        self.cores
            .list()?
            .first()
            .map(|core| core.id)
            .ok_or_else(|| {
                AppError::Conflict(
                    "no browser core configured; set FP_BROWSER_CHROMIUM_BIN and restart"
                        .to_string(),
                )
            })
    }

    pub fn create_profile(&mut self, name: &str) -> Result<ProfileId, AppError> {
        let core_id = self.core_id()?;
        let draft = NewProfile {
            name: name.trim().to_string(),
            core_id,
            user_data_dir: None,
            fingerprint: None,
            proxy_id: None,
            window: None,
            start_target: None,
        };

        let profile = self.record(self.profiles.create(draft))?;
        let id = profile.id;
        self.load()?;
        self.selected = Some(id);
        self.notice = Some(Notice::info(format!("Created {}", profile.name)));
        Ok(id)
    }

    pub fn start(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.record(self.runtime.start(id))?;
        self.refresh_runtime();
        Ok(())
    }

    pub fn stop(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.record(self.runtime.stop(id))?;
        self.refresh_runtime();
        Ok(())
    }

    pub fn restart(&mut self, id: ProfileId) -> Result<(), AppError> {
        self.record(self.runtime.restart(id))?;
        self.refresh_runtime();
        Ok(())
    }

    fn core_id(&mut self) -> Result<CoreId, AppError> {
        let resolved = self.default_core_id();
        self.record(resolved)
    }

    /// Record a failure in the banner, and hand it back to the caller.
    fn record<T>(&mut self, result: Result<T, AppError>) -> Result<T, AppError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => {
                self.notice = Some(Notice::error(error.to_string()));
                Err(error)
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use domain::{ProfileId, RuntimeState};
    use runtime::{RuntimeCommandError, RuntimeFacade, RuntimeSnapshot, StartParams};
    use std::collections::HashMap;
    use std::sync::{Mutex, RwLock};

    /// Synchronous stand-in for the supervisor channel façade.
    ///
    /// Commands take effect immediately so tests assert the UI contract without
    /// depending on process startup.
    #[derive(Default)]
    pub struct FakeRuntime {
        snapshots: RwLock<HashMap<ProfileId, RuntimeSnapshot>>,
        pub commands: Mutex<Vec<String>>,
    }

    impl FakeRuntime {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn set_state(&self, id: ProfileId, state: RuntimeState) {
            let mut snapshots = self.snapshots.write().expect("snapshot lock");
            let snapshot = snapshots
                .entry(id)
                .or_insert_with(|| snapshot(id, RuntimeState::Stopped));
            snapshot.state = state;
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
            self.set_state(id, RuntimeState::Running);
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
            self.set_state(id, RuntimeState::Running);
            self.record(&format!("restart:{id}"));
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

    pub fn core(id: domain::CoreId) -> domain::BrowserCore {
        domain::BrowserCore {
            id,
            name: "Test Core 144".to_string(),
            executable: std::path::PathBuf::from("chrome"),
            version: "144.0.0.0".to_string(),
            major: 144,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::testing::{FakeRuntime, core};
    use application::DefaultProfileService;
    use domain::CoreId;
    use std::path::PathBuf;
    use storage::{
        MemCoreRepository, MemProfileRepository, MemProxyRepository, ProfileRepository as _,
    };

    struct Fixture {
        state: AppState,
        runtime: Arc<FakeRuntime>,
        cores: Arc<MemCoreRepository>,
        profiles: Arc<MemProfileRepository>,
    }

    fn fixture() -> Fixture {
        let profile_repo: Arc<MemProfileRepository> = Arc::new(MemProfileRepository::new());
        let core_repo: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
        let proxy_repo: Arc<MemProxyRepository> = Arc::new(MemProxyRepository::new());
        let runtime = Arc::new(FakeRuntime::new());

        let service = Arc::new(DefaultProfileService::new(
            profile_repo.clone(),
            PathBuf::from("data"),
        ));
        let runtime_service = Arc::new(RuntimeService::new(
            profile_repo.clone(),
            core_repo.clone(),
            proxy_repo.clone(),
            runtime.clone(),
        ));

        let state = AppState::new(
            service,
            runtime_service,
            core_repo.clone(),
            proxy_repo.clone(),
        );

        Fixture {
            state,
            runtime,
            cores: core_repo,
            profiles: profile_repo,
        }
    }

    fn seed_core(fixture: &Fixture) -> CoreId {
        let core = core(CoreId::new());
        fixture.cores.save(&core).expect("save core");
        core.id
    }

    #[test]
    fn create_lists_the_profile_and_selects_it() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("initial load");

        let id = fixture.state.create_profile("Primary").expect("create");

        assert_eq!(fixture.state.rows().len(), 1);
        let row = &fixture.state.rows()[0];
        assert_eq!(row.profile.name, "Primary");
        assert_eq!(row.profile.id, id);
        assert_eq!(row.state(), RuntimeState::Stopped);
        assert_eq!(row.state_label(), "Stopped");
        assert_eq!(row.core_name, "Test Core 144");
        assert_eq!(fixture.state.selected_id(), Some(id));
        assert!(row.can_start());
        assert!(!row.can_stop());
    }

    #[test]
    fn create_without_a_core_records_an_error_notice() {
        let mut fixture = fixture();
        fixture.state.load().expect("initial load");

        let error = fixture
            .state
            .create_profile("Primary")
            .expect_err("must fail");

        assert!(matches!(error, AppError::Conflict(_)));
        assert!(fixture.state.rows().is_empty());
        let notice = fixture.state.notice().expect("notice");
        assert!(notice.error);
        assert!(notice.message.contains("no browser core"));
    }

    #[test]
    fn start_and_stop_update_the_row_from_the_snapshot() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Primary").expect("create");

        fixture.state.start(id).expect("start");
        let row = fixture.state.selected().expect("selected row");
        assert_eq!(row.state(), RuntimeState::Running);
        assert!(row.can_stop());
        assert!(!row.can_start());

        fixture.state.stop(id).expect("stop");
        assert_eq!(
            fixture.state.selected().expect("selected row").state(),
            RuntimeState::Stopped
        );

        let commands = fixture.runtime.commands.lock().expect("commands");
        assert_eq!(
            commands.as_slice(),
            [format!("start:{id}"), format!("stop:{id}")]
        );
    }

    #[test]
    fn starting_an_unknown_profile_records_an_error_notice() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");

        let unknown = ProfileId::new();
        let error = fixture.state.start(unknown).expect_err("must fail");

        assert!(matches!(error, AppError::NotFound(_)));
        assert!(fixture.state.notice().expect("notice").error);
    }

    #[test]
    fn refresh_runtime_picks_up_external_state_changes() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Primary").expect("create");

        // Simulate a supervisor-side crash that the UI never received as an event.
        fixture.runtime.set_state(
            id,
            RuntimeState::Crashed {
                message: "browser exited".to_string(),
            },
        );
        fixture.state.refresh_runtime();

        let row = fixture.state.selected().expect("selected row");
        assert_eq!(row.state_label(), "Crashed");
        assert_eq!(row.state_message(), Some("browser exited"));
        assert!(row.can_start());
    }

    #[test]
    fn load_drops_a_selection_whose_profile_was_deleted() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");
        let id = fixture.state.create_profile("Primary").expect("create");

        fixture.profiles.delete(id).expect("delete");
        fixture.state.load().expect("reload");

        assert!(fixture.state.rows().is_empty());
        assert_eq!(fixture.state.selected_id(), None);
    }

    #[test]
    fn rows_are_sorted_by_name_and_renumbered_for_new_profiles() {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().expect("load");

        assert_eq!(fixture.state.next_profile_name(), "Profile 1");
        fixture.state.create_profile("Bravo").expect("create");
        assert_eq!(fixture.state.next_profile_name(), "Profile 2");
        fixture.state.create_profile("Alpha").expect("create");

        let names: Vec<&str> = fixture
            .state
            .rows()
            .iter()
            .map(|row| row.profile.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "Bravo"]);
    }
}
