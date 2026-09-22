mod channel;
mod ownership;
mod start;
pub use channel::{ChannelRuntimeFacade, HANDOVER_TIMEOUT, RuntimeSupervisorChannels};
use ownership::*;

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

pub struct SupervisorComponents {
    pub planner: Box<dyn LaunchPlanner>,
    pub capability_resolver: Box<dyn CapabilityResolver>,
    pub port_allocator: Box<dyn PortAllocator>,
    pub cdp_probe: Box<dyn CdpProbe>,
    /// Shared rather than owned, so the value a start tracks its own resources in
    /// can end a process group on its way out without borrowing the supervisor
    /// that is running it.
    pub process_tree: Arc<dyn ProcessTreeController>,
    pub process_inspector: Arc<dyn ProcessInspector>,
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
            process_tree: Arc::new(DefaultProcessTreeController::new()),
            process_inspector: Arc::new(DefaultProcessInspector::new()),
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
    process_tree: Arc<dyn ProcessTreeController>,
    process_inspector: Arc<dyn ProcessInspector>,
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

    /// Takes over what an earlier run deliberately left running, and stops what
    /// it left by crashing.
    ///
    /// Call this before the supervisor accepts its first command: a start would
    /// otherwise race with a browser that still holds the profile's data
    /// directory and its debugging port. That is just as true of a browser left on
    /// purpose as of one left by a crash - the difference is the answer, not the
    /// problem. An adopted session is a running profile again: it is in the
    /// snapshots, it is stopped by an ordinary stop, and it is restarted by an
    /// ordinary restart.
    ///
    /// The report it returns is the same one, whether anything was adopted.
    pub fn recover_orphans(&mut self) -> ReclaimReport {
        let report = journal::recover(
            &self.runtime_dir,
            self.process_inspector.as_ref(),
            self.process_tree.as_ref(),
            self.orphan_grace,
        );
        for adopted in &report.adopted {
            self.install_adopted(adopted);
        }
        report
    }

    /// A session from a previous run, running again with no handle to it.
    fn install_adopted(&mut self, adopted: &journal::Adopted) {
        let profile_id = adopted.profile_id;
        // The temporary Xray config is the adopted engine's and is still on disk -
        // it holds the upstream credentials the engine is using. It is named here
        // so that stopping this session removes it, and nothing else removes it
        // while the engine runs.
        let xray_config = adopted.xray.as_ref().map(|_| {
            self.runtime_dir
                .join(profile_id.to_string())
                .join(crate::xray::XRAY_CONFIG_FILE)
        });
        self.active_sessions.insert(
            profile_id,
            ActiveSession {
                _profile_id: profile_id,
                browser: Held::Adopted(adopted.browser.clone()),
                xray: adopted.xray.clone().map(Held::Adopted),
                xray_config,
                _cdp_port: adopted.cdp_port,
                _socks_port: adopted.socks_port,
                // What the previous run launched it with is not in the record:
                // only the browser's own arguments are, and those are the ones
                // Chromium was told. Nothing reads this for an adopted session.
                _effective_args: Vec::new(),
                stopping: false,
                stop_failed: None,
            },
        );
        self.update_full_snapshot(RuntimeSnapshot {
            acknowledged_start: 0,
            profile_id,
            state: RuntimeState::Running,
            browser_pid: Some(adopted.browser.pid),
            xray_pid: adopted.xray.as_ref().map(|process| process.pid),
            cdp_port: Some(adopted.cdp_port),
            socks_port: adopted.socks_port,
            started_at: Some(std::time::UNIX_EPOCH + Duration::from_millis(adopted.started_at)),
            effective_args: Vec::new(),
            last_error: None,
            last_warning: None,
            dropped_events: 0,
        });
        self.emit(RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Running,
        });
    }

    /// Leaves every running session running, and ends the supervisor.
    ///
    /// This is the exit that is *not* a cleanup: the browsers and tunnels stay,
    /// and the records are marked so that the next run adopts them rather than
    /// treating them as a crash's leftovers. It is deliberately the last thing
    /// this run does to them - after this there is no handle left to stop them
    /// with, which is the point.
    ///
    /// A session whose record cannot be marked is still released: the processes
    /// are the user's, and the worst a missing mark can do is make the next run
    /// stop them, which is the safe direction. That failure is logged.
    pub fn release_all(&mut self) -> usize {
        let ids: Vec<ProfileId> = self.active_sessions.keys().copied().collect();
        for profile_id in ids {
            if let Err(error) = journal::mark_left_running(&self.runtime_dir, profile_id) {
                tracing::warn!(
                    "session {profile_id} was left running but its record could not be marked \
                     ({error}); the next start will stop it"
                );
            }
            if let Some(mut session) = self.active_sessions.remove(&profile_id) {
                session.stopping = true;
                if let Err(error) = session.browser.release() {
                    tracing::warn!(
                        "could not release browser {}: {error}",
                        session.browser.id()
                    );
                }
                if let Some(xray) = session.xray.as_mut()
                    && let Err(error) = xray.release()
                {
                    tracing::warn!("could not release xray {}: {error}", xray.id());
                }
                // Dropping the handles here is what leaves them: on Unix a child
                // is not killed when its handle goes, and on Windows the job's
                // kill-on-close limit was just cleared.
            }
            self.set_snapshot_state(profile_id, RuntimeState::Stopped);
        }
        self.active_sessions.len()
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
                let (id, request) = (params.profile.id, params.request_id);
                self.start_profile(params);
                self.acknowledge_start(id, request);
                true
            }
            RuntimeCommand::Stop(profile_id) => {
                self.stop_profile(profile_id);
                true
            }
            RuntimeCommand::Restart(params) => {
                let id = params.profile.id;
                let request = params.request_id;
                self.stop_profile(id);
                self.start_profile(params);
                self.acknowledge_start(id, request);
                true
            }
            RuntimeCommand::ReleaseAll => {
                // Before `shutting_down`, so the tail of the loop finds nothing
                // to clean up: releasing *is* the exit, and a cleanup after it
                // would undo it.
                self.release_all();
                self.shutting_down = true;
                self.pending_commands.clear();
                false
            }
            RuntimeCommand::ShutdownAll => {
                self.shutting_down = true;
                self.pending_commands.clear();
                self.cleanup_all();
                false
            }
        }
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
                && session
                    .browser
                    .exited(self.process_inspector.as_ref())
                    .is_none()
                && self
                    .cdp_probe
                    .close_browser(session._cdp_port, Duration::from_millis(250))
                    .is_ok()
            {
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                while session
                    .browser
                    .exited(self.process_inspector.as_ref())
                    .is_none()
                    && std::time::Instant::now() < deadline
                {
                    self.poll_active_sessions();
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            // Borrowed before the two kills: the inspector and the tree are
            // fields of `self`, which the session is being removed from.
            let inspector = self.process_inspector.as_ref();
            let tree = self.process_tree.as_ref();
            let mut failures = Vec::new();
            if let Err(error) = session.browser.kill(inspector, tree) {
                failures.push(format!("browser {error}"));
            }
            if let Some(child) = session.xray.as_mut()
                && let Err(error) = child.kill(inspector, tree)
            {
                failures.push(format!("xray {error}"));
            }

            if failures.is_empty() {
                remove_config(session.xray_config.as_deref());
                journal::remove(&self.runtime_dir, profile_id);
                self.publish_stopped(profile_id);
            } else {
                // Nothing is cleaned up. Whatever is still running is still
                // described by the record and by the temporary Xray config beside
                // it, so the next run's recovery is one way to reach it; the
                // session stays here, so the window's own stop is the other, and
                // a start cannot put a second browser on a directory this one
                // still holds.
                //
                // The state goes back to `Running` rather than to `Failed`,
                // because that is what is true: this profile has a live session,
                // it holds its ports and its data directory, and a window that
                // offered to start it again would be offering exactly the second
                // browser the paragraph above is about.
                let message = failures.join("; ");
                tracing::warn!("could not stop profile {profile_id}: {message}");
                session.stopping = false;
                session.stop_failed = Some(message.clone());
                self.active_sessions.insert(profile_id, session);
                self.set_snapshot_state(profile_id, RuntimeState::Running);
                self.emit(RuntimeEvent::Warning {
                    profile_id,
                    message: format!(
                        "profile {profile_id} could not be stopped and is still running: {message}"
                    ),
                });
                self.emit(RuntimeEvent::StateChanged {
                    profile_id,
                    state: RuntimeState::Running,
                });
            }
        } else {
            self.publish_stopped(profile_id);
        }
    }

    /// Says a profile is stopped, the one way it is ever said.
    fn publish_stopped(&mut self, profile_id: ProfileId) {
        self.set_snapshot_state(profile_id, RuntimeState::Stopped);
        self.emit(RuntimeEvent::Stopped { profile_id });
        self.emit(RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Stopped,
        });
    }

    fn poll_active_sessions(&mut self) {
        let mut exited = Vec::new();
        for (id, session) in &mut self.active_sessions {
            // A session whose stop already failed is left alone: the failure was
            // reported, the record still describes it, and a poll that retried it
            // on every tick would report the same failure over and over. An
            // explicit stop is what tries again.
            if session.stop_failed.is_some() {
                continue;
            }
            for (component, child) in
                std::iter::once((RuntimeComponent::Browser, &mut session.browser))
                    .chain(session.xray.as_mut().map(|c| (RuntimeComponent::Xray, c)))
            {
                let Some(gone) = child.exited(self.process_inspector.as_ref()) else {
                    continue;
                };
                // Only a browser that reported success is a normal exit - the
                // user closing the browser window. An Xray that exits is always
                // a problem, and a process that left no status is not evidence of
                // one either way.
                let clean =
                    component == RuntimeComponent::Browser && gone.succeeded.unwrap_or(true);
                exited.push((*id, (component, gone.detail, clean)));
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
                        acknowledged_start: 0,
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
                snapshot.acknowledged_start = previous.acknowledged_start;
                snapshot.dropped_events = previous.dropped_events;
                snapshot.last_error = previous.last_error.clone();
                snapshot.last_warning = previous.last_warning.clone();
            }
            lock.insert(snapshot.profile_id, snapshot);
        }
    }

    fn acknowledge_start(&self, id: ProfileId, request: u64) {
        if let Ok(mut snapshots) = self.snapshots.write()
            && let Some(snapshot) = snapshots.get_mut(&id)
        {
            snapshot.acknowledged_start = snapshot.acknowledged_start.max(request);
        }
    }
}

#[cfg(all(test, unix))]
mod tests;

/// The command queue's own rules, tested without anything to run.
///
/// Portable, and deliberately outside the module above: the supervisor's other
/// tests drive real processes through a shell script, which is why that module is
/// Unix-only, but a full queue and a closed channel are the same question on every
/// platform - and a rule tested on one of them is a rule the other can regress
/// without anyone noticing.
#[cfg(test)]
mod command_queue;
