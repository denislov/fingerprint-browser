//! Command handover and snapshot access.
use super::*;

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
