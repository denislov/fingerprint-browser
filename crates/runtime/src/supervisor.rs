use crate::error::RuntimeCommandError;
use crate::events::{RuntimeCommand, RuntimeEvent};
use crate::facade::{RuntimeFacade, RuntimeSnapshot};
use crossbeam_channel::{Receiver, Sender};
use domain::{ProfileId, RuntimeState};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

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
    fn start(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError> {
        self.command_tx
            .send(RuntimeCommand::Start(profile_id))
            .map_err(|_| RuntimeCommandError::ChannelClosed)
    }

    fn stop(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError> {
        self.command_tx
            .send(RuntimeCommand::Stop(profile_id))
            .map_err(|_| RuntimeCommandError::ChannelClosed)
    }

    fn restart(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError> {
        self.command_tx
            .send(RuntimeCommand::Restart(profile_id))
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

pub struct RuntimeSupervisor {
    _command_rx: Receiver<RuntimeCommand>,
    _event_tx: Sender<RuntimeEvent>,
    snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
}

impl RuntimeSupervisor {
    pub fn new(
        command_rx: Receiver<RuntimeCommand>,
        event_tx: Sender<RuntimeEvent>,
        snapshots: Arc<RwLock<HashMap<ProfileId, RuntimeSnapshot>>>,
    ) -> Self {
        Self {
            _command_rx: command_rx,
            _event_tx: event_tx,
            snapshots,
        }
    }

    pub fn update_snapshot_state(&self, profile_id: ProfileId, state: RuntimeState) {
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
}
