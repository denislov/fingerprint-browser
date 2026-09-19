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
    pub xray_builder: Box<dyn XrayConfigBuilder>,
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
            xray_builder: Box::new(DefaultXrayConfigBuilder::new()),
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
    command_rx: Receiver<RuntimeCommand>,
    event_tx: Sender<RuntimeEvent>,
    snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
    active_sessions: HashMap<ProfileId, ActiveSession>,
    planner: Box<dyn LaunchPlanner>,
    capability_resolver: Box<dyn CapabilityResolver>,
    port_allocator: Box<dyn PortAllocator>,
    cdp_probe: Box<dyn CdpProbe>,
    process_tree: Box<dyn ProcessTreeController>,
    xray_builder: Box<dyn XrayConfigBuilder>,
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
            active_sessions: HashMap::new(),
            planner: components.planner,
            capability_resolver: components.capability_resolver,
            port_allocator: components.port_allocator,
            cdp_probe: components.cdp_probe,
            process_tree: components.process_tree,
            xray_builder: components.xray_builder,
            cdp_ready_timeout: components.cdp_ready_timeout,
            xray_ready_timeout: components.xray_ready_timeout,
            xray_executable: components.xray_executable,
            runtime_dir: components.runtime_dir,
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

        let _ = self.event_tx.send(RuntimeEvent::EffectiveLaunchArgs {
            profile_id,
            args: effective_args.clone(),
        });

        // Prepare and verify Xray before Chromium can issue any requests.
        let mut xray = None;
        let xray_config = plan.xray.as_ref().map(|p| p.config_path.clone());
        if let Some(xray_plan) = &plan.xray {
            let result = (|| -> Result<std::process::Child, String> {
                let proxy = params.proxy.as_ref().ok_or("missing proxy configuration")?;
                self.xray_builder
                    .build(proxy, xray_plan.socks_port, &xray_plan.config_path)
                    .map_err(|e| e.to_string())?;
                let mut child = std::process::Command::new(&xray_plan.executable)
                    .arg("run")
                    .arg("-config")
                    .arg(&xray_plan.config_path)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .map_err(|e| format!("Xray spawn failed: {e}"))?;
                if let Err(e) = crate::xray::wait_ready(
                    &mut child,
                    xray_plan.socks_port,
                    self.xray_ready_timeout,
                ) {
                    self.terminate_child(&mut child);
                    return Err(e.to_string());
                }
                Ok(child)
            })();
            match result {
                Ok(child) => xray = Some(child),
                Err(e) => {
                    Self::remove_config(xray_config.as_deref());
                    self.fail_start(profile_id, e);
                    return;
                }
            }
        }
        let xray_pid = xray.as_ref().map(std::process::Child::id);

        // 5. Spawn Chromium
        let mut cmd = std::process::Command::new(&plan.browser_executable);
        cmd.args(&plan.browser_args);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                if let Some(child) = xray.as_mut() {
                    self.terminate_child(child);
                }
                Self::remove_config(xray_config.as_deref());
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
        let readiness = self
            .cdp_probe
            .wait_ready(cdp_port, self.cdp_ready_timeout)
            .map_err(|e| e.to_string())
            .and_then(|info| {
                if let Some(child) = xray.as_mut() {
                    match child.try_wait() {
                        Ok(None) => {}
                        Ok(Some(status)) => {
                            return Err(format!("Xray exited during CDP readiness: {status}"));
                        }
                        Err(e) => return Err(format!("Xray status check failed: {e}")),
                    }
                }
                Ok(info)
            });
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

                self.update_full_snapshot(RuntimeSnapshot {
                    profile_id,
                    state: RuntimeState::Running,
                    browser_pid: Some(browser_pid),
                    xray_pid,
                    cdp_port: Some(cdp_port),
                    socks_port,
                    started_at: Some(SystemTime::now()),
                    effective_args,
                });

                let _ = self.event_tx.send(RuntimeEvent::Started {
                    profile_id,
                    browser_pid,
                    xray_pid,
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
                self.terminate_child(&mut child);
                if let Some(child) = xray.as_mut() {
                    self.terminate_child(child);
                }
                Self::remove_config(xray_config.as_deref());
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

            self.terminate_child(&mut session.browser);
            if let Some(child) = session.xray.as_mut() {
                self.terminate_child(child);
            }
            Self::remove_config(session.xray_config.as_deref());

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
        for (id, session) in &mut self.active_sessions {
            for (component, child) in
                std::iter::once((RuntimeComponent::Browser, &mut session.browser))
                    .chain(session.xray.as_mut().map(|c| (RuntimeComponent::Xray, c)))
            {
                let message = match child.try_wait() {
                    Ok(None) => continue,
                    Ok(Some(status)) => {
                        format!("{component:?} process exited unexpectedly: {status}")
                    }
                    Err(e) => format!("{component:?} process status unavailable: {e}"),
                };
                exited.push((*id, component, message));
                break;
            }
        }
        for (profile_id, component, message) in exited {
            let state = RuntimeState::Crashed {
                message: message.clone(),
            };
            self.set_snapshot_state(profile_id, state.clone());
            let _ = self.event_tx.send(RuntimeEvent::Crashed {
                profile_id,
                component,
                message,
            });
            let _ = self
                .event_tx
                .send(RuntimeEvent::StateChanged { profile_id, state });
            // Always reclaim both components before publishing Stopped.
            self.stop_profile(profile_id);
        }
    }

    fn terminate_child(&self, child: &mut std::process::Child) {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.process_tree.terminate_tree(child.id());
        // A direct kill is a fallback if the platform tree controller fails.
        let _ = child.kill();
        let _ = child.wait();
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
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
        supervisor: RuntimeSupervisor,
        events: Receiver<RuntimeEvent>,
        params: StartParams,
        dir: PathBuf,
    }
    impl Fixture {
        fn new(browser: bool, cdp: bool, script: &str) -> Self {
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
