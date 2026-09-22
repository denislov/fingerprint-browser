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

/// A component the supervisor is responsible for, and how it came to be one.
///
/// A run that was told to leave its children running leaves no handle behind -
/// the handle died with that run - so the next run has only the session record: a
/// pid, the kernel start time that tells one process instance from the next, and
/// the ports. That is enough to watch, to stop and to restart, and it is *all*
/// there is, which is why the two cases are one type here rather than a special
/// case in every method that touches a child.
enum Held {
    /// Started by this run. The handle is the authority: it knows whether the
    /// process is alive without asking the kernel, and stopping it goes through
    /// the job or the process group this run created.
    Owned(crate::process::ManagedChild),
    /// Started by a run that deliberately left it running. The record is the
    /// authority, and every question about it is a question about identity -
    /// which is why it is answered by the same rule the journal uses.
    Adopted(ProcessRecord),
}

impl Held {
    fn id(&self) -> u32 {
        match self {
            Self::Owned(child) => child.id(),
            Self::Adopted(record) => record.pid,
        }
    }

    /// Why it is no longer running, or `None` while it is.
    ///
    /// "Cannot be asked" is deliberately not "gone": a platform that cannot read
    /// another process's identity must not turn into a crash report and a stopped
    /// profile.
    fn exited(&mut self, inspector: &dyn ProcessInspector) -> Option<Gone> {
        match self {
            Self::Owned(child) => match child.try_wait() {
                Ok(None) => None,
                Ok(Some(status)) => Some(Gone {
                    detail: status.to_string(),
                    succeeded: Some(status.success()),
                }),
                Err(error) => Some(Gone {
                    detail: error.to_string(),
                    succeeded: None,
                }),
            },
            Self::Adopted(record) => match journal::verdict(record, inspector) {
                journal::Verdict::Gone => Some(Gone {
                    detail: "the process is gone".to_string(),
                    // A process this run did not spawn has no exit status to
                    // read, so whether it left cleanly cannot be known. It is
                    // reported as a normal stop rather than guessed at as a
                    // crash: the process being gone is the fact, and inventing
                    // an alarm from it is the false report the clean flag exists
                    // to prevent.
                    succeeded: None,
                }),
                journal::Verdict::Ours => None,
                journal::Verdict::Foreign(reason) => Some(Gone {
                    detail: reason,
                    succeeded: None,
                }),
                // Nothing could be learned, so nothing may be decided.
                journal::Verdict::Unreadable(_) => None,
            },
        }
    }

    /// The handle behind an owned component, for a test that has to end a child
    /// the way only its owner can. An adopted component has no handle - which is
    /// the whole distinction - so asking for one is a mistake worth a panic.
    ///
    /// Gated exactly like the test module that is its only caller: `cfg(test)`
    /// alone leaves it unused on Windows, where the supervisor's tests do not
    /// compile because they spawn Unix processes, and the gate refuses warnings.
    #[cfg(all(test, unix))]
    fn owned_mut(&mut self) -> &mut crate::process::ManagedChild {
        match self {
            Self::Owned(child) => child,
            Self::Adopted(_) => panic!("an adopted session has no handle"),
        }
    }

    /// Stops it, and everything it started, or says why it could not.
    ///
    /// The answer matters for an adopted session and only for one: it is stopped
    /// by pid against a record that may no longer describe what is running there,
    /// so "the process is not this one" and "it did not exit" have to reach the
    /// caller rather than a log line - the caller is what decides whether the
    /// record may be thrown away. An owned child is a handle this process holds,
    /// so its `wait` is the confirmation and the result is always `Ok`.
    fn kill(
        &mut self,
        inspector: &dyn ProcessInspector,
        tree: &dyn ProcessTreeController,
    ) -> Result<(), String> {
        match self {
            Self::Owned(child) => {
                terminate_owned(tree, child);
                Ok(())
            }
            // No handle, so the recorded identity is rechecked at the moment of
            // termination and the tree is stopped by pid.
            Self::Adopted(record) => {
                journal::stop(record, inspector, tree, journal::DEFAULT_GRACE).map(|_| ())
            }
        }
    }

    /// Prepares to leave it running, and gives up the handle without stopping it.
    ///
    /// On Unix a child is not killed when its handle is dropped, so there is
    /// nothing to do. On Windows there is: every child is in a job whose limit is
    /// "kill everything in it when the last handle closes", which is exactly what
    /// makes a crashed manager take its browsers with it. Leaving them on purpose
    /// means clearing that limit first, and then the handle may close.
    fn release(&mut self) -> std::io::Result<()> {
        match self {
            Self::Owned(child) => crate::process::release_child(child),
            Self::Adopted(_) => Ok(()),
        }
    }
}

/// Everything one start acquires, in one value.
///
/// A start takes a CDP port, sometimes a SOCKS port, sometimes an Xray process
/// with a temporary config on disk, a browser process, and a session record. Each
/// of those has to be given back if anything later in the start goes wrong, and
/// there are five places in the start that can go wrong - which is five
/// hand-written rollbacks, each of which has to remember every resource acquired
/// before it. This is that list in one place: [`StartAttempt::drop`] ends what it
/// still holds, so a path out of the start that forgets to clean up does not
/// exist, and a sixth failure point added later cannot leak a browser.
///
/// On success the children are taken out and [`StartAttempt::keep`] is called:
/// the record and the config are the running session's then, and must not be
/// undone.
struct StartAttempt {
    profile_id: ProfileId,
    runtime_dir: std::path::PathBuf,
    /// The same tree controller the supervisor uses, shared rather than borrowed
    /// so this can outlive a borrow of it.
    tree: Arc<dyn ProcessTreeController>,
    /// Held until the child that will bind it starts. A reservation is a bound
    /// socket, so it cannot simply be kept: the browser has to be able to take
    /// the port.
    cdp: Option<crate::ports::PortReservation>,
    socks: Option<crate::ports::PortReservation>,
    browser: Option<crate::process::ManagedChild>,
    xray: Option<crate::process::ManagedChild>,
    /// The ports the children were given, and the arguments the browser was
    /// launched with: what the running session is described by.
    cdp_port: u16,
    socks_port: Option<u16>,
    effective_args: Vec<String>,
    /// The temporary Xray config, which holds upstream credentials. Removed on
    /// every path, including the ones that fail before it is written.
    xray_config: Option<std::path::PathBuf>,
    /// Whether a session record was written for this attempt.
    recorded: bool,
    /// Set by the cancellation polls, so the caller can tell "the start was
    /// called off" from "the start failed".
    cancelled: bool,
    kept: bool,
}

impl StartAttempt {
    fn new(
        profile_id: ProfileId,
        tree: Arc<dyn ProcessTreeController>,
        runtime_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            profile_id,
            runtime_dir,
            tree,
            cdp: None,
            socks: None,
            browser: None,
            xray: None,
            cdp_port: 0,
            socks_port: None,
            effective_args: Vec::new(),
            xray_config: None,
            recorded: false,
            cancelled: false,
            kept: false,
        }
    }

    /// The ports, once the plan has been built from them.
    fn note_ports(&mut self, cdp_port: u16, socks_port: Option<u16>, args: Vec<String>) {
        self.cdp_port = cdp_port;
        self.socks_port = socks_port;
        self.effective_args = args;
    }

    fn cdp(&self) -> u16 {
        self.cdp
            .as_ref()
            .expect("the CDP port is held before the plan is built from it")
            .port()
    }

    fn socks(&self) -> Option<u16> {
        self.socks.as_ref().map(crate::ports::PortReservation::port)
    }

    fn browser_id(&self) -> u32 {
        self.browser
            .as_ref()
            .expect("the browser is held before its identifier is asked for")
            .id()
    }

    fn xray_id(&self) -> Option<u32> {
        self.xray.as_ref().map(crate::process::ManagedChild::id)
    }

    fn hold_cdp(&mut self, reservation: crate::ports::PortReservation) {
        self.cdp = Some(reservation);
    }

    fn hold_socks(&mut self, reservation: crate::ports::PortReservation) {
        self.socks = Some(reservation);
    }

    /// Lets go of a port so the child that is about to start can bind it.
    fn release_cdp(&mut self) {
        drop(self.cdp.take());
    }

    fn release_socks(&mut self) {
        drop(self.socks.take());
    }

    fn hold_browser(&mut self, child: crate::process::ManagedChild) {
        self.browser = Some(child);
    }

    fn hold_xray(&mut self, child: crate::process::ManagedChild) {
        self.xray = Some(child);
    }

    fn browser_mut(&mut self) -> &mut crate::process::ManagedChild {
        self.browser
            .as_mut()
            .expect("the browser is held before anything asks about it")
    }

    fn xray_mut(&mut self) -> Option<&mut crate::process::ManagedChild> {
        self.xray.as_mut()
    }

    fn xray_status(&mut self) -> Option<std::io::Result<Option<std::process::ExitStatus>>> {
        self.xray
            .as_mut()
            .map(crate::process::ManagedChild::try_wait)
    }

    /// Remembers the config this attempt will have written, before it is written:
    /// a failure partway through writing it still has to remove it.
    fn note_config(&mut self, path: Option<std::path::PathBuf>) {
        self.xray_config = path;
    }

    fn note_record(&mut self) {
        self.recorded = true;
    }

    /// Somewhere the start checks whether it has been called off.
    fn cancelled(&self) -> bool {
        self.cancelled
    }

    fn note_cancelled(&mut self, cancelled: bool) {
        self.cancelled = cancelled;
    }

    /// The start succeeded: these children are the running session's now, and the
    /// record and config describe a live session rather than a failed attempt.
    fn keep(&mut self) {
        self.kept = true;
    }

    fn take_browser(&mut self) -> crate::process::ManagedChild {
        self.browser.take().expect("a kept attempt has a browser")
    }

    fn take_xray(&mut self) -> Option<crate::process::ManagedChild> {
        self.xray.take()
    }

    /// The config path, for the session that will own it.
    fn config(&self) -> Option<&std::path::Path> {
        self.xray_config.as_deref()
    }
}

impl Drop for StartAttempt {
    /// Undoes whatever the start acquired and did not hand over.
    ///
    /// Children first, then the files that describe them: a record removed before
    /// its process is gone is a process nothing can find, and the terminal moment
    /// of a failed start is exactly when that matters.
    fn drop(&mut self) {
        if self.kept {
            return;
        }
        if let Some(child) = self.browser.as_mut() {
            terminate_owned(self.tree.as_ref(), child);
        }
        if let Some(child) = self.xray.as_mut() {
            terminate_owned(self.tree.as_ref(), child);
        }
        remove_config(self.xray_config.as_deref());
        if self.recorded {
            journal::remove(&self.runtime_dir, self.profile_id);
        }
    }
}

/// The CDP port of an attempt, for the probe that is waiting on it.
///
/// A free function because the probe takes the port while the attempt is borrowed
/// mutably by the check that ran just before it.
fn cdp_port_of(attempt: &StartAttempt) -> u16 {
    attempt.cdp_port
}

/// Why a start did not become a session.
struct StartFailure {
    message: String,
    /// Whether the start was called off rather than failing: a cancelled start is
    /// not an error to show the user, and it leaves the profile stopped.
    cancelled: bool,
}

impl StartFailure {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: false,
        }
    }

    fn cancelled(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cancelled: true,
        }
    }
}

/// Ends a child this program started, and the tree under it.
///
/// One function, called by the start's own value on its way out and by the
/// supervisor for a session that is being stopped, because a browser is a process
/// group and a job object rather than one process: two rules for ending one would
/// be two chances to leave a renderer behind.
fn terminate_owned(tree: &dyn ProcessTreeController, child: &mut crate::process::ManagedChild) {
    #[cfg(windows)]
    if let Err(error) = child.terminate_tree() {
        tracing::error!("failed to terminate managed job: {error}");
    }
    if matches!(child.try_wait(), Ok(None)) {
        #[cfg(unix)]
        let _ = tree.terminate_tree(child.id());
        // A direct kill is a fallback if the platform tree controller fails.
        let _ = child.kill();
    }
    let _ = child.wait();
}

/// Removes a temporary Xray config, which holds the upstream credentials.
fn remove_config(path: Option<&std::path::Path>) {
    let Some(path) = path else {
        return;
    };
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("failed to remove the temporary Xray config {path:?}: {error}");
    }
}

/// A component that is no longer running, and how much can be said about why.
struct Gone {
    detail: String,
    /// The exit status, where there is one to read. `None` for a process this run
    /// did not spawn, and for a status that could not be read.
    succeeded: Option<bool>,
}

struct ActiveSession {
    _profile_id: ProfileId,
    browser: Held,
    xray: Option<Held>,
    xray_config: Option<std::path::PathBuf>,
    _cdp_port: u16,
    _socks_port: Option<u16>,
    _effective_args: Vec<String>,
    stopping: bool,
    /// Why the last attempt to stop this session failed, if one did.
    ///
    /// A session that could not be stopped stays here, and this is what keeps the
    /// poll from reporting the same failure on every tick: the process is still
    /// there and there is nothing new to learn about it. An explicit stop is what
    /// tries again.
    stop_failed: Option<String>,
}

/// How long the one command that waits is willing to wait.
///
/// `ReleaseAll` is what a run does last, and leaving the running browsers behind
/// it is the whole point of it, so it waits for the supervisor to take it - but
/// not forever: a supervisor that has not taken it in this long is not going to,
/// and an exit that never finishes is worse than an exit that warns.
pub const HANDOVER_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct ChannelRuntimeFacade {
    command_tx: Sender<RuntimeCommand>,
    snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
    handover_timeout: Duration,
}

impl ChannelRuntimeFacade {
    pub fn new(
        command_tx: Sender<RuntimeCommand>,
        snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
    ) -> Self {
        Self {
            command_tx,
            snapshots,
            handover_timeout: HANDOVER_TIMEOUT,
        }
    }

    /// The same facade with a different patience, for a test that has to see it
    /// run out.
    #[cfg(test)]
    pub fn with_handover_timeout(mut self, timeout: Duration) -> Self {
        self.handover_timeout = timeout;
        self
    }

    /// Hands a command over without waiting for the supervisor to pick it up.
    ///
    /// This is called from the window's own thread, and the supervisor is
    /// regularly busy for seconds at a time - a readiness wait, a process tree
    /// being killed, a shutdown - so a blocking send is a frozen window. A queue
    /// that is full means a hundred-odd commands the supervisor has not reached,
    /// which is a runtime that has stopped keeping up rather than a click that
    /// arrived too quickly; refusing it says so, and says it immediately.
    fn send(&self, command: RuntimeCommand) -> Result<(), RuntimeCommandError> {
        self.command_tx
            .try_send(command)
            .map_err(|error| match error {
                crossbeam_channel::TrySendError::Full(_) => RuntimeCommandError::Busy,
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    RuntimeCommandError::ChannelClosed
                }
            })
    }
}

impl RuntimeFacade for ChannelRuntimeFacade {
    fn start(&self, params: StartParams) -> Result<(), RuntimeCommandError> {
        self.send(RuntimeCommand::Start(params))
    }

    fn stop(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError> {
        self.send(RuntimeCommand::Stop(profile_id))
    }

    fn restart(&self, params: StartParams) -> Result<(), RuntimeCommandError> {
        self.send(RuntimeCommand::Restart(params))
    }

    /// The one command that waits, because it is the last one: see
    /// [`HANDOVER_TIMEOUT`].
    fn release_all(&self) -> Result<(), RuntimeCommandError> {
        self.command_tx
            .send_timeout(RuntimeCommand::ReleaseAll, self.handover_timeout)
            .map_err(|error| match error {
                crossbeam_channel::SendTimeoutError::Timeout(_) => RuntimeCommandError::Busy,
                crossbeam_channel::SendTimeoutError::Disconnected(_) => {
                    RuntimeCommandError::ChannelClosed
                }
            })
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

        // Everything this attempt acquires lives in one value from here on, and
        // every way out of this function that is not the session below drops it -
        // which ends the children it still holds, removes the temporary config and
        // removes the record. The five hand-written rollbacks that used to be here
        // each had to remember every resource acquired before them, and a sixth
        // failure point added later would have had to remember them too.
        let (mut attempt, record) = match self.begin_attempt(&params) {
            Ok(started) => started,
            Err(failure) => {
                // The attempt was dropped inside `begin_attempt`: nothing is
                // running by the time the profile is told it failed.
                self.report_start_failure(profile_id, failure);
                return;
            }
        };

        // Probe CDP readiness. The record is on disk and both children are
        // running; this is the part that decides whether they become a session.
        let deadline = std::time::Instant::now() + self.cdp_ready_timeout;
        let readiness = loop {
            attempt.note_cancelled(self.poll_start_commands(profile_id));
            if attempt.cancelled() {
                break Err("startup cancelled".to_string());
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break Err("CDP readiness timed out".to_string());
            }
            let live = (|| -> Result<(), String> {
                if let Some(status) = attempt
                    .browser_mut()
                    .try_wait()
                    .map_err(|e| e.to_string())?
                {
                    return Err(format!("browser exited during CDP readiness: {status}"));
                }
                if let Some(state) = attempt.xray_status()
                    && let Some(status) = state.map_err(|e| e.to_string())?
                {
                    return Err(format!("Xray exited during CDP readiness: {status}"));
                }
                Ok(())
            })();
            if let Err(error) = live {
                break Err(error);
            }
            match self.cdp_probe.wait_ready(
                cdp_port_of(&attempt),
                remaining.min(Duration::from_millis(100)),
            ) {
                Ok(info) => {
                    attempt.note_cancelled(self.poll_start_commands(profile_id));
                    if attempt.cancelled() {
                        break Err("startup cancelled".to_string());
                    }
                    if !matches!(attempt.browser_mut().try_wait(), Ok(None)) {
                        break Err("browser exited during CDP readiness".into());
                    }
                    // Recheck Xray after the probe before advertising Running.
                    if let Some(status) = attempt.xray_status()
                        && !matches!(status, Ok(None))
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
                let cancelled = attempt.cancelled();
                debug_assert!(!cancelled, "a cancelled start never reports readiness");
                let browser = attempt.take_browser();
                let xray = attempt.take_xray();
                let xray_config = attempt.config().map(std::path::Path::to_path_buf);
                let (cdp_port, socks_port, effective_args) = (
                    attempt.cdp_port,
                    attempt.socks_port,
                    attempt.effective_args.clone(),
                );
                // From here the children, the config and the record belong to the
                // running session, and must not be undone by the drop below.
                attempt.keep();

                let browser_pid = record.browser.pid;
                let xray_pid = record.xray.as_ref().map(|xray| xray.pid);
                let session = ActiveSession {
                    _profile_id: profile_id,
                    browser: Held::Owned(browser),
                    xray: xray.map(Held::Owned),
                    xray_config,
                    _cdp_port: cdp_port,
                    _socks_port: socks_port,
                    _effective_args: effective_args.clone(),
                    stopping: false,
                    stop_failed: None,
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
                // Dropping the attempt is the rollback: the browser, the Xray
                // process, the temporary config and the record all go, in that
                // order, whatever failed.
                let failure = StartFailure {
                    message: format!("CDP readiness probe failed: {e}"),
                    cancelled: attempt.cancelled(),
                };
                drop(attempt);
                self.report_start_failure(profile_id, failure);
            }
        }
    }

    /// Everything a start does before the browser is ready to be probed.
    ///
    /// Acquires the ports, resolves capabilities, builds the plan, starts Xray and
    /// Chromium and writes the session record - and returns the value that owns
    /// all of it, or the reason there is nothing to own. It does not decide the
    /// profile's state: its caller is what turns either answer into a snapshot and
    /// an event, so every failure leaves the same way.
    fn begin_attempt(
        &mut self,
        params: &StartParams,
    ) -> Result<(StartAttempt, SessionRecord), StartFailure> {
        let profile_id = params.profile.id;
        let mut attempt = StartAttempt::new(
            profile_id,
            Arc::clone(&self.process_tree),
            self.runtime_dir.clone(),
        );

        // 1. Allocate CDP port
        let cdp_reservation = self
            .port_allocator
            .reserve_loopback()
            .map_err(|e| StartFailure::failed(format!("port allocation failed: {e}")))?;
        attempt.hold_cdp(cdp_reservation);

        // 2. Allocate SOCKS port if proxy present
        if params.proxy.is_some() {
            let socks_reservation = self
                .port_allocator
                .reserve_loopback()
                .map_err(|e| StartFailure::failed(format!("socks port allocation failed: {e}")))?;
            attempt.hold_socks(socks_reservation);
        }
        let cdp_port = attempt.cdp();
        let socks_port = attempt.socks();

        // 3. Resolve capabilities
        let capabilities = self
            .capability_resolver
            .resolve(&params.core)
            .map_err(|e| StartFailure::failed(format!("capability error: {e}")))?;

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

        let plan = self
            .planner
            .build(ctx)
            .map_err(|e| StartFailure::failed(format!("launch plan error: {e}")))?;

        let effective_args: Vec<String> = plan
            .browser_args
            .iter()
            .map(|a| a.to_string_lossy().to_string())
            .collect();

        self.emit(RuntimeEvent::EffectiveLaunchArgs {
            profile_id,
            args: effective_args.clone(),
        });
        attempt.note_ports(cdp_port, socks_port, effective_args);

        // The config path is remembered before it is written, so a failure
        // partway through writing it still removes it.
        attempt.note_config(plan.xray.as_ref().map(|xray| xray.config_path.clone()));

        // Prepare and verify Xray before Chromium can issue any requests.
        if let Some(xray_plan) = &plan.xray {
            let proxy = params
                .proxy
                .as_ref()
                .ok_or_else(|| StartFailure::failed("missing proxy configuration"))?;
            self.xray_builder
                .build(proxy, xray_plan.socks_port, &xray_plan.config_path)
                .map_err(|e| StartFailure::failed(e.to_string()))?;

            let mut command = std::process::Command::new(&xray_plan.executable);
            command
                .args(xray_plan.args())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            // Xray binds its own socket; release only at the handoff.
            attempt.release_socks();

            let child = crate::process::spawn_managed(&mut command)
                .map_err(|e| StartFailure::failed(format!("Xray spawn failed: {e}")))?;
            attempt.hold_xray(child);

            let mut cancelled = false;
            let ready = {
                let child = attempt
                    .xray_mut()
                    .expect("the Xray process was held just above");
                crate::xray::wait_ready(
                    child,
                    xray_plan.socks_port,
                    self.xray_ready_timeout,
                    || {
                        cancelled = self.poll_start_commands(profile_id);
                        !cancelled
                    },
                )
            };
            if let Err(error) = ready {
                // Dropped on the way out, which ends the Xray process and removes
                // the config it was reading.
                return Err(if cancelled {
                    StartFailure::cancelled(error.to_string())
                } else {
                    StartFailure::failed(error.to_string())
                });
            }
        }

        if self.poll_start_commands(profile_id) {
            return Err(StartFailure::cancelled("startup cancelled"));
        }

        // 5. Spawn Chromium
        let mut cmd = std::process::Command::new(&plan.browser_executable);
        cmd.args(&plan.browser_args);
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::null());
        cmd.stderr(std::process::Stdio::null());

        attempt.release_cdp();
        let child = crate::process::spawn_managed(&mut cmd).map_err(|e| {
            StartFailure::failed(format!(
                "failed to spawn executable {:?}: {e}",
                plan.browser_executable
            ))
        })?;
        attempt.hold_browser(child);

        let browser_pid = attempt.browser_id();
        let xray_pid = attempt.xray_id();

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
            left_running: false,
            browser: ProcessRecord::captured(
                browser_pid,
                &plan.browser_executable,
                &attempt.effective_args,
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
        // Whether or not the write landed, a failure from here removes whatever
        // is there: a half-written record is worse than none.
        attempt.note_record();

        Ok((attempt, record))
    }

    /// Publishes what a start that did not become a session leaves behind.
    ///
    /// A start that was called off is stopped; a start that went wrong is failed,
    /// with the reason. Either way everything it acquired has already been given
    /// back: the attempt was dropped before this is called.
    fn report_start_failure(&mut self, profile_id: ProfileId, failure: StartFailure) {
        if failure.cancelled {
            self.stop_profile(profile_id);
        } else {
            self.fail_start(profile_id, failure.message);
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
    use crate::process::{ProcessIdentity, ProcessReading};
    use crate::{CdpError, CdpInfo, LaunchPlanError, ProcessError};
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
        /// The parameters this fixture starts with, for a test that drives the
        /// facade rather than the supervisor.
        fn params(&self) -> StartParams {
            self.params.clone()
        }

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
            f.supervisor.recover_orphans().is_empty(),
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

        let report = f.supervisor.recover_orphans();

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
        xray.owned_mut().kill().unwrap();
        xray.owned_mut().wait().unwrap();
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
        child.owned_mut().kill().unwrap();
        child.owned_mut().wait().unwrap();
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

    /// The command channel is bounded, and the window's own thread is what fills
    /// it. A send that waits for room is a frozen window while the supervisor is
    /// inside a readiness wait or a process-tree kill, so a full queue is refused
    /// - immediately, and with an answer that says trying again is worth it.
    #[test]
    fn a_full_command_queue_is_refused_rather_than_waited_on() {
        let (command_tx, command_rx) = crossbeam_channel::bounded(1);
        let facade = ChannelRuntimeFacade::new(
            command_tx,
            Arc::new(RwLock::new(HashMap::<ProfileId, RuntimeSnapshot>::new())),
        );
        let params = Fixture::new(true, true, XRAY).params();

        // Nobody is draining: the second command has nowhere to go.
        facade
            .start(params.clone())
            .expect("room for the first command");
        let refused = facade
            .stop(params.profile.id)
            .expect_err("the queue is full and this must not wait for it");
        assert!(matches!(refused, RuntimeCommandError::Busy), "{refused}");
        assert!(
            matches!(facade.start(params.clone()), Err(RuntimeCommandError::Busy)),
            "and it is refused for every command, not just the one"
        );

        // A supervisor that is gone, on the other hand, is not a queue to retry:
        // the two answers have to stay different.
        drop(command_rx);
        assert!(matches!(
            facade.restart(params),
            Err(RuntimeCommandError::ChannelClosed)
        ));
    }

    /// Leaving the running sessions behind is the last thing a run does, so it is
    /// the one command that waits for room - and it stops waiting, because an
    /// exit that never finishes is worse than one that warns.
    #[test]
    fn the_handover_waits_for_room_but_not_forever() {
        let (command_tx, command_rx) = crossbeam_channel::bounded(1);
        let facade = ChannelRuntimeFacade::new(
            command_tx,
            Arc::new(RwLock::new(HashMap::<ProfileId, RuntimeSnapshot>::new())),
        )
        .with_handover_timeout(Duration::from_millis(50));
        let params = Fixture::new(true, true, XRAY).params();
        facade.start(params).expect("room for the first command");

        let started = std::time::Instant::now();
        let refused = facade
            .release_all()
            .expect_err("a full queue it cannot wait out");
        let waited = started.elapsed();
        assert!(matches!(refused, RuntimeCommandError::Busy), "{refused}");
        assert!(
            waited >= Duration::from_millis(50) && waited < Duration::from_secs(2),
            "it waits for the timeout and then gives up: {waited:?}"
        );

        // With room it is taken, which is the ordinary exit.
        command_rx.recv().expect("the first command");
        facade
            .release_all()
            .expect("the command was taken once there was room");
        assert!(matches!(command_rx.recv(), Ok(RuntimeCommand::ReleaseAll)));
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
        xray.owned_mut().kill().unwrap();
        xray.owned_mut().wait().unwrap();
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
        session.xray.as_mut().unwrap().owned_mut().kill().unwrap();
        session.xray.as_mut().unwrap().owned_mut().wait().unwrap();
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
            // The record and the processes are the other two things a start
            // acquires. Which of the four cases reached the record differs - two
            // fail before both children exist - but none of them may leave one
            // behind, and none may leave a process for the next run to reclaim.
            assert!(
                journal::read(&f.dir, f.params.profile.id)
                    .expect("the journal is readable")
                    .is_none(),
                "a rolled back start left its record behind"
            );
            assert!(
                f.supervisor.recover_orphans().is_empty(),
                "a rolled back start left a process running"
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
        session.browser.owned_mut().kill().unwrap();
        session.browser.owned_mut().wait().unwrap();
        f.supervisor.poll_active_sessions();
        f.supervisor.stop_profile(id);
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(f.snapshot().xray_pid.is_none());
        assert!(!f.config().exists());
        #[cfg(target_os = "linux")]
        assert!(!PathBuf::from(format!("/proc/{xray_pid}")).exists());
    }

    /// An inspector that reports every pid as a process with someone else's
    /// command line: "this is not the process the record describes", which is one
    /// of the answers a stop has to be able to give.
    struct SomeoneElse;
    impl ProcessInspector for SomeoneElse {
        fn inspect(&self, _: u32) -> ProcessReading {
            ProcessReading::Live(ProcessIdentity {
                argv: vec!["/usr/bin/something-else".to_string()],
                start_time: Some(1),
            })
        }
    }

    /// An inspector that reports every pid as gone.
    struct Nobody;
    impl ProcessInspector for Nobody {
        fn inspect(&self, _: u32) -> ProcessReading {
            ProcessReading::Absent
        }
    }

    /// A tree controller that refuses to terminate anything, for the other half
    /// of "the stop did not happen".
    struct Refuses;
    impl ProcessTreeController for Refuses {
        fn terminate_tree(&self, pid: u32) -> Result<(), ProcessError> {
            Err(ProcessError::TerminationFailed(format!(
                "pid {pid} refused"
            )))
        }

        fn terminate_instance(&self, pid: u32, _: Option<u64>) -> Result<(), ProcessError> {
            Err(ProcessError::TerminationFailed(format!(
                "pid {pid} refused"
            )))
        }
    }

    /// An adopted session as recovery installs one: the record on disk, the
    /// temporary Xray config beside it, and no handle.
    fn adopt(f: &mut Fixture, browser_pid: u32, with_xray: bool) -> journal::Adopted {
        let profile_id = f.params.profile.id;
        let browser = ProcessRecord {
            pid: browser_pid,
            executable: "/bin/sleep".into(),
            args: vec!["60".into()],
            start_time: Some(7),
        };
        let xray = with_xray.then(|| ProcessRecord {
            pid: browser_pid + 1,
            executable: "/bin/sleep".into(),
            args: vec!["60".into()],
            start_time: Some(8),
        });
        let record = SessionRecord {
            profile_id,
            cdp_port: 9222,
            socks_port: Some(1080),
            started_at: 1_700_000_000_000,
            left_running: true,
            browser: browser.clone(),
            xray: xray.clone(),
        };
        journal::write(&f.dir, &record).expect("write the record");
        let config = f
            .dir
            .join(profile_id.to_string())
            .join(crate::xray::XRAY_CONFIG_FILE);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, "{}").unwrap();

        let adopted = journal::Adopted {
            profile_id,
            cdp_port: record.cdp_port,
            socks_port: record.socks_port,
            started_at: record.started_at,
            browser,
            xray,
        };
        f.supervisor.install_adopted(&adopted);
        adopted
    }

    /// A session this run adopted has no handle, so it is stopped by pid against
    /// a record that may no longer describe what is running there. That answer has
    /// to reach the caller: the record is the only thing that can still reach
    /// whatever is running, and publishing Stopped over it would leave a live
    /// browser that nothing tracks, that no start refuses, and that the next run
    /// has no reason to look for.
    #[test]
    fn a_stop_that_cannot_reach_an_adopted_process_keeps_the_session_and_its_record() {
        let mut f = Fixture::new(false, false, XRAY);
        let adopted = adopt(&mut f, 4242, true);
        let profile_id = adopted.profile_id;
        f.supervisor.process_inspector = Arc::new(SomeoneElse);

        f.supervisor.stop_profile(profile_id);

        // Not Stopped, and nothing was thrown away.
        assert_eq!(f.snapshot().state, RuntimeState::Running);
        assert!(f.supervisor.active_sessions.contains_key(&profile_id));
        assert!(
            journal::read(&f.dir, profile_id).unwrap().is_some(),
            "the record still describes the running process"
        );
        assert!(f.config().exists(), "the Xray config was not removed");
        let warning = std::iter::from_fn(|| f.events.try_recv().ok())
            .find_map(|event| match event {
                RuntimeEvent::Warning { message, .. } => Some(message),
                _ => None,
            })
            .expect("the window is told the stop did not happen");
        assert!(warning.contains("could not be stopped"), "{warning}");
        assert!(
            !std::iter::from_fn(|| f.events.try_recv().ok())
                .any(|event| matches!(event, RuntimeEvent::Stopped { .. })),
            "a stop that did not happen must not report one"
        );

        // The process is gone now, so the retry the session stayed for works:
        // everything a normal stop cleans up is cleaned up.
        f.supervisor.process_inspector = Arc::new(Nobody);
        f.supervisor.stop_profile(profile_id);
        assert_eq!(f.snapshot().state, RuntimeState::Stopped);
        assert!(!f.supervisor.active_sessions.contains_key(&profile_id));
        assert!(journal::read(&f.dir, profile_id).unwrap().is_none());
        assert!(!f.config().exists());
    }

    /// A tree that refuses to terminate a process it did recognise is the same
    /// answer by the other route, and the same handling.
    #[test]
    fn a_stop_a_process_tree_refuses_keeps_the_session_and_its_record() {
        let mut f = Fixture::new(false, false, XRAY);
        let adopted = adopt(&mut f, 4343, false);
        let profile_id = adopted.profile_id;
        f.supervisor.process_tree = Arc::new(Refuses);
        // The inspector says the pid is still ours, so the stop gets as far as
        // asking the tree to end it - and the grace period is what it spends
        // waiting first.
        f.supervisor.process_inspector = Arc::new(Ours);

        f.supervisor.stop_profile(profile_id);

        assert_eq!(f.snapshot().state, RuntimeState::Running);
        assert!(f.supervisor.active_sessions.contains_key(&profile_id));
        assert!(journal::read(&f.dir, profile_id).unwrap().is_some());
    }

    /// An inspector that reports the recorded process, so a stop escalates to the
    /// tree.
    struct Ours;
    impl ProcessInspector for Ours {
        fn inspect(&self, _: u32) -> ProcessReading {
            ProcessReading::Live(ProcessIdentity {
                argv: vec!["/bin/sleep".to_string(), "60".to_string()],
                start_time: Some(7),
            })
        }
    }
}
