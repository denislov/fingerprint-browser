use crate::error::StorageError;
use crate::traits::{CoreRepository, ProfileRepository, ProxyRepository};
use domain::{BrowserCore, BrowserProfile, CoreId, ProfileId, ProxyId, ProxyProfile};
use std::collections::HashMap;
use std::sync::RwLock;

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
