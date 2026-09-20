//! Proxies the user can assign to a profile.
//!
//! Two rules live here rather than in the UI. A proxy is checked against the
//! same domain rules storage enforces and against what the runtime can actually
//! build, so an entry that could never work is refused when it is created
//! instead of failing later at launch. And a proxy that profiles still point at
//! cannot be deleted: the storage schema would null the assignment out and
//! quietly send that traffic direct, which is the one outcome a proxy must
//! never have.

use crate::error::AppError;
use domain::{ProxyId, ProxyOutbound, ProxyProfile, validate_proxy};
use std::collections::HashMap;
use std::sync::Arc;
use storage::{ProfileRepository, ProxyRepository};

#[derive(Debug, Clone)]
pub struct NewProxy {
    pub name: String,
    pub outbound: ProxyOutbound,
}

pub trait ProxyService: Send + Sync {
    fn create(&self, draft: NewProxy) -> Result<ProxyProfile, AppError>;
    fn update(&self, proxy: ProxyProfile) -> Result<(), AppError>;
    fn delete(&self, id: ProxyId) -> Result<(), AppError>;
    fn get(&self, id: ProxyId) -> Result<Option<ProxyProfile>, AppError>;
    fn list(&self) -> Result<Vec<ProxyProfile>, AppError>;
    /// The names of the profiles assigned to each proxy, keyed by proxy.
    fn usage(&self) -> Result<HashMap<ProxyId, Vec<String>>, AppError>;
}

pub struct DefaultProxyService {
    proxies: Arc<dyn ProxyRepository>,
    profiles: Arc<dyn ProfileRepository>,
}

impl DefaultProxyService {
    pub fn new(proxies: Arc<dyn ProxyRepository>, profiles: Arc<dyn ProfileRepository>) -> Self {
        Self { proxies, profiles }
    }

    /// Storage rules, and the domain rules storage also enforces.
    ///
    /// There used to be a second question here - does the runtime have a config
    /// builder for this protocol - and the answer is now "for all six". What is
    /// left is what the rules say, and the window is where a protocol without a
    /// form is reached, by pasting a link.
    fn check(&self, proxy: &ProxyProfile) -> Result<(), AppError> {
        validate_proxy(proxy)?;
        Ok(())
    }

    fn users_of(&self, id: ProxyId) -> Result<Vec<String>, AppError> {
        let mut users: Vec<String> = self
            .profiles
            .list()?
            .into_iter()
            .filter(|profile| profile.proxy_id == Some(id))
            .map(|profile| profile.name)
            .collect();
        users.sort();
        Ok(users)
    }
}

impl ProxyService for DefaultProxyService {
    fn create(&self, draft: NewProxy) -> Result<ProxyProfile, AppError> {
        let proxy = ProxyProfile {
            id: ProxyId::new(),
            name: draft.name,
            outbound: draft.outbound,
        };
        self.check(&proxy)?;
        self.proxies.save(&proxy)?;
        Ok(proxy)
    }

    fn update(&self, proxy: ProxyProfile) -> Result<(), AppError> {
        if self.proxies.get(proxy.id)?.is_none() {
            return Err(AppError::NotFound(format!("proxy {}", proxy.id)));
        }
        self.check(&proxy)?;
        self.proxies.save(&proxy)?;
        Ok(())
    }

    fn delete(&self, id: ProxyId) -> Result<(), AppError> {
        let proxy = self
            .proxies
            .get(id)?
            .ok_or_else(|| AppError::NotFound(format!("proxy {id}")))?;
        let users = self.users_of(id)?;
        if !users.is_empty() {
            return Err(AppError::Conflict(format!(
                "{} is assigned to {}; assign {} to another proxy or to Direct first",
                proxy.name,
                users.join(", "),
                if users.len() == 1 { "it" } else { "them" },
            )));
        }
        self.proxies.delete(id)?;
        Ok(())
    }

    fn get(&self, id: ProxyId) -> Result<Option<ProxyProfile>, AppError> {
        Ok(self.proxies.get(id)?)
    }

    fn list(&self) -> Result<Vec<ProxyProfile>, AppError> {
        let mut proxies = self.proxies.list()?;
        proxies.sort_by_key(|proxy| proxy.name.to_lowercase());
        Ok(proxies)
    }

    fn usage(&self) -> Result<HashMap<ProxyId, Vec<String>>, AppError> {
        let mut usage: HashMap<ProxyId, Vec<String>> = HashMap::new();
        for profile in self.profiles.list()? {
            if let Some(id) = profile.proxy_id {
                usage.entry(id).or_default().push(profile.name);
            }
        }
        for names in usage.values_mut() {
            names.sort();
        }
        Ok(usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        BrowserProfile, CoreId, FingerprintProfile, ProfileId, Socks5Outbound, StartTarget,
        WindowProfile,
    };
    use storage::{MemProfileRepository, MemProxyRepository};

    fn service() -> (
        DefaultProxyService,
        Arc<MemProxyRepository>,
        Arc<MemProfileRepository>,
    ) {
        let proxies = Arc::new(MemProxyRepository::new());
        let profiles = Arc::new(MemProfileRepository::new());
        let service = DefaultProxyService::new(
            Arc::clone(&proxies) as Arc<dyn ProxyRepository>,
            Arc::clone(&profiles) as Arc<dyn ProfileRepository>,
        );
        (service, proxies, profiles)
    }

    fn socks5(host: &str, port: u16) -> ProxyOutbound {
        ProxyOutbound::Socks5(Socks5Outbound {
            host: host.to_string(),
            port,
            username: None,
            password: None,
        })
    }

    fn assign(profiles: &MemProfileRepository, name: &str, proxy: Option<ProxyId>) -> ProfileId {
        let profile = BrowserProfile {
            id: ProfileId::new(),
            name: name.to_string(),
            core_id: CoreId::new(),
            user_data_dir: std::path::PathBuf::from("/tmp/none"),
            fingerprint: FingerprintProfile::new_random(7),
            proxy_id: proxy,
            window: WindowProfile::default(),
            start_target: StartTarget::default(),
        };
        profiles.insert(&profile).expect("insert profile");
        profile.id
    }

    fn draft(name: &str) -> NewProxy {
        NewProxy {
            name: name.to_string(),
            outbound: socks5("10.0.0.1", 1080),
        }
    }

    #[test]
    fn a_created_proxy_can_be_read_back() {
        let (service, _, _) = service();
        let created = service.create(draft("Office")).expect("create");
        assert_eq!(created.name, "Office");
        assert_eq!(service.list().expect("list").len(), 1);
        assert_eq!(
            service.get(created.id).expect("get").expect("exists").id,
            created.id
        );
    }

    #[test]
    fn a_broken_proxy_is_refused_before_it_is_stored() {
        let (service, proxies, _) = service();
        let mut draft = draft("Office");
        draft.outbound = socks5("", 1080);
        let error = service.create(draft).unwrap_err();
        assert!(error.to_string().contains("host"), "{error}");
        assert!(proxies.list().expect("list").is_empty());
    }

    /// A protocol with no form is stored like any other: the window reaches it
    /// by pasting a link, and an imported proxy goes through the same rules a
    /// typed one does.
    #[test]
    fn an_imported_protocol_is_stored_like_any_other() {
        let (service, proxies, _) = service();
        let imported = domain::parse_proxy_uri(
            "vless://b831381d-6324-4d53-ad4f-8cda48b30811@node.example:443?encryption=none\
             &security=reality&pbk=LOLTG162EtSegCnMAofVY3oKrbrCvH8zOTZEPd1GRQU&sni=front.example",
        )
        .expect("a share link");
        service
            .create(NewProxy {
                name: imported.suggested_name(),
                outbound: imported.outbound,
            })
            .expect("an imported proxy is storable");

        let stored = proxies.list().expect("list");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "vless node.example");
        assert_eq!(stored[0].outbound.kind(), "vless");
    }

    #[test]
    fn updating_a_proxy_that_is_gone_is_not_found() {
        let (service, _, _) = service();
        let proxy = ProxyProfile {
            id: ProxyId::new(),
            name: "Ghost".to_string(),
            outbound: socks5("10.0.0.1", 1080),
        };
        assert!(matches!(service.update(proxy), Err(AppError::NotFound(_))));
    }

    #[test]
    fn an_edit_is_stored_and_re_checked() {
        let (service, _, _) = service();
        let mut proxy = service.create(draft("Office")).expect("create");
        proxy.name = "Office (new)".to_string();
        proxy.outbound = socks5("10.0.0.2", 1081);
        service.update(proxy.clone()).expect("update");
        assert_eq!(service.get(proxy.id).expect("get").expect("exists"), proxy);

        let mut broken = proxy.clone();
        broken.outbound = socks5("10.0.0.2", 0);
        assert!(service.update(broken).is_err());
        assert_eq!(
            service.get(proxy.id).expect("get").expect("exists"),
            proxy,
            "a refused edit must leave the stored proxy alone"
        );
    }

    /// Deleting would null the assignment out and send that traffic direct.
    #[test]
    fn a_proxy_still_assigned_to_a_profile_cannot_be_deleted() {
        let (service, _, profiles) = service();
        let proxy = service.create(draft("Office")).expect("create");
        assign(&profiles, "Profile 1", Some(proxy.id));

        let error = service.delete(proxy.id).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Office"), "{message}");
        assert!(message.contains("Profile 1"), "{message}");
        assert_eq!(service.list().expect("list").len(), 1);
    }

    #[test]
    fn an_unassigned_proxy_is_deleted() {
        let (service, _, profiles) = service();
        let proxy = service.create(draft("Office")).expect("create");
        let other = service.create(draft("Home")).expect("create");
        assign(&profiles, "Profile 1", Some(other.id));

        service.delete(proxy.id).expect("delete");
        let remaining = service.list().expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, other.id);
    }

    #[test]
    fn usage_names_the_profiles_per_proxy() {
        let (service, _, profiles) = service();
        let proxy = service.create(draft("Office")).expect("create");
        let unused = service.create(draft("Home")).expect("create");
        assign(&profiles, "Profile 2", Some(proxy.id));
        assign(&profiles, "Profile 1", Some(proxy.id));
        assign(&profiles, "Unproxied", None);

        let usage = service.usage().expect("usage");
        assert_eq!(
            usage.get(&proxy.id).cloned().unwrap_or_default(),
            vec!["Profile 1".to_string(), "Profile 2".to_string()]
        );
        assert!(!usage.contains_key(&unused.id));
    }

    #[test]
    fn deleting_a_missing_proxy_is_not_found() {
        let (service, _, _) = service();
        assert!(matches!(
            service.delete(ProxyId::new()),
            Err(AppError::NotFound(_))
        ));
    }
}
