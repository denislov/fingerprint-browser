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
}

pub trait RuntimeFacade: Send + Sync {
    fn start(&self, params: StartParams) -> Result<(), RuntimeCommandError>;
    fn stop(&self, profile_id: ProfileId) -> Result<(), RuntimeCommandError>;
    fn restart(&self, params: StartParams) -> Result<(), RuntimeCommandError>;
    fn snapshot(&self, profile_id: ProfileId) -> Option<RuntimeSnapshot>;
}
