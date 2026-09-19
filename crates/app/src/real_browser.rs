//! Opt-in acceptance for the exact path the window uses:
//! `AppState` -> `RuntimeService` -> supervisor channel -> real Chromium.
//!
//! ```sh
//! CHROMIUM_BIN=/absolute/path/to/chrome cargo test -p app real_chromium -- --ignored --nocapture
//! ```
//!
//! This is deliberately headless: it proves the wiring behind the buttons, not
//! the pixels. The window itself is covered by the headless UI test in `ui.rs`.

use crate::state::AppState;
use application::{DefaultProfileService, ProfileService, RuntimeService};
use domain::{BrowserCore, CoreCapabilities, CoreId, LaunchPlan, ProfileId, RuntimeState};
use runtime::{
    ChannelRuntimeFacade, DefaultLaunchPlanner, LaunchContext, LaunchPlanError, LaunchPlanner,
    RuntimeCommand, RuntimeFacade, RuntimeSupervisor, RuntimeSupervisorChannels,
    SupervisorComponents,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use storage::{CoreRepository, ProfileRepository, ProxyRepository, SqliteStorage};

const START_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(15);

struct HeadlessPlanner;

impl LaunchPlanner for HeadlessPlanner {
    fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
        let mut plan = DefaultLaunchPlanner.build(ctx)?;
        for flag in [
            "--headless",
            "--disable-gpu",
            "--disable-background-networking",
        ] {
            plan.browser_args.insert(0, flag.into());
        }
        Ok(plan)
    }
}

struct Harness {
    state: AppState,
    command_tx: crossbeam_channel::Sender<RuntimeCommand>,
    supervisor: Option<std::thread::JoinHandle<()>>,
    /// Detected major of `CHROMIUM_BIN`, which selects the capability table.
    major: u32,
    dir: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let executable: PathBuf = std::env::var_os("CHROMIUM_BIN")
            .expect("set CHROMIUM_BIN")
            .into();
        let detected =
            runtime::version::VersionReport::probe(&executable, runtime::DEFAULT_VERSION_TIMEOUT);
        let major = detected.major.expect("CHROMIUM_BIN must report a version");

        let dir = std::env::temp_dir().join(format!("fp-app-acceptance-{}", ProfileId::new()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let storage = SqliteStorage::open(dir.join("app.db")).expect("open sqlite storage");

        let profile_repo: Arc<dyn ProfileRepository> = Arc::new(storage.profiles());
        let core_repo: Arc<dyn CoreRepository> = Arc::new(storage.cores());
        let proxy_repo: Arc<dyn ProxyRepository> = Arc::new(storage.proxies());

        let core = BrowserCore {
            id: CoreId::new(),
            name: format!("chromium {major}"),
            executable,
            version: detected
                .banner
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            major,
        };
        core_repo.save(&core).expect("save core");

        let channels = RuntimeSupervisorChannels::new(128);
        let snapshots = Arc::new(RwLock::new(HashMap::new()));
        let supervisor = RuntimeSupervisor::with_components(
            channels.command_rx,
            channels.event_tx,
            Arc::clone(&snapshots),
            SupervisorComponents {
                planner: Box::new(HeadlessPlanner),
                runtime_dir: dir.join("runtime"),
                ..Default::default()
            },
        );

        let facade: Arc<dyn RuntimeFacade> = Arc::new(ChannelRuntimeFacade::new(
            channels.command_tx.clone(),
            snapshots,
        ));
        let runtime_service = Arc::new(RuntimeService::new(
            Arc::clone(&profile_repo),
            Arc::clone(&core_repo),
            Arc::clone(&proxy_repo),
            facade,
        ));
        let profiles: Arc<dyn ProfileService> = Arc::new(DefaultProfileService::new(
            Arc::clone(&profile_repo),
            dir.clone(),
        ));

        let proxy_service: Arc<dyn application::ProxyService> =
            Arc::new(application::DefaultProxyService::new(
                Arc::clone(&proxy_repo),
                Arc::clone(&profile_repo),
            ));
        let cores = application::DefaultCoreService::new(core_repo, Arc::clone(&profile_repo));
        let mut state = AppState::new(profiles, runtime_service, Arc::new(cores), proxy_service);
        state.load().expect("initial load");

        Self {
            state,
            command_tx: channels.command_tx,
            supervisor: Some(supervisor.spawn()),
            major,
            dir,
        }
    }

    fn wait_for(&mut self, id: ProfileId, expected: RuntimeState) -> ProfileRowSnapshot {
        let deadline = Instant::now() + START_TIMEOUT.max(STOP_TIMEOUT);
        loop {
            self.state.refresh_runtime();
            let row = self
                .state
                .rows()
                .iter()
                .find(|row| row.profile.id == id)
                .expect("profile row");
            assert!(
                !matches!(row.state(), RuntimeState::Failed { .. }),
                "transition failed: {:?}",
                row.snapshot.as_ref().map(|s| s.last_error.clone())
            );
            if row.state() == expected {
                return ProfileRowSnapshot {
                    browser_pid: row.browser_pid(),
                    cdp_port: row.cdp_port(),
                    args: row.effective_args().to_vec(),
                    warning: row.last_warning().map(str::to_string),
                    data_dir: row.profile.user_data_dir.clone(),
                };
            }
            assert!(
                Instant::now() < deadline,
                "waiting for {expected:?}, still {:?}",
                row.state()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.command_tx.send(RuntimeCommand::ShutdownAll);
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct ProfileRowSnapshot {
    browser_pid: Option<u32>,
    cdp_port: Option<u16>,
    args: Vec<String>,
    warning: Option<String>,
    data_dir: PathBuf,
}

#[test]
#[ignore = "requires CHROMIUM_BIN pointing to a real Chromium"]
fn real_chromium_start_stop_through_app_state() {
    let mut harness = Harness::new();

    let first = harness
        .state
        .create_profile("Acceptance A")
        .expect("create A");
    let second = harness
        .state
        .create_profile("Acceptance B")
        .expect("create B");
    assert_eq!(harness.state.rows().len(), 2);

    harness.state.start(first).expect("start A");
    harness.state.start(second).expect("start B");

    let a = harness.wait_for(first, RuntimeState::Running);
    let b = harness.wait_for(second, RuntimeState::Running);

    // Both sessions are live at once, with their own process and CDP port.
    assert!(a.browser_pid.is_some() && b.browser_pid.is_some());
    assert_ne!(a.browser_pid, b.browser_pid);
    assert_ne!(a.cdp_port, b.cdp_port);
    assert_ne!(a.data_dir, b.data_dir);
    assert!(a.data_dir.is_dir() && b.data_dir.is_dir());
    for pid in [a.browser_pid, b.browser_pid].into_iter().flatten() {
        assert!(
            std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "browser {pid} reported by the snapshot is not a live process"
        );
    }
    println!(
        "A pid {:?} cdp {:?} | B pid {:?} cdp {:?}",
        a.browser_pid, a.cdp_port, b.browser_pid, b.cdp_port
    );

    // The fingerprint switches the UI reports are the ones actually launched.
    let seed_a = harness.state.rows()[0].profile.fingerprint.seed;
    let seed_b = harness.state.rows()[1].profile.fingerprint.seed;
    assert_ne!(seed_a, seed_b, "each profile owns a distinct seed");
    assert!(
        a.args
            .iter()
            .any(|arg| arg == &format!("--fingerprint={seed_a}")),
        "effective args carry profile A's seed: {a:?}",
        a = a.args
    );
    assert!(
        b.args
            .iter()
            .any(|arg| arg == &format!("--fingerprint={seed_b}")),
        "effective args carry profile B's seed"
    );
    for (snapshot, seed) in [(&a, seed_a), (&b, seed_b)] {
        let fingerprints: Vec<&String> = snapshot
            .args
            .iter()
            .filter(|arg| arg.starts_with("--fingerprint="))
            .collect();
        assert_eq!(fingerprints.len(), 1, "exactly one seed switch per session");
        assert_eq!(fingerprints[0], &format!("--fingerprint={seed}"));
    }

    // The switch set follows the detected major, and anything the core cannot
    // honour is reported rather than dropped in silence.
    let capabilities = CoreCapabilities::for_major(harness.major);
    let noise = |snapshot: &ProfileRowSnapshot| {
        snapshot
            .args
            .iter()
            .any(|arg| arg == "--fingerprinting-canvas-image-data-noise")
    };
    match a.warning.as_deref() {
        None => {
            assert!(capabilities.is_verified(), "an unverified core must warn");
            assert!(
                noise(&a) && noise(&b),
                "the verified set carries canvas noise"
            );
        }
        Some(warning) => {
            assert!(
                !capabilities.is_verified(),
                "a verified core must not warn: {warning}"
            );
            assert!(
                warning.contains("verified fingerprint generation"),
                "unexpected warning: {warning}"
            );
            assert!(
                !noise(&a) && !noise(&b),
                "canvas noise is omitted for a legacy core"
            );
        }
    }

    // Stopping one profile leaves the other running.
    harness.state.stop(first).expect("stop A");
    let stopped = harness.wait_for(first, RuntimeState::Stopped);
    assert!(stopped.browser_pid.is_none());
    harness.wait_for(second, RuntimeState::Running);

    // Restart goes through the same state entry point as the button.
    harness.state.restart(first).expect("restart A");
    let restarted = harness.wait_for(first, RuntimeState::Running);
    assert!(restarted.browser_pid.is_some());
    assert_ne!(
        restarted.browser_pid, a.browser_pid,
        "restart is a new process"
    );

    harness.state.stop(second).expect("stop B");
    harness.wait_for(second, RuntimeState::Stopped);

    println!("restarted A pid {:?}", restarted.browser_pid);
}

#[test]
#[ignore = "requires CHROMIUM_BIN pointing to a real Chromium"]
fn real_chromium_fingerprint_verification_through_the_verifier() {
    use crate::verifier::{CdpFingerprintVerifier, FingerprintVerifier};
    use runtime::{CdpProbe as _, HttpCdpProbe};

    let mut harness = Harness::new();
    let id = harness.state.create_profile("Verify me").expect("create");
    harness.state.start(id).expect("start");
    let row = harness.wait_for(id, RuntimeState::Running);
    let port = row.cdp_port.expect("a running profile has a CDP port");

    // The window's own verifier, on the profile the window would verify.
    let job = harness.state.begin_verification(id).expect("begin");
    assert_eq!(job.port, port);
    let pages_before = HttpCdpProbe
        .page_targets(port, Duration::from_secs(3))
        .expect("the browser lists its pages")
        .len();

    let outcome = CdpFingerprintVerifier::default()
        .verify(job.port, &job.profile, &job.capabilities)
        .expect("the fingerprint can be read out of a running profile");
    harness.state.finish_verification(id, Ok(outcome));
    println!("verification: {:?}", harness.state.verification(id));

    let verification = harness
        .state
        .verification(id)
        .expect("the outcome is recorded");
    assert_eq!(
        verification.disagreements(),
        &[],
        "the engine reproduced every claim the profile makes"
    );
    assert_eq!(verification.failure(), None);

    // Verifying must not disturb the session it verifies.
    let pages_after = HttpCdpProbe
        .page_targets(port, Duration::from_secs(3))
        .expect("the browser still lists its pages")
        .len();
    assert_eq!(
        pages_before, pages_after,
        "the probe opens its own tab and closes it again"
    );
    assert!(
        std::path::Path::new(&format!(
            "/proc/{}",
            row.browser_pid.expect("a running browser has a pid")
        ))
        .exists(),
        "the browser is still the same live process"
    );
}
