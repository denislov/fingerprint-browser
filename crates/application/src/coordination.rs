//! Nonblocking installation leases shared by services and background workers.
use crate::{AppError, CoreService, NewProfile, NewProxy, ProfileService, ProxyService};
use domain::{BrowserCore, BrowserProfile, CoreId, ProfileId, ProxyId, ProxyProfile};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Debug, Default)]
pub struct Installation {
    state: Mutex<(usize, bool)>,
}

impl Installation {
    pub fn shared(self: &Arc<Self>) -> Result<InstallationLease, AppError> {
        self.acquire(false)
    }

    pub fn exclusive(self: &Arc<Self>) -> Result<InstallationLease, AppError> {
        self.acquire(true)
    }

    fn acquire(self: &Arc<Self>, exclusive: bool) -> Result<InstallationLease, AppError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.1 || (exclusive && state.0 != 0) {
            return Err(AppError::Conflict(
                "configuration maintenance is in progress".into(),
            ));
        }
        if exclusive {
            state.1 = true;
        } else {
            state.0 += 1;
        }
        Ok(InstallationLease {
            installation: Arc::clone(self),
            exclusive,
        })
    }
}

/// Owned rather than borrowed so a job can carry its lease onto a worker.
#[derive(Debug)]
#[must_use]
pub struct InstallationLease {
    installation: Arc<Installation>,
    exclusive: bool,
}

impl Drop for InstallationLease {
    fn drop(&mut self) {
        let mut state = self
            .installation
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.exclusive {
            state.1 = false;
        } else {
            state.0 -= 1;
        }
    }
}

/// Service reads remain available to the restore worker. Every mutation holds
/// a shared lease for its entire call, including slow executable probes.
pub struct Coordinated<T: ?Sized> {
    pub service: Arc<T>,
    pub installation: Arc<Installation>,
}

macro_rules! mutation {
    ($name:ident($($arg:ident: $ty:ty),*) -> $result:ty) => {
        fn $name(&self, $($arg: $ty),*) -> Result<$result, AppError> {
            let _lease = self.installation.shared()?;
            self.service.$name($($arg),*)
        }
    };
}

impl ProfileService for Coordinated<dyn ProfileService> {
    mutation!(create(draft: NewProfile) -> BrowserProfile);
    mutation!(update(profile: BrowserProfile) -> ());
    mutation!(insert(profile: BrowserProfile) -> ());
    mutation!(delete(id: ProfileId) -> ());
    mutation!(duplicate(id: ProfileId, name: String) -> BrowserProfile);
    fn get(&self, id: ProfileId) -> Result<Option<BrowserProfile>, AppError> {
        self.service.get(id)
    }
    fn list(&self) -> Result<Vec<BrowserProfile>, AppError> {
        self.service.list()
    }
}

impl ProxyService for Coordinated<dyn ProxyService> {
    mutation!(create(draft: NewProxy) -> ProxyProfile);
    mutation!(update(proxy: ProxyProfile) -> ());
    mutation!(insert(proxy: ProxyProfile) -> ());
    mutation!(delete(id: ProxyId) -> ());
    fn get(&self, id: ProxyId) -> Result<Option<ProxyProfile>, AppError> {
        self.service.get(id)
    }
    fn list(&self) -> Result<Vec<ProxyProfile>, AppError> {
        self.service.list()
    }
    fn usage(&self) -> Result<HashMap<ProxyId, Vec<String>>, AppError> {
        self.service.usage()
    }
}

impl CoreService for Coordinated<dyn CoreService> {
    mutation!(add(name: Option<String>, path: PathBuf) -> BrowserCore);
    mutation!(update(core: BrowserCore) -> BrowserCore);
    mutation!(insert(core: BrowserCore) -> ());
    mutation!(delete(id: CoreId) -> ());
    mutation!(redetect(id: CoreId) -> BrowserCore);
    fn get(&self, id: CoreId) -> Result<Option<BrowserCore>, AppError> {
        self.service.get(id)
    }
    fn list(&self) -> Result<Vec<BrowserCore>, AppError> {
        self.service.list()
    }
    fn usage(&self) -> Result<HashMap<CoreId, Vec<String>>, AppError> {
        self.service.usage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leases_exclude_conflicting_work_and_release_on_unwind() {
        let installation = Arc::new(Installation::default());
        let shared = installation.shared().unwrap();
        assert!(installation.exclusive().is_err());
        drop(shared);
        let guard = installation.exclusive().unwrap();
        assert!(installation.shared().is_err());
        assert!(installation.exclusive().is_err());
        let _ = std::panic::catch_unwind(move || {
            let _guard = guard;
            panic!("worker failed");
        });
        assert!(installation.shared().is_ok());
    }
}
