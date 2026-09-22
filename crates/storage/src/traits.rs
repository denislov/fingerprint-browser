use crate::error::StorageError;
use domain::{BrowserCore, BrowserProfile, CoreId, ProfileId, ProxyId, ProxyProfile};

pub trait ProfileRepository: Send + Sync {
    fn get(&self, id: ProfileId) -> Result<Option<BrowserProfile>, StorageError>;
    fn list(&self) -> Result<Vec<BrowserProfile>, StorageError>;
    fn insert(&self, profile: &BrowserProfile) -> Result<(), StorageError>;
    fn update(&self, profile: &BrowserProfile) -> Result<(), StorageError>;
    fn delete(&self, id: ProfileId) -> Result<(), StorageError>;
}

pub trait ProxyRepository: Send + Sync {
    fn get(&self, id: ProxyId) -> Result<Option<ProxyProfile>, StorageError>;
    fn list(&self) -> Result<Vec<ProxyProfile>, StorageError>;
    fn save(&self, proxy: &ProxyProfile) -> Result<(), StorageError>;
    fn delete(&self, id: ProxyId) -> Result<(), StorageError>;
}

pub trait CoreRepository: Send + Sync {
    fn get(&self, id: CoreId) -> Result<Option<BrowserCore>, StorageError>;
    fn list(&self) -> Result<Vec<BrowserCore>, StorageError>;
    fn save(&self, core: &BrowserCore) -> Result<(), StorageError>;
    fn delete(&self, id: CoreId) -> Result<(), StorageError>;
}

/// The whole configuration, written as one thing.
///
/// The three repositories above share one connection but take it one record at a
/// time, so a caller that deletes everything and then writes something else
/// commits after every step: a crash, a full disk or a refused record in the
/// middle leaves an installation that is neither the old configuration nor the
/// new one. A restore is the one operation whose whole promise is that it does
/// not - "be this file" has no meaningful half - so it is the one operation that
/// gets a transaction of its own instead of a sequence of repository calls.
///
/// `replace` is total: on success every core, proxy and profile is one of these,
/// and on failure nothing has changed.
pub trait ConfigurationRepository: Send + Sync {
    fn replace(
        &self,
        cores: &[BrowserCore],
        proxies: &[ProxyProfile],
        profiles: &[BrowserProfile],
    ) -> Result<(), StorageError>;
}
