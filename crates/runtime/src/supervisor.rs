use crate::capability::{CapabilityResolver, DefaultCapabilityResolver};
use crate::cdp::{CdpProbe, HttpCdpProbe};
use crate::error::RuntimeCommandError;
use crate::events::{RuntimeCommand, RuntimeComponent, RuntimeEvent, StartParams};
use crate::facade::{RuntimeFacade, RuntimeSnapshot};
use crate::planner::{DefaultLaunchPlanner, LaunchContext, LaunchPlanner};
use crate::ports::{PortAllocator, TcpPortAllocator};
use crate::process::{DefaultProcessTreeController, ProcessTreeController};
use crate::xray::{DefaultXrayConfigBuilder, XrayConfigBuilder};
use crossbeam_channel::{Receiver, Sender};
use domain::{ProfileId, RuntimeState};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

struct ActiveSession {
    _profile_id: ProfileId,
    browser: std::process::Child,
    _xray: Option<std::process::Child>,
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
    pub xray_builder: Box<dyn XrayConfigBuilder>,
    pub cdp_ready_timeout: Duration,
}

impl Default for SupervisorComponents {
    fn default() -> Self {
        Self {
            planner: Box::new(DefaultLaunchPlanner::new()),
            capability_resolver: Box::new(DefaultCapabilityResolver::new()),
            port_allocator: Box::new(TcpPortAllocator::new()),
            cdp_probe: Box::new(HttpCdpProbe::new()),
            process_tree: Box::new(DefaultProcessTreeController::new()),
            xray_builder: Box::new(DefaultXrayConfigBuilder::new()),
            cdp_ready_timeout: Duration::from_secs(12),
        }
    }
}

pub struct RuntimeSupervisor {
    command_rx: Receiver<RuntimeCommand>,
    event_tx: Sender<RuntimeEvent>,
    snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
    active_sessions: HashMap<ProfileId, ActiveSession>,
    planner: Box<dyn LaunchPlanner>,
    capability_resolver: Box<dyn CapabilityResolver>,
    port_allocator: Box<dyn PortAllocator>,
    cdp_probe: Box<dyn CdpProbe>,
    process_tree: Box<dyn ProcessTreeController>,
    _xray_builder: Box<dyn XrayConfigBuilder>,
    cdp_ready_timeout: Duration,
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
            active_sessions: HashMap::new(),
            planner: components.planner,
            capability_resolver: components.capability_resolver,
            port_allocator: components.port_allocator,
            cdp_probe: components.cdp_probe,
            process_tree: components.process_tree,
            _xray_builder: components.xray_builder,
            cdp_ready_timeout: components.cdp_ready_timeout,
        }
    }

    pub fn spawn(mut self) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("runtime-supervisor".to_string())
            .spawn(move || self.run())
            .expect("failed to spawn runtime supervisor thread")
    }

    pub fn run(&mut self) {
        loop {
            match self.command_rx.recv_timeout(Duration::from_millis(100)) {
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
                self.cleanup_all();
                false
            }
        }
    }

    fn start_profile(&mut self, params: StartParams) {
        let profile_id = params.profile.id;

        if self.active_sessions.contains_key(&profile_id) {
            let _ = self.event_tx.send(RuntimeEvent::Warning {
                profile_id,
                message: format!("profile {profile_id} is already running"),
            });
            return;
        }

        // Set state to Starting
        self.set_snapshot_state(profile_id, RuntimeState::Starting);
        let _ = self.event_tx.send(RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Starting,
        });

        // 1. Allocate CDP port
        let cdp_port = match self.port_allocator.allocate_loopback() {
            Ok(p) => p,
            Err(e) => {
                self.fail_start(profile_id, format!("port allocation failed: {e}"));
                return;
            }
        };

        // 2. Allocate SOCKS port if proxy present
        let socks_port = match &params.proxy {
            Some(_) => match self.port_allocator.allocate_loopback() {
                Ok(p) => Some(p),
                Err(e) => {
                    self.fail_start(profile_id, format!("socks port allocation failed: {e}"));
                    return;
                }
            },
            None => None,
        };

        // 3. Resolve capabilities
        let capabilities = match self.capability_resolver.resolve(&params.core) {
            Ok(c) => c,
            Err(e) => {
                self.fail_start(profile_id, format!("capability error: {e}"));
                return;
            }
        };

        // 4. Build LaunchPlan
        let ctx = LaunchContext {
            profile: &params.profile,
            core: &params.core,
            proxy: params.proxy.as_ref(),
            capabilities: &capabilities,
            cdp_port,
            socks_port,
            xray_executable: None,
            xray_config_dir: None,
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

        let _ = self.event_tx.send(RuntimeEvent::EffectiveLaunchArgs {
            profile_id,
            args: effective_args.clone(),
        });

        // 5. Spawn Chromium
        let mut cmd = std::process::Command::new(&plan.browser_executable);
        cmd.args(&plan.browser_args);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
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

        // 6. Probe CDP readiness
        match self.cdp_probe.wait_ready(cdp_port, self.cdp_ready_timeout) {
            Ok(_info) => {
                let session = ActiveSession {
                    _profile_id: profile_id,
                    browser: child,
                    _xray: None,
                    _cdp_port: cdp_port,
                    _socks_port: socks_port,
                    _effective_args: effective_args.clone(),
                    stopping: false,
                };

                self.active_sessions.insert(profile_id, session);

                self.update_full_snapshot(RuntimeSnapshot {
                    profile_id,
                    state: RuntimeState::Running,
                    browser_pid: Some(browser_pid),
                    xray_pid: None,
                    cdp_port: Some(cdp_port),
                    socks_port,
                    started_at: Some(SystemTime::now()),
                    effective_args,
                });

                let _ = self.event_tx.send(RuntimeEvent::Started {
                    profile_id,
                    browser_pid,
                    xray_pid: None,
                    cdp_port,
                    socks_port,
                });
                let _ = self.event_tx.send(RuntimeEvent::StateChanged {
                    profile_id,
                    state: RuntimeState::Running,
                });
            }
            Err(e) => {
                // Rollback
                let _ = self.process_tree.terminate_tree(browser_pid);
                let _ = child.wait();
                self.fail_start(profile_id, format!("CDP readiness probe failed: {e}"));
            }
        }
    }

    fn fail_start(&mut self, profile_id: ProfileId, message: String) {
        let state = RuntimeState::Failed {
            message: message.clone(),
        };
        self.set_snapshot_state(profile_id, state.clone());
        let _ = self
            .event_tx
            .send(RuntimeEvent::StateChanged { profile_id, state });
    }

    fn stop_profile(&mut self, profile_id: ProfileId) {
        if let Some(mut session) = self.active_sessions.remove(&profile_id) {
            session.stopping = true;

            self.set_snapshot_state(profile_id, RuntimeState::Stopping);
            let _ = self.event_tx.send(RuntimeEvent::StateChanged {
                profile_id,
                state: RuntimeState::Stopping,
            });

            let pid = session.browser.id();
            let _ = self.process_tree.terminate_tree(pid);
            let _ = session.browser.wait();

            self.set_snapshot_state(profile_id, RuntimeState::Stopped);
            let _ = self.event_tx.send(RuntimeEvent::Stopped { profile_id });
            let _ = self.event_tx.send(RuntimeEvent::StateChanged {
                profile_id,
                state: RuntimeState::Stopped,
            });
        } else {
            self.set_snapshot_state(profile_id, RuntimeState::Stopped);
            let _ = self.event_tx.send(RuntimeEvent::Stopped { profile_id });
        }
    }

    fn poll_active_sessions(&mut self) {
        let mut exited = Vec::new();

        for (profile_id, session) in self.active_sessions.iter_mut() {
            match session.browser.try_wait() {
                Ok(Some(status)) => {
                    exited.push((*profile_id, session.stopping, status));
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("error polling child process for {profile_id}: {e}");
                }
            }
        }

        for (profile_id, was_stopping, status) in exited {
            self.active_sessions.remove(&profile_id);

            if was_stopping {
                self.set_snapshot_state(profile_id, RuntimeState::Stopped);
                let _ = self.event_tx.send(RuntimeEvent::Stopped { profile_id });
                let _ = self.event_tx.send(RuntimeEvent::StateChanged {
                    profile_id,
                    state: RuntimeState::Stopped,
                });
            } else {
                let msg = format!("browser process exited unexpectedly: {status}");
                let _ = self.event_tx.send(RuntimeEvent::Crashed {
                    profile_id,
                    component: RuntimeComponent::Browser,
                    message: msg.clone(),
                });
                self.set_snapshot_state(profile_id, RuntimeState::Crashed { message: msg });
                let _ = self.event_tx.send(RuntimeEvent::StateChanged {
                    profile_id,
                    state: RuntimeState::Stopped,
                });
            }
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
                    },
                );
            }
        }
    }

    fn update_full_snapshot(&self, snapshot: RuntimeSnapshot) {
        if let Ok(mut lock) = self.snapshots.write() {
            lock.insert(snapshot.profile_id, snapshot);
        }
    }
}
