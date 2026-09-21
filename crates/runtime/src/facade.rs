use crate::error::RuntimeCommandError;
use crate::events::StartParams;
use domain::{ProfileId, RuntimeState};
use std::time::SystemTime;

#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub profile_id: ProfileId,
    pub state: RuntimeState,
    pub browser_pid: Option<u32>,
    pub xray_pid: Option<u32>,
    pub cdp_port: Option<u16>,
    pub socks_port: Option<u16>,
    pub started_at: Option<SystemTime>,
    pub effective_args: Vec<String>,
    /// Latest diagnostics survive stop and event delivery loss until the next start.
    pub last_error: Option<String>,
    pub last_warning: Option<String>,
    /// Cumulative notification loss; consumers must reconcile from snapshots.
    pub dropped_events: u64,
}

pub trait RuntimeFacade: Send + Sync {
    fn start(&self, params: StartParams) -> Result<(), RuntimeCommandError>;
    fn stop(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError>;
    fn restart(&self, params: StartParams) -> Result<(), RuntimeCommandError>;
    /// End this run with every running session left running.
    fn release_all(&self) -> Result<(), RuntimeCommandError>;
    fn snapshot(&self, profile_id: ProfileId) -> Option<RuntimeSnapshot>;
}
