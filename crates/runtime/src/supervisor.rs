use crate::capability::{CapabilityResolver, DefaultCapabilityResolver};
use crate::cdp::{CdpProbe, HttpCdpProbe};
use crate::error::RuntimeCommandError;
use crate::events::{RuntimeCommand, RuntimeComponent, RuntimeEvent, StartParams};
use crate::facade::{RuntimeFacade, RuntimeSnapshot};
use crate::journal::{self, ProcessRecord, ReclaimReport, SessionRecord};
use crate::planner::{DefaultLaunchPlanner, LaunchContext, LaunchPlanner};
use crate::ports::{PortAllocator, TcpPortAllocator};
use crate::process::{
    DefaultProcessInspector, DefaultProcessTreeController, ProcessInspector, ProcessTreeController,
};
use crate::xray::{DefaultXrayConfigBuilder, XrayConfigBuilder};
use crossbeam_channel::{Receiver, Sender};
use domain::{ProfileId, RuntimeState};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

struct ActiveSession {
    _profile_id: ProfileId,
    browser: std::process::Child,
    xray: Option<std::process::Child>,
    xray_config: Option<std::path::PathBuf>,
    _cdp_port: u16,
    _socks_port: Option<u16>,
    _effective_args: Vec<String>,
    stopping: bool,
}

#[derive(Clone)]
pub struct ChannelRuntimeFacade {
    command_tx: Sender<RuntimeCommand>,
    snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
}

impl ChannelRuntimeFacade {
    pub fn new(
        command_tx: Sender<RuntimeCommand>,
        snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
    ) -> Self {
        Self {
            command_tx,
            snapshots,
        }
    }
}

impl RuntimeFacade for ChannelRuntimeFacade {
    fn start(&self, params: StartParams) -> Result<(), RuntimeCommandError> {
        self.command_tx
            .send(RuntimeCommand::Start(params))
            .map_err(|_| RuntimeCommandError::ChannelClosed)
    }

    fn stop(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError> {
        self.command_tx
            .send(RuntimeCommand::Stop(profile_id))
            .map_err(|_| RuntimeCommandError::ChannelClosed)
    }

    fn restart(&self, params: StartParams) -> Result<(), RuntimeCommandError> {
        self.command_tx
            .send(RuntimeCommand::Restart(params))
            .map_err(|_| RuntimeCommandError::ChannelClosed)
    }

    fn snapshot(&self, profile_id: ProfileId) -> Option<RuntimeSnapshot> {
        self.snapshots.read().ok()?.get(&profile_id).cloned()
    }
}

pub struct RuntimeSupervisorChannels {
    pub command_tx: Sender<RuntimeCommand>,
    pub command_rx: Receiver<RuntimeCommand>,
    pub event_tx: Sender<RuntimeEvent>,
    pub event_rx: Receiver<RuntimeEvent>,
}

impl RuntimeSupervisorChannels {
    pub fn new(capacity: usize) -> Self {
        let (command_tx, command_rx) = crossbeam_channel::bounded(capacity);
        let (event_tx, event_rx) = crossbeam_channel::bounded(capacity);
        Self {
            command_tx,
            command_rx,
            event_tx,
            event_rx,
        }
    }
}

pub struct SupervisorComponents {
    pub planner: Box<dyn LaunchPlanner>,
    pub capability_resolver: Box<dyn CapabilityResolver>,
    pub port_allocator: Box<dyn PortAllocator>,
    pub cdp_probe: Box<dyn CdpProbe>,
    pub process_tree: Box<dyn ProcessTreeController>,
    pub process_inspector: Box<dyn ProcessInspector>,
    pub xray_builder: Box<dyn XrayConfigBuilder>,
    /// How long a reclaimed orphan may take to exit before it is killed.
    pub orphan_grace: Duration,
    pub cdp_ready_timeout: Duration,
    pub xray_ready_timeout: Duration,
    pub xray_executable: std::path::PathBuf,
    pub runtime_dir: std::path::PathBuf,
}

impl Default for SupervisorComponents {
    fn default() -> Self {
        Self {
            planner: Box::new(DefaultLaunchPlanner::new()),
            capability_resolver: Box::new(DefaultCapabilityResolver::new()),
            port_allocator: Box::new(TcpPortAllocator::new()),
            cdp_probe: Box::new(HttpCdpProbe::new()),
            process_tree: Box::new(DefaultProcessTreeController::new()),
            process_inspector: Box::new(DefaultProcessInspector::new()),
            xray_builder: Box::new(DefaultXrayConfigBuilder::new()),
            orphan_grace: journal::DEFAULT_GRACE,
            cdp_ready_timeout: Duration::from_secs(12),
            xray_ready_timeout: Duration::from_secs(5),
            xray_executable: if cfg!(windows) {
                "bin/xray.exe"
            } else {
                "bin/xray"
            }
            .into(),
            runtime_dir: "data/runtime".into(),
        }
    }
}

pub struct RuntimeSupervisor {
    pending_commands: VecDeque<RuntimeCommand>,
    shutting_down: bool,
    command_rx: Receiver<RuntimeCommand>,
    event_tx: Sender<RuntimeEvent>,
    snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
    active_sessions: HashMap<ProfileId, ActiveSession>,
    planner: Box<dyn LaunchPlanner>,
    capability_resolver: Box<dyn CapabilityResolver>,
    port_allocator: Box<dyn PortAllocator>,
    cdp_probe: Box<dyn CdpProbe>,
    process_tree: Box<dyn ProcessTreeController>,
    process_inspector: Box<dyn ProcessInspector>,
    xray_builder: Box<dyn XrayConfigBuilder>,
    orphan_grace: Duration,
    cdp_ready_timeout: Duration,
    xray_ready_timeout: Duration,
    xray_executable: std::path::PathBuf,
    runtime_dir: std::path::PathBuf,
}

impl RuntimeSupervisor {
    pub fn new(
        command_rx: Receiver<RuntimeCommand>,
        event_tx: Sender<RuntimeEvent>,
        snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
    ) -> Self {
        Self::with_components(
            command_rx,
            event_tx,
            snapshots,
            SupervisorComponents::default(),
        )
    }

    pub fn with_components(
        command_rx: Receiver<RuntimeCommand>,
        event_tx: Sender<RuntimeEvent>,
        snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
        components: SupervisorComponents,
    ) -> Self {
        Self {
            command_rx,
            event_tx,
            snapshots,
            pending_commands: VecDeque::new(),
            shutting_down: false,
            active_sessions: HashMap::new(),
            planner: components.planner,
            capability_resolver: components.capability_resolver,
            port_allocator: components.port_allocator,
            cdp_probe: components.cdp_probe,
            process_tree: components.process_tree,
            process_inspector: components.process_inspector,
            xray_builder: components.xray_builder,
            orphan_grace: components.orphan_grace,
            cdp_ready_timeout: components.cdp_ready_timeout,
            xray_ready_timeout: components.xray_ready_timeout,
            xray_executable: components.xray_executable,
            runtime_dir: components.runtime_dir,
        }
    }

    /// Stops what an earlier run left running, and clears the records it left.
    ///
    /// Call this before the supervisor accepts its first command: a start would
    /// otherwise race with a browser that still holds the profile's data
    /// directory and its debugging port.
    pub fn reclaim_orphans(&self) -> ReclaimReport {
        journal::reclaim(
            &self.runtime_dir,
            self.process_inspector.as_ref(),
            self.process_tree.as_ref(),
            self.orphan_grace,
        )
    }

    pub fn spawn(mut self) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("runtime-supervisor".to_string())
            .spawn(move || self.run())
            .expect("failed to spawn runtime supervisor thread")
    }

    pub fn run(&mut self) {
        while !self.shutting_down {
            let command = match self.pending_commands.pop_front() {
                Some(command) => Ok(command),
                None => self.command_rx.recv_timeout(Duration::from_millis(100)),
            };
            match command {
                Ok(cmd) => {
                    let should_continue = self.handle_command(cmd);
                    if !should_continue {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }

            self.poll_active_sessions();
        }

        self.cleanup_all();
    }

    fn handle_command(&mut self, cmd: RuntimeCommand) -> bool {
        match cmd {
            RuntimeCommand::Start(params) => {
                self.start_profile(params);
                true
            }
            RuntimeCommand::Stop(profile_id) => {
                self.stop_profile(profile_id);
                true
            }
            RuntimeCommand::Restart(params) => {
                let id = params.profile.id;
                self.stop_profile(id);
                self.start_profile(params);
                true
            }
            RuntimeCommand::ShutdownAll => {
                self.shutting_down = true;
                self.pending_commands.clear();
                self.cleanup_all();
                false
            }
        }
    }

    fn start_profile(&mut self, params: StartParams) {
        let profile_id = params.profile.id;

        if self.active_sessions.contains_key(&profile_id) {
            self.emit(RuntimeEvent::Warning {
                profile_id,
                message: format!("profile {profile_id} is already running"),
            });
            return;
        }

        // Set state to Starting
        self.set_snapshot_state(profile_id, RuntimeState::Starting);
        self.emit(RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Starting,
        });

        if self.poll_start_commands(profile_id) {
            self.stop_profile(profile_id);
            return;
        }

        // 1. Allocate CDP port
        let cdp_reservation = match self.port_allocator.reserve_loopback() {
            Ok(p) => p,
            Err(e) => {
                self.fail_start(profile_id, format!("port allocation failed: {e}"));
                return;
            }
        };

        // 2. Allocate SOCKS port if proxy present
        let mut socks_reservation = match &params.proxy {
            Some(_) => match self.port_allocator.reserve_loopback() {
                Ok(p) => Some(p),
                Err(e) => {
                    self.fail_start(profile_id, format!("socks port allocation failed: {e}"));
                    return;
                }
            },
            None => None,
        };
        let cdp_port = cdp_reservation.port();
        let socks_port = socks_reservation
            .as_ref()
            .map(|reservation| reservation.port());

        // 3. Resolve capabilities
        let capabilities = match self.capability_resolver.resolve(&params.core) {
            Ok(c) => c,
            Err(e) => {
                self.fail_start(profile_id, format!("capability error: {e}"));
                return;
            }
        };

        // 3b. Report every switch the core cannot honour. The serializer omits
        // them, so without this the profile would claim a fingerprint the
        // engine never applies.
        let compatibility = crate::compat::check(&params.core, &params.profile, &capabilities);
        if let Some(message) = compatibility.message() {
            self.emit(RuntimeEvent::Warning {
                profile_id,
                message,
            });
        }

        // 4. Build LaunchPlan
        let ctx = LaunchContext {
            profile: &params.profile,
            core: &params.core,
            proxy: params.proxy.as_ref(),
            capabilities: &capabilities,
            cdp_port,
            socks_port,
            xray_executable: Some(self.xray_executable.clone()),
            xray_config_dir: Some(self.runtime_dir.join(profile_id.to_string())),
        };

        let plan = match self.planner.build(ctx) {
            Ok(p) => p,
            Err(e) => {
                self.fail_start(profile_id, format!("launch plan error: {e}"));
                return;
            }
        };

        let effective_args: Vec<String> = plan
            .browser_args
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        self.emit(RuntimeEvent::EffectiveLaunchArgs {
            profile_id,
            args: effective_args.clone(),
        });

        // Prepare and verify Xray before Chromium can issue any requests.
        let mut xray = None;
        let mut cancelled = false;
        let xray_config = plan.xray.as_ref().map(|p| p.config_path.clone());
        if let Some(xray_plan) = &plan.xray {
            let result = (|| -> Result<std::process::Child, String> {
                let proxy = params.proxy.as_ref().ok_or("missing proxy configuration")?;
                self.xray_builder
                    .build(proxy, xray_plan.socks_port, &xray_plan.config_path)
                    .map_err(|e| e.to_string())?;
                let mut command = std::process::Command::new(&xray_plan.executable);
                command
                    .args(xray_plan.args())
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                // Xray binds its own socket; release only at the handoff.
                drop(socks_reservation.take());
                let mut child = crate::process::spawn_managed(&mut command)
                    .map_err(|e| format!("Xray spawn failed: {e}"))?;
                if let Err(e) = crate::xray::wait_ready(
                    &mut child,
                    xray_plan.socks_port,
                    self.xray_ready_timeout,
                    || {
                        cancelled = self.poll_start_commands(profile_id);
                        !cancelled
                    },
                ) {
                    self.terminate_child(&mut child);
                    return Err(e.to_string());
                }
                Ok(child)
            })();
            match result {
                Ok(child) => xray = Some(child),
                Err(e) => {
                    self.clear_start_files(profile_id, xray_config.as_deref());
                    if cancelled {
                        self.stop_profile(profile_id);
                    } else {
                        self.fail_start(profile_id, e);
                    }
                    return;
                }
            }
        }
        let xray_pid = xray.as_ref().map(std::process::Child::id);

        if self.poll_start_commands(profile_id) {
            if let Some(child) = xray.as_mut() {
                self.terminate_child(child);
            }
            self.clear_start_files(profile_id, xray_config.as_deref());
            self.stop_profile(profile_id);
            return;
        }

        // 5. Spawn Chromium
        let mut cmd = std::process::Command::new(&plan.browser_executable);
        cmd.args(&plan.browser_args);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        drop(cdp_reservation);
        let mut child = match crate::process::spawn_managed(&mut cmd) {
            Ok(c) => c,
            Err(e) => {
                if let Some(child) = xray.as_mut() {
                    self.terminate_child(child);
                }
                self.clear_start_files(profile_id, xray_config.as_deref());
                self.fail_start(
                    profile_id,
                    format!(
                        "failed to spawn executable {:?}: {e}",
                        plan.browser_executable
                    ),
                );
                return;
            }
        };

        let browser_pid = child.id();

        // Write the session record as soon as both children exist, before the
        // readiness wait: a process killed during that wait would otherwise
        // leave a browser running that no later run can find. The start is still
        // not failed for a record that cannot be written - the browser is up -
        // and the warning says what is lost.
        let record = SessionRecord {
            profile_id,
            cdp_port,
            socks_port,
            started_at: journal::now_millis(),
            browser: ProcessRecord::captured(
                browser_pid,
                &plan.browser_executable,
                &effective_args,
                self.process_inspector.as_ref(),
            ),
            xray: plan.xray.as_ref().and_then(|xray_plan| {
                xray_pid.map(|pid| {
                    ProcessRecord::captured(
                        pid,
                        &xray_plan.executable,
                        &xray_plan.args(),
                        self.process_inspector.as_ref(),
                    )
                })
            }),
        };
        if let Err(error) = journal::write(&self.runtime_dir, &record) {
            self.emit(RuntimeEvent::Warning {
                profile_id,
                message: format!(
                    "the session record could not be written ({error}); if this process is \
                     killed, the browser it started will not be reclaimed by the next run"
                ),
            });
        }

        // 6. Probe CDP readiness
        let deadline = std::time::Instant::now() + self.cdp_ready_timeout;
        let readiness = loop {
            cancelled = self.poll_start_commands(profile_id);
            if cancelled {
                break Err("startup cancelled".to_string());
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break Err("CDP readiness timed out".to_string());
            }
            let live = (|| -> Result<(), String> {
                if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                    return Err(format!("browser exited during CDP readiness: {status}"));
                }
                if let Some(child) = xray.as_mut() {
                    match child.try_wait() {
                        Ok(None) => {}
                        Ok(Some(status)) => {
                            return Err(format!("Xray exited during CDP readiness: {status}"));
                        }
                        Err(e) => return Err(format!("Xray status check failed: {e}")),
                    }
                }
                Ok(())
            })();
            if let Err(error) = live {
                break Err(error);
            }
            match self
                .cdp_probe
                .wait_ready(cdp_port, remaining.min(Duration::from_millis(100)))
            {
                Ok(info) => {
                    cancelled = self.poll_start_commands(profile_id);
                    if cancelled {
                        break Err("startup cancelled".to_string());
                    }
                    if !matches!(child.try_wait(), Ok(None)) {
                        break Err("browser exited during CDP readiness".into());
                    }
                    // Recheck Xray after the probe before advertising Running.
                    if let Some(xray) = xray.as_mut()
                        && !matches!(xray.try_wait(), Ok(None))
                    {
                        break Err("Xray exited during CDP readiness".into());
                    }
                    break Ok(info);
                }
                Err(crate::CdpError::Timeout { .. }) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => break Err(error.to_string()),
            }
        };
        match readiness {
            Ok(_info) => {
                let session = ActiveSession {
                    _profile_id: profile_id,
                    browser: child,
                    xray,
                    xray_config,
                    _cdp_port: cdp_port,
                    _socks_port: socks_port,
                    _effective_args: effective_args.clone(),
                    stopping: false,
                };

                self.active_sessions.insert(profile_id, session);

                // The record has to be readable back, or the next run will find
                // a process it cannot identify. A disagreement is reported now
                // rather than at the next start.
                if let Some(disagreement) =
                    journal::confirm(&record, self.process_inspector.as_ref())
                {
                    self.emit(RuntimeEvent::Warning {
                        profile_id,
                        message: format!(
                            "the session record does not match the running processes ({disagreement}); \
                             the next run will not reclaim them"
                        ),
                    });
                }

                self.update_full_snapshot(RuntimeSnapshot {
                    profile_id,
                    state: RuntimeState::Running,
                    browser_pid: Some(browser_pid),
                    xray_pid,
                    cdp_port: Some(cdp_port),
                    socks_port,
                    started_at: Some(SystemTime::now()),
                    effective_args,
                    last_error: None,
                    last_warning: None,
                    dropped_events: 0,
                });

                self.emit(RuntimeEvent::Started {
                    profile_id,
                    browser_pid,
                    xray_pid,
                    cdp_port,
                    socks_port,
                });
                self.emit(RuntimeEvent::StateChanged {
                    profile_id,
                    state: RuntimeState::Running,
                });
            }
            Err(e) => {
                // Rollback
                self.terminate_child(&mut child);
                if let Some(child) = xray.as_mut() {
                    self.terminate_child(child);
                }
                self.clear_start_files(profile_id, xray_config.as_deref());
                if cancelled {
                    self.stop_profile(profile_id);
                } else {
                    self.fail_start(profile_id, format!("CDP readiness probe failed: {e}"));
                }
            }
        }
    }

    /// Service cancellation without recursively starting another profile.
    /// A bounded batch leaves time for readiness and process monitoring.
    fn poll_start_commands(&mut self, starting: ProfileId) -> bool {
        self.poll_active_sessions();
        for _ in 0..64 {
            let command = match self.command_rx.try_recv() {
                Ok(command) => command,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.shutting_down = true;
                    self.pending_commands.clear();
                    self.cleanup_all();
                    return true;
                }
            };
            match command {
                RuntimeCommand::ShutdownAll => {
                    self.shutting_down = true;
                    self.pending_commands.clear();
                    self.cleanup_all();
                    return true;
                }
                RuntimeCommand::Stop(id) => {
                    self.pending_commands.retain(|command| match command {
                        RuntimeCommand::Start(p) | RuntimeCommand::Restart(p) => {
                            p.profile_id() != id
                        }
                        _ => true,
                    });
                    if id == starting {
                        return true;
                    }
                    self.stop_profile(id);
                }
                RuntimeCommand::Start(params) if params.profile_id() == starting => {
                    self.emit(RuntimeEvent::Warning {
                        profile_id: starting,
                        message: format!("profile {starting} is already starting"),
                    });
                }
                RuntimeCommand::Restart(params) if params.profile_id() == starting => {
                    self.pending_commands
                        .push_back(RuntimeCommand::Restart(params));
                    return true;
                }
                command => self.pending_commands.push_back(command),
            }
        }
        false
    }

    fn fail_start(&mut self, profile_id: ProfileId, message: String) {
        let state = RuntimeState::Failed {
            message: message.clone(),
        };
        self.set_snapshot_state(profile_id, state.clone());
        self.emit(RuntimeEvent::StateChanged { profile_id, state });
    }

    fn stop_profile(&mut self, profile_id: ProfileId) {
        self.stop_session(profile_id, true);
    }

    fn stop_session(&mut self, profile_id: ProfileId, graceful: bool) {
        if let Some(mut session) = self.active_sessions.remove(&profile_id) {
            session.stopping = true;

            self.set_snapshot_state(profile_id, RuntimeState::Stopping);
            self.emit(RuntimeEvent::StateChanged {
                profile_id,
                state: RuntimeState::Stopping,
            });

            if graceful
                && matches!(session.browser.try_wait(), Ok(None))
                && self
                    .cdp_probe
                    .close_browser(session._cdp_port, Duration::from_millis(250))
                    .is_ok()
            {
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                while matches!(session.browser.try_wait(), Ok(None))
                    && std::time::Instant::now() < deadline
                {
                    self.poll_active_sessions();
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            self.terminate_child(&mut session.browser);
            if let Some(child) = session.xray.as_mut() {
                self.terminate_child(child);
            }
            Self::remove_config(session.xray_config.as_deref());
            journal::remove(&self.runtime_dir, profile_id);

            self.set_snapshot_state(profile_id, RuntimeState::Stopped);
            self.emit(RuntimeEvent::Stopped { profile_id });
            self.emit(RuntimeEvent::StateChanged {
                profile_id,
                state: RuntimeState::Stopped,
            });
        } else {
            self.set_snapshot_state(profile_id, RuntimeState::Stopped);
            self.emit(RuntimeEvent::Stopped { profile_id });
            self.emit(RuntimeEvent::StateChanged {
                profile_id,
                state: RuntimeState::Stopped,
            });
        }
    }

    fn poll_active_sessions(&mut self) {
        let mut exited = Vec::new();
        for (id, session) in &mut self.active_sessions {
            for (component, child) in
                std::iter::once((RuntimeComponent::Browser, &mut session.browser))
                    .chain(session.xray.as_mut().map(|c| (RuntimeComponent::Xray, c)))
            {
                let outcome = match child.try_wait() {
                    Ok(None) => continue,
                    Ok(Some(status)) => {
                        let clean = component == RuntimeComponent::Browser && status.success();
                        (component, status.to_string(), clean)
                    }
                    Err(e) => (component, e.to_string(), false),
                };
                exited.push((*id, outcome));
                break;
            }
        }
        for (profile_id, (component, detail, clean)) in exited {
            if clean {
                // Browser exited cleanly with exit code 0 (e.g. user closed the browser window).
                // This is a normal user exit, so reclaim without raising a false crash alarm.
                self.stop_session(profile_id, false);
            } else {
                let message = format!("{component:?} process exited unexpectedly: {detail}");
                let state = RuntimeState::Crashed {
                    message: message.clone(),
                };
                self.set_snapshot_state(profile_id, state.clone());
                self.emit(RuntimeEvent::Crashed {
                    profile_id,
                    component,
                    message,
                });
                self.emit(RuntimeEvent::StateChanged { profile_id, state });
                // Always reclaim both components before publishing Stopped.
                self.stop_session(profile_id, false);
            }
        }
    }

    /// Events are bounded best-effort notifications; snapshots are authoritative.
    /// Never let a slow or disconnected UI stall child-process management.
    fn emit(&self, event: RuntimeEvent) {
        let id = event.profile_id();
        if let Ok(mut snapshots) = self.snapshots.write()
            && let Some(snapshot) = snapshots.get_mut(&id)
        {
            match &event {
                RuntimeEvent::EffectiveLaunchArgs { args, .. } => {
                    snapshot.effective_args = args.clone()
                }
                RuntimeEvent::Warning { message, .. } => {
                    snapshot.last_warning = Some(message.clone())
                }
                RuntimeEvent::Crashed { message, .. }
                | RuntimeEvent::StateChanged {
                    state: RuntimeState::Failed { message },
                    ..
                } => {
                    snapshot.last_error = Some(message.clone());
                }
                _ => {}
            }
        }
        if self.event_tx.try_send(event).is_err()
            && let Ok(mut snapshots) = self.snapshots.write()
            && let Some(snapshot) = snapshots.get_mut(&id)
        {
            snapshot.dropped_events = snapshot.dropped_events.saturating_add(1);
        }
    }

    fn terminate_child(&self, child: &mut std::process::Child) {
        if matches!(child.try_wait(), Ok(None)) {
            // The group may still contain renderers even after its leader exits.
            let _ = self.process_tree.terminate_tree(child.id());
            // A direct kill is a fallback if the platform tree controller fails.
            let _ = child.kill();
        }
        let _ = child.wait();
    }

    /// What a failed, cancelled or rolled-back start leaves behind: the
    /// temporary Xray config, which holds upstream credentials, and the session
    /// record, which would otherwise name processes that are already gone.
    fn clear_start_files(&self, profile_id: ProfileId, xray_config: Option<&std::path::Path>) {
        Self::remove_config(xray_config);
        journal::remove(&self.runtime_dir, profile_id);
    }

    fn remove_config(path: Option<&std::path::Path>) {
        if let Some(path) = path
            && let Err(e) = std::fs::remove_file(path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("failed to remove temporary Xray config: {e}");
        }
    }

    fn cleanup_all(&mut self) {
        let active_ids: Vec<ProfileId> = self.active_sessions.keys().copied().collect();
        for id in active_ids {
            self.stop_profile(id);
        }
    }

    fn set_snapshot_state(&self, profile_id: ProfileId, state: RuntimeState) {
        if let Ok(mut lock) = self.snapshots.write() {
            if let Some(s) = lock.get_mut(&profile_id) {
                if state == RuntimeState::Starting {
                    s.last_error = None;
                    s.last_warning = None;
                    s.effective_args.clear();
                }
                if matches!(state, RuntimeState::Stopped | RuntimeState::Failed { .. }) {
                    s.browser_pid = None;
                    s.xray_pid = None;
                    s.cdp_port = None;
                    s.socks_port = None;
                    s.started_at = None;
                }
                s.state = state;
            } else {
                lock.insert(
                    profile_id,
                    RuntimeSnapshot {
                        profile_id,
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
                    },
                );
            }
        }
    }

    fn update_full_snapshot(&self, mut snapshot: RuntimeSnapshot) {
        if let Ok(mut lock) = self.snapshots.write() {
            if let Some(previous) = lock.get(&snapshot.profile_id) {
                snapshot.dropped_events = previous.dropped_events;
                snapshot.last_error = previous.last_error.clone();
                snapshot.last_warning = previous.last_warning.clone();
            }
            lock.insert(snapshot.profile_id, snapshot);
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::process::ProcessReading;
    use crate::{CdpError, CdpInfo, LaunchPlanError};
    use domain::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct Probe(bool);
    impl CdpProbe for Probe {
        fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
            if self.0 {
                Ok(CdpInfo::default())
            } else {
                Err(CdpError::Timeout { timeout_secs: 0 })
            }
        }
    }
    struct Planner(bool);
    impl LaunchPlanner for Planner {
        fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
            let mut plan = DefaultLaunchPlanner.build(ctx)?;
            plan.browser_executable = if self.0 {
                "/bin/sleep"
            } else {
                "/nonexistent/browser"
            }
            .into();
            plan.browser_args = vec!["60".into()];
            Ok(plan)
        }
    }
    struct Fixture {
        _serial: std::sync::MutexGuard<'static, ()>,
        commands: Option<Sender<RuntimeCommand>>,
        supervisor: RuntimeSupervisor,
        events: Receiver<RuntimeEvent>,
        params: StartParams,
        dir: PathBuf,
    }
    impl Fixture {
        fn new(browser: bool, cdp: bool, script: &str) -> Self {
            // Avoid fork/exec races with another test writing its executable
            // and immediate port-rebind assertions racing sibling fixtures.
            static FIXTURES: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let serial = FIXTURES.lock().unwrap_or_else(|error| error.into_inner());
            let id = ProfileId::new();
            let dir = std::env::temp_dir().join(format!("fp-runtime-{id}"));
            std::fs::create_dir_all(&dir).unwrap();
            let executable = dir.join("xray");
            std::fs::write(&executable, script).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let components = SupervisorComponents {
                planner: Box::new(Planner(browser)),
                cdp_probe: Box::new(Probe(cdp)),
                xray_executable: executable,
                runtime_dir: dir.clone(),
                xray_ready_timeout: Duration::from_millis(500),
                cdp_ready_timeout: Duration::from_millis(200),
                ..Default::default()
            };
            let channels = RuntimeSupervisorChannels::new(128);
            let supervisor = RuntimeSupervisor::with_components(
                channels.command_rx,
                channels.event_tx,
                Arc::new(RwLock::new(HashMap::new())),
                components,
            );
            let proxy = ProxyProfile {
                id: ProxyId::new(),
                name: "test".into(),
                outbound: ProxyOutbound::Socks5(Socks5Outbound {
                    host: "localhost".into(),
                    port: 1080,
                    username: Some("user".into()),
                    password: Some("secret".into()),
                }),
            };
            let core = BrowserCore {
                id: CoreId::new(),
                name: "test".into(),
                executable: "/bin/sleep".into(),
                version: "128".into(),
                major: 128,
            };
            let profile = BrowserProfile {
                id,
                name: "test".into(),
                core_id: core.id,
                user_data_dir: dir.join("profile"),
                fingerprint: FingerprintProfile::new_random(42),
                proxy_id: Some(proxy.id),
                window: WindowProfile::new(800, 600),
                start_target: StartTarget::Blank,
            };
            Self {
                _serial: serial,
                commands: Some(channels.command_tx),
                supervisor,
                events: channels.event_rx,
                params: StartParams::with_proxy(profile, core, proxy),
                dir,
            }
        }
        fn start(&mut self) {
            self.supervisor.start_profile(self.params.clone());
        }
        fn config(&self) -> PathBuf {
            self.dir
                .join(self.params.profile.id.to_string())
                .join("xray.json")
        }
        fn snapshot(&self) -> RuntimeSnapshot {
            self.supervisor.snapshots.read().unwrap()[&self.params.profile.id].clone()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.supervisor.cleanup_all();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    const XRAY: &str = "#!/usr/bin/env python3\nimport json,socket,sys,time\nc=json.load(open(sys.argv[3]))\ns=socket.socket()\ns.bind(('127.0.0.1',c['inbounds'][0]['port']))\ns.listen()\ntime.sleep(60)\n";

    #[test]
    fn a_running_session_writes_a_record_and_stopping_it_removes_it() {
        let mut f = Fixture::new(true, true, XRAY);
        f.start();

        let record = journal::read(&f.dir, f.params.profile.id)
            .unwrap()
            .expect("a running session writes a record");
        let session = &f.supervisor.active_sessions[&f.params.profile.id];
        assert_eq!(record.browser.pid, session.browser.id());
        assert_eq!(
            record.xray.as_ref().map(|xray| xray.pid),
            session.xray.as_ref().map(|xray| xray.id())
        );
        assert_eq!(record.cdp_port, f.snapshot().cdp_port.unwrap());
        assert_eq!(record.browser.args, ["60"]);
        let xray = record.xray.as_ref().unwrap();
        assert_eq!(xray.args[0], "run");
        assert_eq!(xray.args[2], f.config().to_string_lossy());

        f.supervisor.stop_profile(f.params.profile.id);
        assert!(
            journal::read(&f.dir, f.params.profile.id)
                .unwrap()
                .is_none(),
            "a stopped session leaves no record to reclaim"
        );
    }

    /// A start that rolls back has to take its record with it: a record naming
    /// processes that are already gone would be reported as a leftover at the
    /// next start.
    #[test]
    fn a_start_that_never_reaches_readiness_leaves_no_record() {
        // The browser starts (it is `/bin/sleep`) but its debugging endpoint
        // never answers, so the start fails and rolls back.
        let mut f = Fixture::new(true, false, XRAY);
        f.start();

        assert!(
            matches!(f.snapshot().state, RuntimeState::Failed { .. }),
            "{:?}",
            f.snapshot().state
        );
        assert!(
            journal::read(&f.dir, f.params.profile.id)
                .unwrap()
                .is_none(),
            "a rolled back start left a record behind"
        );
        assert!(!f.config().exists());
        assert!(
            f.supervisor.reclaim_orphans().is_empty(),
            "a rolled back start leaves nothing to reclaim"
        );
    }

    /// A process that was killed - `SIGKILL`, a crash, a window destroyed
    /// without the close protocol - leaves its children running and its record
    /// on disk. The next start has to find them from that record alone.
    #[test]
    fn reclaiming_orphans_stops_what_a_killed_run_left_running() {
        let mut f = Fixture::new(true, true, XRAY);
        f.start();
        let record = journal::read(&f.dir, f.params.profile.id)
            .unwrap()
            .expect("a running session writes a record");

        // Taking the handles away does not stop the children; that is what
        // makes this the crash case rather than a stop.
        let session = f
            .supervisor
            .active_sessions
            .remove(&f.params.profile.id)
            .unwrap();
        drop(session);
        assert!(
            matches!(
                DefaultProcessInspector.inspect(record.browser.pid),
                ProcessReading::Live(_)
            ),
            "the browser should still be running for this test to mean anything"
        );

        let report = f.supervisor.reclaim_orphans();

        assert_eq!(report.reclaimed.len(), 1, "{report:?}");
        assert_eq!(report.reclaimed[0].browser_pid, record.browser.pid);
        assert_eq!(
            report.reclaimed[0].xray_pid,
            record.xray.as_ref().map(|xray| xray.pid)
        );
        assert_eq!(
            DefaultProcessInspector.inspect(record.browser.pid),
            ProcessReading::Absent
        );
        assert!(
            journal::read(&f.dir, f.params.profile.id)
                .unwrap()
                .is_none()
        );
        assert!(
            !f.config().exists(),
            "a killed run leaves the upstream credentials in its temporary config"
        );
    }

    #[test]
    fn normal_stop_attempts_graceful_close_but_xray_crash_skips_it() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct ClosingProbe(Arc<AtomicUsize>);
        impl CdpProbe for ClosingProbe {
            fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
                Ok(CdpInfo::default())
            }
            fn close_browser(&self, _: u16, _: Duration) -> Result<(), CdpError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(CdpError::Http("simulate unavailable CDP".into()))
            }
        }
        let mut f = Fixture::new(true, true, XRAY);
        let calls = Arc::new(AtomicUsize::new(0));
        f.supervisor.cdp_probe = Box::new(ClosingProbe(calls.clone()));
        f.start();
        f.supervisor.stop_profile(f.params.profile.id);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        f.start();
        let xray = f
            .supervisor
            .active_sessions
            .get_mut(&f.params.profile.id)
            .unwrap()
            .xray
            .as_mut()
            .unwrap();
        xray.kill().unwrap();
        xray.wait().unwrap();
        f.supervisor.poll_active_sessions();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
    }

    #[test]
    fn a_legacy_core_reports_the_switches_it_cannot_honour() {
        let mut f = Fixture::new(true, true, XRAY);
        // Measured on 142: the exclusions are what a legacy core ignores. The
        // noise switch is carried below the pivot, so it is not the omission to
        // expect here.
        f.params.profile.fingerprint.disabled_spoofing = vec![SpoofingFeature::Font];
        f.start();

        let warning = f
            .snapshot()
            .last_warning
            .clone()
            .expect("compatibility warning");
        assert!(warning.contains("--disable-spoofing"), "{warning}");
        assert!(warning.contains("major 144"), "{warning}");

        // The same fixture with a verified core reports nothing.
        f.supervisor.stop_profile(f.params.profile.id);
        f.params.core.major = 148;
        f.params.core.version = "148.0.7778.215".into();
        f.start();

        assert_eq!(f.snapshot().state, RuntimeState::Running);
        assert_eq!(f.snapshot().last_warning, None);
    }

    #[test]
    fn full_event_queue_does_not_block_stop_crash_or_shutdown() {
        let mut f = Fixture::new(true, true, XRAY);
        // A warning has to exist for the checks below to be about replacing it.
        f.params.profile.fingerprint.disabled_spoofing = vec![SpoofingFeature::Font];
        let (sender, receiver) = crossbeam_channel::bounded(1);
        f.supervisor.event_tx = sender;
        f.events = receiver;
        f.start();
        assert_eq!(f.snapshot().state, RuntimeState::Running);
        assert!(f.snapshot().dropped_events > 0);
        assert!(!f.snapshot().effective_args.is_empty());
        f.start();
        let already_running = f.snapshot().last_warning.clone().expect("warning");
        assert!(
            already_running.contains("already running"),
            "{already_running}"
        );
        f.supervisor.stop_profile(f.params.profile.id);
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(!f.config().exists());
        f.start();
        let fresh = f.snapshot().last_warning.clone().expect("warning");
        assert!(
            !fresh.contains("already running"),
            "a new start must replace the previous diagnostics: {fresh}"
        );
        let child = f
            .supervisor
            .active_sessions
            .get_mut(&f.params.profile.id)
            .unwrap()
            .xray
            .as_mut()
            .unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        f.supervisor.poll_active_sessions();
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(f.snapshot().last_error.as_ref().unwrap().contains("Xray"));
        assert!(!f.config().exists());
        f.start();
        assert!(f.snapshot().last_error.is_none());
        assert!(!f.supervisor.handle_command(RuntimeCommand::ShutdownAll));
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(f.supervisor.active_sessions.is_empty());
        assert!(!f.config().exists());
    }

    #[test]
    fn disconnected_event_receiver_preserves_failure_diagnostics() {
        let mut f = Fixture::new(false, true, XRAY);
        let (sender, receiver) = crossbeam_channel::bounded(1);
        drop(receiver);
        f.supervisor.event_tx = sender;
        f.start();
        let snapshot = f.snapshot();
        assert!(matches!(snapshot.state, RuntimeState::Failed { .. }));
        assert!(
            snapshot
                .last_error
                .unwrap()
                .contains("failed to spawn executable")
        );
        assert!(!snapshot.effective_args.is_empty());
        assert!(snapshot.dropped_events >= 3);
        assert!(!f.config().exists());
    }

    #[test]
    fn startup_holds_distinct_ports_and_releases_them_on_planning_failure() {
        use std::net::TcpListener;
        use std::sync::Mutex;
        struct CheckingPlanner(Arc<Mutex<Vec<u16>>>);
        impl LaunchPlanner for CheckingPlanner {
            fn build(&self, ctx: LaunchContext<'_>) -> Result<LaunchPlan, LaunchPlanError> {
                let ports = vec![ctx.cdp_port, ctx.socks_port.unwrap()];
                assert_ne!(ports[0], ports[1]);
                for port in &ports {
                    assert!(TcpListener::bind(("127.0.0.1", *port)).is_err());
                }
                *self.0.lock().unwrap() = ports;
                Err(LaunchPlanError::InvalidArguments(
                    "test planning failure".into(),
                ))
            }
        }
        let mut f = Fixture::new(false, true, XRAY);
        let ports = Arc::new(Mutex::new(Vec::new()));
        f.supervisor.planner = Box::new(CheckingPlanner(ports.clone()));
        f.start();
        assert!(matches!(f.snapshot().state, RuntimeState::Failed { .. }));
        for port in ports.lock().unwrap().iter() {
            assert!(TcpListener::bind(("127.0.0.1", *port)).is_ok());
        }
    }

    struct CommandProbe {
        sender: Sender<RuntimeCommand>,
        command: RuntimeCommand,
    }
    impl CdpProbe for CommandProbe {
        fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
            self.sender.send(self.command.clone()).unwrap();
            // Cancellation wins even if the same probe reports readiness.
            Ok(CdpInfo::default())
        }
    }

    #[test]
    fn stop_during_cdp_readiness_rolls_back_without_started_or_failed() {
        let mut f = Fixture::new(true, true, XRAY);
        f.supervisor.cdp_ready_timeout = Duration::from_secs(30);
        f.supervisor.cdp_probe = Box::new(CommandProbe {
            sender: f.commands.as_ref().unwrap().clone(),
            command: RuntimeCommand::Stop(f.params.profile.id),
        });
        let start = std::time::Instant::now();
        f.start();
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(!f.config().exists());
        assert!(f.supervisor.active_sessions.is_empty());
        assert!(!f.events.try_iter().any(|event| matches!(
            event,
            RuntimeEvent::Started { .. }
                | RuntimeEvent::StateChanged {
                    state: RuntimeState::Failed { .. },
                    ..
                }
        )));
    }

    #[test]
    fn stop_during_xray_wait_reaps_process_before_timeout() {
        let mut f = Fixture::new(
            true,
            true,
            "#!/bin/sh\necho $$ > \"$3.pid\"\nexec sleep 60\n",
        );
        f.supervisor.xray_ready_timeout = Duration::from_secs(30);
        let marker = f.config().with_file_name("xray.json.pid");
        let sender = f.commands.as_ref().unwrap().clone();
        let id = f.params.profile.id;
        let worker = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            let pid = loop {
                if let Ok(text) = std::fs::read_to_string(&marker)
                    && let Ok(pid) = text.trim().parse::<u32>()
                {
                    break pid;
                }
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(5));
            };
            sender.send(RuntimeCommand::Stop(id)).unwrap();
            pid
        });
        let start = std::time::Instant::now();
        f.start();
        let pid = worker.join().unwrap();
        assert!(start.elapsed() < Duration::from_secs(3));
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(!f.config().exists());
        #[cfg(target_os = "linux")]
        assert!(!PathBuf::from(format!("/proc/{pid}")).exists());
    }

    #[test]
    fn shutdown_during_start_exits_run_loop_and_discards_deferred_start() {
        let mut f = Fixture::new(true, true, XRAY);
        f.supervisor.cdp_probe = Box::new(CommandProbe {
            sender: f.commands.as_ref().unwrap().clone(),
            command: RuntimeCommand::ShutdownAll,
        });
        let mut next = f.params.clone();
        next.profile.id = ProfileId::new();
        let next_id = next.profile.id;
        let sender = f.commands.as_ref().unwrap();
        sender
            .send(RuntimeCommand::Start(f.params.clone()))
            .unwrap();
        sender.send(RuntimeCommand::Start(next)).unwrap();
        f.supervisor.run();
        assert!(f.supervisor.shutting_down);
        assert!(f.supervisor.pending_commands.is_empty());
        assert!(f.supervisor.active_sessions.is_empty());
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(!f.config().exists());
        assert!(
            !f.supervisor
                .snapshots
                .read()
                .unwrap()
                .contains_key(&next_id)
        );
    }

    #[test]
    fn disconnected_command_channel_cancels_start() {
        let mut f = Fixture::new(true, true, XRAY);
        f.commands.take();
        f.start();
        assert!(f.supervisor.shutting_down);
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(!f.config().exists());
    }

    #[test]
    fn restart_during_readiness_cancels_then_starts_again_without_recursion() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct RestartProbe {
            sender: Sender<RuntimeCommand>,
            params: StartParams,
            calls: AtomicUsize,
        }
        impl CdpProbe for RestartProbe {
            fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
                let command = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    RuntimeCommand::Restart(self.params.clone())
                } else {
                    RuntimeCommand::ShutdownAll
                };
                self.sender.send(command).unwrap();
                Ok(CdpInfo::default())
            }
        }
        let mut f = Fixture::new(true, true, XRAY);
        f.supervisor.cdp_probe = Box::new(RestartProbe {
            sender: f.commands.as_ref().unwrap().clone(),
            params: f.params.clone(),
            calls: AtomicUsize::new(0),
        });
        f.commands
            .as_ref()
            .unwrap()
            .send(RuntimeCommand::Start(f.params.clone()))
            .unwrap();
        f.supervisor.run();
        let events: Vec<_> = f.events.try_iter().collect();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    RuntimeEvent::StateChanged {
                        state: RuntimeState::Starting,
                        ..
                    }
                ))
                .count(),
            2
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, RuntimeEvent::Started { .. }))
        );
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(!f.config().exists());
    }

    #[test]
    fn stop_other_profile_removes_earlier_deferred_start_and_preserves_later_start() {
        let mut f = Fixture::new(true, true, XRAY);
        f.start();
        let id = f.params.profile.id;
        let sender = f.commands.as_ref().unwrap();
        sender
            .send(RuntimeCommand::Start(f.params.clone()))
            .unwrap();
        sender.send(RuntimeCommand::Stop(id)).unwrap();
        sender
            .send(RuntimeCommand::Start(f.params.clone()))
            .unwrap();
        assert!(!f.supervisor.poll_start_commands(ProfileId::new()));
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(!f.config().exists());
        assert_eq!(f.supervisor.pending_commands.len(), 1);
        assert!(
            matches!(f.supervisor.pending_commands.front(), Some(RuntimeCommand::Start(p)) if p.profile_id() == id)
        );
    }

    #[test]
    fn starting_another_profile_still_reaps_existing_crashed_session() {
        struct CheckingProbe {
            snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
            crashed: ProfileId,
        }
        impl CdpProbe for CheckingProbe {
            fn wait_ready(&self, _: u16, _: Duration) -> Result<CdpInfo, CdpError> {
                assert_eq!(
                    self.snapshots.read().unwrap()[&self.crashed].state,
                    RuntimeState::Stopped
                );
                Ok(CdpInfo::default())
            }
        }
        let mut f = Fixture::new(true, true, XRAY);
        f.start();
        let id = f.params.profile.id;
        let xray = f
            .supervisor
            .active_sessions
            .get_mut(&id)
            .unwrap()
            .xray
            .as_mut()
            .unwrap();
        xray.kill().unwrap();
        xray.wait().unwrap();
        f.supervisor.cdp_probe = Box::new(CheckingProbe {
            snapshots: f.supervisor.snapshots.clone(),
            crashed: id,
        });
        let mut next = f.params.clone();
        next.profile.id = ProfileId::new();
        next.profile.user_data_dir = f.dir.join("second-profile");
        let next_id = next.profile.id;
        f.supervisor.start_profile(next);
        assert_eq!(
            f.supervisor.snapshots.read().unwrap()[&next_id].state,
            RuntimeState::Running
        );
    }

    #[test]
    fn xray_crash_terminates_browser_and_clears_session() {
        let mut f = Fixture::new(true, true, XRAY);
        f.start();
        assert_eq!(f.snapshot().state, RuntimeState::Running);
        assert!(f.snapshot().xray_pid.is_some());
        let id = f.params.profile.id;
        let session = f.supervisor.active_sessions.get_mut(&id).unwrap();
        let browser_pid = session.browser.id();
        session.xray.as_mut().unwrap().kill().unwrap();
        session.xray.as_mut().unwrap().wait().unwrap();
        f.supervisor.poll_active_sessions();
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(f.snapshot().browser_pid.is_none());
        assert!(!f.config().exists());
        #[cfg(target_os = "linux")]
        assert!(!PathBuf::from(format!("/proc/{browser_pid}")).exists());
        assert!(f.events.try_iter().any(|e| matches!(
            e,
            RuntimeEvent::Crashed {
                component: RuntimeComponent::Xray,
                ..
            }
        )));
    }

    #[test]
    fn startup_failures_rollback_xray_and_config() {
        for (browser, cdp, script) in [
            (false, true, XRAY),
            (true, false, XRAY),
            (true, true, "#!/bin/sh\nexit 1\n"),
            (true, true, "#!/bin/sh\nexec sleep 60\n"),
        ] {
            let mut f = Fixture::new(browser, cdp, script);
            f.start();
            assert!(matches!(f.snapshot().state, RuntimeState::Failed { .. }));
            assert!(f.supervisor.active_sessions.is_empty());
            assert!(!f.config().exists());
            assert!(
                !f.events
                    .try_iter()
                    .any(|e| matches!(e, RuntimeEvent::Started { .. }))
            );
        }
    }

    #[test]
    fn normal_stop_reclaims_both_children_and_missing_xray_fails_closed() {
        let mut f = Fixture::new(true, true, XRAY);
        f.start();
        assert_eq!(f.snapshot().state, RuntimeState::Running);
        f.supervisor.stop_profile(f.params.profile.id);
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(f.supervisor.active_sessions.is_empty());
        assert!(!f.config().exists());
        std::fs::remove_file(&f.supervisor.xray_executable).unwrap();
        f.start();
        assert!(matches!(f.snapshot().state, RuntimeState::Failed { .. }));
        assert!(!f.config().exists());
    }

    #[test]
    fn browser_crash_reclaims_xray_and_stop_is_idempotent() {
        let mut f = Fixture::new(true, true, XRAY);
        f.start();
        let id = f.params.profile.id;
        let session = f.supervisor.active_sessions.get_mut(&id).unwrap();
        let xray_pid = session.xray.as_ref().unwrap().id();
        session.browser.kill().unwrap();
        session.browser.wait().unwrap();
        f.supervisor.poll_active_sessions();
        f.supervisor.stop_profile(id);
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(f.snapshot().xray_pid.is_none());
        assert!(!f.config().exists());
        #[cfg(target_os = "linux")]
        assert!(!PathBuf::from(format!("/proc/{xray_pid}")).exists());
    }
}
