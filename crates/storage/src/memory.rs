use crate::error::StorageError;
use crate::traits::{ConfigurationRepository, CoreRepository, ProfileRepository, ProxyRepository};
use domain::{BrowserCore, BrowserProfile, CoreId, ProfileId, ProxyId, ProxyProfile};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Default)]
pub struct MemProfileRepository {
    profiles: RwLock<HashMap<ProfileId, BrowserProfile>>,
}

impl MemProfileRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProfileRepository for MemProfileRepository {
    fn get(&self, id: ProfileId) -> Result<Option<BrowserProfile>, StorageError> {
        let guard = self
            .profiles
            .read()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        Ok(guard.get(&id).cloned())
    }

    fn list(&self) -> Result<Vec<BrowserProfile>, StorageError> {
        let guard = self
            .profiles
            .read()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        Ok(guard.values().cloned().collect())
    }

    fn insert(&self, profile: &BrowserProfile) -> Result<(), StorageError> {
        let mut guard = self
            .profiles
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        if guard.contains_key(&profile.id) {
            return Err(StorageError::Conflict(format!(
                "profile with id {} already exists",
                profile.id
            )));
        }
        guard.insert(profile.id, profile.clone());
        Ok(())
    }

    fn update(&self, profile: &BrowserProfile) -> Result<(), StorageError> {
        let mut guard = self
            .profiles
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        if !guard.contains_key(&profile.id) {
            return Err(StorageError::NotFound(format!(
                "profile with id {} not found",
                profile.id
            )));
        }
        guard.insert(profile.id, profile.clone());
        Ok(())
    }

    fn delete(&self, id: ProfileId) -> Result<(), StorageError> {
        let mut guard = self
            .profiles
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        guard.remove(&id);
        Ok(())
    }
}

#[derive(Default)]
pub struct MemProxyRepository {
    proxies: RwLock<HashMap<ProxyId, ProxyProfile>>,
}

impl MemProxyRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProxyRepository for MemProxyRepository {
    fn get(&self, id: ProxyId) -> Result<Option<ProxyProfile>, StorageError> {
        let guard = self
            .proxies
            .read()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        Ok(guard.get(&id).cloned())
    }

    fn list(&self) -> Result<Vec<ProxyProfile>, StorageError> {
        let guard = self
            .proxies
            .read()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        Ok(guard.values().cloned().collect())
    }

    fn save(&self, proxy: &ProxyProfile) -> Result<(), StorageError> {
        let mut guard = self
            .proxies
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        guard.insert(proxy.id, proxy.clone());
        Ok(())
    }

    fn delete(&self, id: ProxyId) -> Result<(), StorageError> {
        let mut guard = self
            .proxies
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        guard.remove(&id);
        Ok(())
    }
}

#[derive(Default)]
pub struct MemCoreRepository {
    cores: RwLock<HashMap<CoreId, BrowserCore>>,
}

impl MemCoreRepository {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CoreRepository for MemCoreRepository {
    fn get(&self, id: CoreId) -> Result<Option<BrowserCore>, StorageError> {
        let guard = self
            .cores
            .read()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        Ok(guard.get(&id).cloned())
    }

    fn list(&self) -> Result<Vec<BrowserCore>, StorageError> {
        let guard = self
            .cores
            .read()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        Ok(guard.values().cloned().collect())
    }

    fn save(&self, core: &BrowserCore) -> Result<(), StorageError> {
        let mut guard = self
            .cores
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        guard.insert(core.id, core.clone());
        Ok(())
    }

    fn delete(&self, id: CoreId) -> Result<(), StorageError> {
        let mut guard = self
            .cores
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        guard.remove(&id);
        Ok(())
    }
}

impl MemProfileRepository {
    /// Every profile, in one step, with no refusal of one that is already here.
    ///
    /// The half of [`ConfigurationRepository::replace`] that belongs to this kind,
    /// and the reason it is not `delete` then `insert` per record: a replacement
    /// does not care whether an identifier was already present, because by the
    /// time it runs the old configuration is gone.
    pub fn replace_all(&self, profiles: &[BrowserProfile]) -> Result<(), StorageError> {
        let mut guard = self
            .profiles
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        guard.clear();
        for profile in profiles {
            guard.insert(profile.id, profile.clone());
        }
        Ok(())
    }
}

impl MemProxyRepository {
    /// Every proxy, in one step. See [`MemProfileRepository::replace_all`].
    pub fn replace_all(&self, proxies: &[ProxyProfile]) -> Result<(), StorageError> {
        let mut guard = self
            .proxies
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        guard.clear();
        for proxy in proxies {
            guard.insert(proxy.id, proxy.clone());
        }
        Ok(())
    }
}

impl MemCoreRepository {
    /// Every core, in one step. See [`MemProfileRepository::replace_all`].
    pub fn replace_all(&self, cores: &[BrowserCore]) -> Result<(), StorageError> {
        let mut guard = self
            .cores
            .write()
            .map_err(|e| StorageError::Other(e.to_string()))?;
        guard.clear();
        for core in cores {
            guard.insert(core.id, core.clone());
        }
        Ok(())
    }
}

/// The three in-memory repositories, addressed as one configuration.
///
/// What a test that exercises a replacement uses instead of a database. There is
/// no transaction to enlist in, so the rollback is written out: what was there
/// before is read first and put back if any of the three writes fails. The only
/// failure this backend has is a poisoned lock, which is to say the rollback is
/// not a thing a passing test will ever see - but the shape of the operation is
/// the same one the SQLite backend gives, and the caller cannot tell them apart.
pub struct MemConfiguration {
    pub cores: Arc<MemCoreRepository>,
    pub proxies: Arc<MemProxyRepository>,
    pub profiles: Arc<MemProfileRepository>,
}

impl MemConfiguration {
    pub fn new(
        cores: Arc<MemCoreRepository>,
        proxies: Arc<MemProxyRepository>,
        profiles: Arc<MemProfileRepository>,
    ) -> Self {
        Self {
            cores,
            proxies,
            profiles,
        }
    }
}

impl ConfigurationRepository for MemConfiguration {
    fn replace(
        &self,
        cores: &[BrowserCore],
        proxies: &[ProxyProfile],
        profiles: &[BrowserProfile],
    ) -> Result<(), StorageError> {
        let before = (
            CoreRepository::list(self.cores.as_ref())?,
            ProxyRepository::list(self.proxies.as_ref())?,
            ProfileRepository::list(self.profiles.as_ref())?,
        );

        let outcome = self
            .cores
            .replace_all(cores)
            .and_then(|()| self.proxies.replace_all(proxies))
            .and_then(|()| self.profiles.replace_all(profiles));

        if let Err(error) = outcome {
            let _ = self.cores.replace_all(&before.0);
            let _ = self.proxies.replace_all(&before.1);
            let _ = self.profiles.replace_all(&before.2);
            return Err(error);
        }

        Ok(())
    }
}
