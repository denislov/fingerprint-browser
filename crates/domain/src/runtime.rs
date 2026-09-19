use crate::id::ProfileId;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RuntimeState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Failed { message: String },
    Crashed { message: String },
}

impl RuntimeState {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSession {
    pub profile_id: ProfileId,
    pub state: RuntimeState,
    pub browser_pid: Option<u32>,
    pub xray_pid: Option<u32>,
    pub cdp_port: Option<u16>,
    pub socks_port: Option<u16>,
    pub started_at: Option<SystemTime>,
    pub effective_args: Vec<String>,
}

impl RuntimeSession {
    pub fn new(profile_id: ProfileId) -> Self {
        Self {
            profile_id,
            state: RuntimeState::Stopped,
            browser_pid: None,
            xray_pid: None,
            cdp_port: None,
            socks_port: None,
            started_at: None,
            effective_args: Vec::new(),
        }
    }
}
