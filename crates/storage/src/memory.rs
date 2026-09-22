use crate::error::StorageError;
use crate::traits::{ConfigurationRepository, CoreRepository, ProfileRepository, ProxyRepository};
use domain::{BrowserCore, BrowserProfile, CoreId, ProfileId, ProxyId, ProxyProfile};
use std::collections::HashMap;
use std::sync::{Arc, RwLock, Weak};

/// The other two repositories, when this one is part of a configuration.
///
/// Weak, not strong: the three point at each other, and a cycle of strong
/// references would keep every in-memory storage alive for the life of the
/// process. A reference that cannot be upgraded means the storage is gone, which
/// is the same answer as "there is nothing left to check against".
#[derive(Default)]
struct Wired {
    cores: Option<Weak<MemCoreRepository>>,
    proxies: Option<Weak<MemProxyRepository>>,
}

#[derive(Default)]
pub struct MemProfileRepository {
    profiles: RwLock<HashMap<ProfileId, BrowserProfile>>,
    wired: RwLock<Wired>,
}

impl MemProfileRepository {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes this repository refuse a profile whose core or proxy is not stored.
    ///
    /// The memory backend has no foreign keys of its own, so the questions SQLite
    /// answers inside the insert are answered here instead. Without this a profile
    /// could name a core that does not exist, and every application test that runs
    /// against this backend would be proving nothing about the rule the database
    /// enforces.
    pub fn check_references(
        &self,
        cores: &Arc<MemCoreRepository>,
        proxies: &Arc<MemProxyRepository>,
    ) {
        let mut wired = self
            .wired
            .write()
            .unwrap_or_else(|error| error.into_inner());
        wired.cores = Some(Arc::downgrade(cores));
        wired.proxies = Some(Arc::downgrade(proxies));
    }

    /// Which of a profile's references is not stored, named, or `None` when there
    /// is nothing to check against.
    fn missing_reference(&self, profile: &BrowserProfile) -> Option<String> {
        let wired = self.wired.read().ok()?;
        if let Some(cores) = wired.cores.as_ref().and_then(Weak::upgrade)
            && cores.get(profile.core_id).ok()?.is_none()
        {
            return Some(format!(
                "profile {} names core {} which is not stored",
                profile.id, profile.core_id
            ));
        }
        if let Some(proxy_id) = profile.proxy_id
            && let Some(proxies) = wired.proxies.as_ref().and_then(Weak::upgrade)
            && proxies.get(proxy_id).ok()?.is_none()
        {
            return Some(format!(
                "profile {} names proxy {proxy_id} which is not stored",
                profile.id
            ));
        }
        None
    }

    /// Refuses a profile that names something which is not stored.
    fn check(&self, profile: &BrowserProfile) -> Result<(), StorageError> {
        match self.missing_reference(profile) {
            Some(reason) => Err(StorageError::Dangling(reason)),
            None => Ok(()),
        }
    }

    /// The name of the profile that still points at this core, if one does.
    fn named_by_core(&self, id: CoreId) -> Option<String> {
        self.used_by(|profile| profile.core_id == id)
    }

    /// The name of the profile that still points at this proxy, if one does.
    fn named_by_proxy(&self, id: ProxyId) -> Option<String> {
        self.used_by(|profile| profile.proxy_id == Some(id))
    }

    fn used_by(&self, matches: impl Fn(&BrowserProfile) -> bool) -> Option<String> {
        let guard = self.profiles.read().ok()?;
        guard
            .values()
            .find(|profile| matches(profile))
            .map(|profile| profile.name.clone())
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
        // Before the lock: the check reads the other two repositories, and taking
        // this one first would order the locks the other way round from a delete.
        self.check(profile)?;
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
        // An update can change which core or proxy a profile names, so it is the
        // same rule as an insert.
        self.check(profile)?;
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
    /// The profiles that may point at a proxy, when this repository is part of a
    /// configuration. See [`MemProfileRepository::check_references`].
    used_by: RwLock<Option<Weak<MemProfileRepository>>>,
}

impl MemProxyRepository {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes deleting a proxy a profile still uses fail, the way the database's
    /// foreign key makes it fail.
    pub fn check_usage(&self, profiles: &Arc<MemProfileRepository>) {
        let mut used_by = self
            .used_by
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *used_by = Some(Arc::downgrade(profiles));
    }

    /// Why the delete is refused, naming what still uses it.
    fn in_use(&self, id: ProxyId) -> Option<String> {
        let used_by = self.used_by.read().ok()?;
        let profiles = used_by.as_ref()?.upgrade()?;
        let user = profiles.named_by_proxy(id)?;
        Some(format!("proxy {id} is still used by {user}"))
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
        // Asked before the lock is taken: the answer comes from the profiles, and
        // taking this repository's lock first would order the two the other way
        // round from an insert.
        if let Some(reason) = self.in_use(id) {
            return Err(StorageError::Conflict(reason));
        }
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
    /// The profiles that may point at a core, when this repository is part of a
    /// configuration.
    used_by: RwLock<Option<Weak<MemProfileRepository>>>,
}

impl MemCoreRepository {
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes deleting a core a profile still uses fail, exactly as a proxy does.
    pub fn check_usage(&self, profiles: &Arc<MemProfileRepository>) {
        let mut used_by = self
            .used_by
            .write()
            .unwrap_or_else(|error| error.into_inner());
        *used_by = Some(Arc::downgrade(profiles));
    }

    /// Why the delete is refused, naming what still uses it.
    fn in_use(&self, id: CoreId) -> Option<String> {
        let used_by = self.used_by.read().ok()?;
        let profiles = used_by.as_ref()?.upgrade()?;
        let user = profiles.named_by_core(id)?;
        Some(format!("core {id} is still used by {user}"))
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
        if let Some(reason) = self.in_use(id) {
            return Err(StorageError::Conflict(reason));
        }
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
        // Checked first, and against the cores and proxies already written by the
        // same replacement: the whole batch is what a database would enforce its
        // foreign keys against, and a profile naming a core the batch does not
        // hold has to fail here too or the two backends disagree about what a
        // replacement may store.
        for profile in profiles {
            self.check(profile)?;
        }
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
        // The three are one configuration, so each has to be able to see the
        // others: this backend has no foreign keys of its own, and the questions
        // SQLite answers for itself are asked here instead.
        profiles.check_references(&cores, &proxies);
        cores.check_usage(&profiles);
        proxies.check_usage(&profiles);
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
