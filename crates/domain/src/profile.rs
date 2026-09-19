use crate::fingerprint::FingerprintProfile;
use crate::id::{CoreId, ProfileId, ProxyId};
use crate::start_target::StartTarget;
use crate::window::WindowProfile;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserProfile {
    pub id: ProfileId,
    pub name: String,
    pub core_id: CoreId,
    pub user_data_dir: PathBuf,
    pub fingerprint: FingerprintProfile,
    pub proxy_id: Option<ProxyId>,
    pub window: WindowProfile,
    pub start_target: StartTarget,
}

impl BrowserProfile {
    pub fn duplicate(
        &self,
        new_id: ProfileId,
        new_name: String,
        new_user_data_dir: PathBuf,
        new_seed: u32,
    ) -> Self {
        let mut cloned = self.clone();
        cloned.id = new_id;
        cloned.name = new_name;
        cloned.user_data_dir = new_user_data_dir;
        cloned.fingerprint.seed = new_seed;
        cloned
    }
}
