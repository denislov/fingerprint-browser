use crate::error::AppError;
use domain::{
    BrowserProfile, CoreId, FingerprintProfile, ProfileId, ProxyId, StartTarget, WindowProfile,
    validate_profile,
};
use std::path::PathBuf;
use std::sync::Arc;
use storage::ProfileRepository;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    KeepUserData,
    RemoveUserData,
}

#[derive(Debug, Clone)]
pub struct NewProfile {
    pub name: String,
    pub core_id: CoreId,
    pub user_data_dir: Option<PathBuf>,
    pub fingerprint: Option<FingerprintProfile>,
    pub proxy_id: Option<ProxyId>,
    pub window: Option<WindowProfile>,
    pub start_target: Option<StartTarget>,
}

/// Where a profile's browser data goes when nothing else says.
///
/// One function because two places have to agree: [`DefaultProfileService::create`]
/// gives a new profile this path, and an import gives it to a profile whose
/// recorded directory is not on the machine reading the file. Two copies of the
/// rule would let a created profile and an imported one live in different places
/// under the same identifier - which is precisely the pairing the identifier
/// exists to keep.
pub fn default_user_data_dir(base: &std::path::Path, id: ProfileId) -> PathBuf {
    base.join("profiles").join(id.to_string())
}

pub trait ProfileService: Send + Sync {
    fn create(&self, draft: NewProfile) -> Result<BrowserProfile, AppError>;
    fn update(&self, profile: BrowserProfile) -> Result<(), AppError>;
    /// Stores a profile exactly as given, keeping its identifier and its path.
    ///
    /// For import. [`ProfileService::create`] would mint a new identifier and
    /// derive a new directory, and both are the things a restored profile has to
    /// keep: the identifier is what its browser data is named after. The rules
    /// are still the domain's, so a profile with no name is refused here rather
    /// than stored.
    fn insert(&self, profile: BrowserProfile) -> Result<(), AppError>;
    fn delete(&self, id: ProfileId, mode: DeleteMode) -> Result<(), AppError>;
    fn duplicate(&self, id: ProfileId, new_name: String) -> Result<BrowserProfile, AppError>;
    fn get(&self, id: ProfileId) -> Result<Option<BrowserProfile>, AppError>;
    fn list(&self) -> Result<Vec<BrowserProfile>, AppError>;
}

pub struct DefaultProfileService {
    repo: Arc<dyn ProfileRepository>,
    default_base_dir: PathBuf,
}

impl DefaultProfileService {
    pub fn new(repo: Arc<dyn ProfileRepository>, default_base_dir: PathBuf) -> Self {
        Self {
            repo,
            default_base_dir,
        }
    }

    fn generate_seed(&self) -> u32 {
        use std::time::{SystemTime, UNIX_EPOCH};
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        (duration.as_nanos() & 0xFFFF_FFFF) as u32
    }
}

impl ProfileService for DefaultProfileService {
    fn create(&self, draft: NewProfile) -> Result<BrowserProfile, AppError> {
        let id = ProfileId::new();
        let user_data_dir = draft
            .user_data_dir
            .unwrap_or_else(|| default_user_data_dir(&self.default_base_dir, id));

        let seed = self.generate_seed();
        let fingerprint = draft
            .fingerprint
            .unwrap_or_else(|| FingerprintProfile::new_random(seed));

        let profile = BrowserProfile {
            id,
            name: draft.name,
            core_id: draft.core_id,
            user_data_dir,
            fingerprint,
            proxy_id: draft.proxy_id,
            window: draft.window.unwrap_or_default(),
            start_target: draft.start_target.unwrap_or_default(),
        };

        validate_profile(&profile)?;
        self.repo.insert(&profile)?;
        Ok(profile)
    }

    fn update(&self, profile: BrowserProfile) -> Result<(), AppError> {
        validate_profile(&profile)?;
        self.repo.update(&profile)?;
        Ok(())
    }

    fn insert(&self, profile: BrowserProfile) -> Result<(), AppError> {
        validate_profile(&profile)?;
        self.repo.insert(&profile)?;
        Ok(())
    }

    fn delete(&self, id: ProfileId, mode: DeleteMode) -> Result<(), AppError> {
        if let Some(profile) = self.repo.get(id)? {
            self.repo.delete(id)?;
            if mode == DeleteMode::RemoveUserData
                && profile.user_data_dir.exists()
                && let Err(e) = std::fs::remove_dir_all(&profile.user_data_dir)
            {
                tracing::warn!(
                    "failed to delete user data dir {}: {e}",
                    profile.user_data_dir.display()
                );
            }
        }
        Ok(())
    }

    fn duplicate(&self, id: ProfileId, new_name: String) -> Result<BrowserProfile, AppError> {
        let existing = self
            .repo
            .get(id)?
            .ok_or_else(|| AppError::NotFound(format!("profile {id} not found")))?;

        let new_id = ProfileId::new();
        let new_user_data_dir = default_user_data_dir(&self.default_base_dir, new_id);
        let new_seed = self.generate_seed();

        let duplicated = existing.duplicate(new_id, new_name, new_user_data_dir, new_seed);
        validate_profile(&duplicated)?;
        self.repo.insert(&duplicated)?;
        Ok(duplicated)
    }

    fn get(&self, id: ProfileId) -> Result<Option<BrowserProfile>, AppError> {
        Ok(self.repo.get(id)?)
    }

    fn list(&self) -> Result<Vec<BrowserProfile>, AppError> {
        Ok(self.repo.list()?)
    }
}
