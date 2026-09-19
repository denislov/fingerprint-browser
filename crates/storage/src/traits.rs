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
