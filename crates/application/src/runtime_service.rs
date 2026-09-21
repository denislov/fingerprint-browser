use crate::error::AppError;
use domain::ProfileId;
use runtime::{RuntimeFacade, RuntimeSnapshot, StartParams};
use std::sync::Arc;
use storage::{CoreRepository, ProfileRepository, ProxyRepository};

pub struct RuntimeService {
    profile_repo: Arc<dyn ProfileRepository>,
    core_repo: Arc<dyn CoreRepository>,
    proxy_repo: Arc<dyn ProxyRepository>,
    runtime: Arc<dyn RuntimeFacade>,
}

impl RuntimeService {
    pub fn new(
        profile_repo: Arc<dyn ProfileRepository>,
        core_repo: Arc<dyn CoreRepository>,
        proxy_repo: Arc<dyn ProxyRepository>,
        runtime: Arc<dyn RuntimeFacade>,
    ) -> Self {
        Self {
            profile_repo,
            core_repo,
            proxy_repo,
            runtime,
        }
    }

    /// Ends this run with every running session left running.
    ///
    /// The one command that is not about a profile: "leave them all" is a
    /// decision about this program, and the runtime is what knows which sessions
    /// there are.
    pub fn release_all(&self) -> Result<(), AppError> {
        self.runtime.release_all().map_err(AppError::Runtime)
    }

    pub fn start(&self, profile_id: ProfileId) -> Result<(), AppError> {
        let profile = self
            .profile_repo
            .get(profile_id)?
            .ok_or_else(|| AppError::NotFound(format!("profile {profile_id} not found")))?;

        let core = self
            .core_repo
            .get(profile.core_id)?
            .ok_or_else(|| AppError::NotFound(format!("core {} not found", profile.core_id)))?;

        let proxy = match profile.proxy_id {
            Some(pid) => Some(
                self.proxy_repo
                    .get(pid)?
                    .ok_or_else(|| AppError::NotFound(format!("proxy {pid} not found")))?,
            ),
            None => None,
        };

        let params = StartParams {
            profile,
            core,
            proxy,
        };

        self.runtime.start(params)?;
        Ok(())
    }

    pub fn stop(&self, profile_id: ProfileId) -> Result<(), AppError> {
        self.runtime.stop(profile_id)?;
        Ok(())
    }

    pub fn restart(&self, profile_id: ProfileId) -> Result<(), AppError> {
        let profile = self
            .profile_repo
            .get(profile_id)?
            .ok_or_else(|| AppError::NotFound(format!("profile {profile_id} not found")))?;

        let core = self
            .core_repo
            .get(profile.core_id)?
            .ok_or_else(|| AppError::NotFound(format!("core {} not found", profile.core_id)))?;

        let proxy = match profile.proxy_id {
            Some(pid) => Some(
                self.proxy_repo
                    .get(pid)?
                    .ok_or_else(|| AppError::NotFound(format!("proxy {pid} not found")))?,
            ),
            None => None,
        };

        let params = StartParams {
            profile,
            core,
            proxy,
        };

        self.runtime.restart(params)?;
        Ok(())
    }

    pub fn snapshot(&self, profile_id: ProfileId) -> Option<RuntimeSnapshot> {
        self.runtime.snapshot(profile_id)
    }
}
