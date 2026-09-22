use crate::error::StorageError;
use domain::{BrowserCore, BrowserProfile, CoreId, ProfileId, ProxyId, ProxyProfile};

/// Profiles, one record at a time.
///
/// `insert` and `update` are the two halves of a write that knows which it is:
/// one refuses an identifier that is taken, the other refuses one that is not
/// there. A profile's identifier is not a label - it is what its browser data
/// directory is named after - so overwriting one silently would be a way to lose
/// the record of a directory that is still on disk, which is why there is no
/// `save` here.
pub trait ProfileRepository: Send + Sync {
    fn get(&self, id: ProfileId) -> Result<Option<BrowserProfile>, StorageError>;
    fn list(&self) -> Result<Vec<BrowserProfile>, StorageError>;
    fn insert(&self, profile: &BrowserProfile) -> Result<(), StorageError>;
    fn update(&self, profile: &BrowserProfile) -> Result<(), StorageError>;
    fn delete(&self, id: ProfileId) -> Result<(), StorageError>;
}

/// Proxies, one record at a time.
///
/// `save` is an upsert: it writes the record under the identifier it carries,
/// replacing whatever was stored under that identifier. That is what an import
/// needs - the file names the identifier, and the identifier is what a profile's
/// assignment travels by - and it is not what an edit needs, which goes through
/// the application service, where the rules are.
pub trait ProxyRepository: Send + Sync {
    fn get(&self, id: ProxyId) -> Result<Option<ProxyProfile>, StorageError>;
    fn list(&self) -> Result<Vec<ProxyProfile>, StorageError>;
    fn save(&self, proxy: &ProxyProfile) -> Result<(), StorageError>;
    fn delete(&self, id: ProxyId) -> Result<(), StorageError>;
}

/// Cores, one record at a time. See [`ProxyRepository::save`] for the upsert.
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
