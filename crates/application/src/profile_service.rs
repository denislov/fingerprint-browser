use crate::error::AppError;
use domain::{
    BrowserProfile, CoreId, FingerprintProfile, ProfileId, ProxyId, StartTarget, WindowProfile,
    validate_profile,
};
use std::path::PathBuf;
use std::sync::Arc;
use storage::ProfileRepository;

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
    /// Removes the record, and only the record.
    ///
    /// There is deliberately no mode that removes the browser data as well. A
    /// profile's directory is not always a directory this program created: an
    /// imported profile keeps the absolute path it was exported with, so a
    /// recursive delete here would end at whatever that path names - and it would
    /// end there *after* the record was gone, leaving nothing that describes what
    /// was deleted or why. If deleting browser data is ever wanted, it belongs
    /// behind proof that the directory is the one this installation keeps for that
    /// profile, a check that no browser is running in it, and a report of what
    /// could not be removed - none of which this service can establish on its own.
    fn delete(&self, id: ProfileId) -> Result<(), AppError>;
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

    fn delete(&self, id: ProfileId) -> Result<(), AppError> {
        // Read first, so a missing profile is not silently a successful delete:
        // the caller wants to know that the identifier named nothing.
        if self.repo.get(id)?.is_some() {
            self.repo.delete(id)?;
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
