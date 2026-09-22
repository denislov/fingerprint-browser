use crate::error::AppError;
use domain::ProfileId;
use runtime::{RuntimeFacade, RuntimeSnapshot, StartParams};
use std::sync::Arc;
use storage::{CoreRepository, ProfileRepository, ProxyRepository};

pub struct RuntimeService {
    profile_repo: Arc<dyn ProfileRepository>,
    core_repo: Arc<dyn CoreRepository>,
    proxy_repo: Arc<dyn ProxyRepository>,
    runtime: Arc<dyn RuntimeFacade>,
}

impl RuntimeService {
    pub fn new(
        profile_repo: Arc<dyn ProfileRepository>,
        core_repo: Arc<dyn CoreRepository>,
        proxy_repo: Arc<dyn ProxyRepository>,
        runtime: Arc<dyn RuntimeFacade>,
    ) -> Self {
        Self {
            profile_repo,
            core_repo,
            proxy_repo,
            runtime,
        }
    }

    /// Ends this run with every running session left running.
    ///
    /// The one command that is not about a profile: "leave them all" is a
    /// decision about this program, and the runtime is what knows which sessions
    /// there are.
    pub fn release_all(&self) -> Result<(), AppError> {
        self.runtime.release_all().map_err(AppError::Runtime)
    }

    /// Every record a launch needs, or the name of the one that is missing.
    ///
    /// A launch needs three records and refuses to guess at any of them: the
    /// profile, the core it runs on, and the proxy it egresses through when it has
    /// one. A start and a restart need exactly the same three, so they ask here -
    /// two copies of this were two places for the rule to drift, and the missing
    /// record is only ever discovered by the runtime if one of them forgot it.
    ///
    /// Its own function also because it is the part worth testing: no runtime is
    /// involved in resolving a reference, and the failure says which record was
    /// missing rather than that something was.
    fn launch_params(&self, profile_id: ProfileId) -> Result<StartParams, AppError> {
        let profile = self
            .profile_repo
            .get(profile_id)?
            .ok_or_else(|| AppError::NotFound(format!("profile {profile_id} not found")))?;

        let core = self
            .core_repo
            .get(profile.core_id)?
            .ok_or_else(|| AppError::NotFound(format!("core {} not found", profile.core_id)))?;

        let proxy = match profile.proxy_id {
            Some(pid) => Some(
                self.proxy_repo
                    .get(pid)?
                    .ok_or_else(|| AppError::NotFound(format!("proxy {pid} not found")))?,
            ),
            None => None,
        };

        Ok(StartParams {
            request_id: StartParams::next_request_id(),
            profile,
            core,
            proxy,
        })
    }

    pub fn start(&self, profile_id: ProfileId) -> Result<u64, AppError> {
        let params = self.launch_params(profile_id)?;
        let request = params.request_id;
        self.runtime.start(params)?;
        Ok(request)
    }

    pub fn stop(&self, profile_id: ProfileId) -> Result<(), AppError> {
        self.runtime.stop(profile_id)?;
        Ok(())
    }

    pub fn restart(&self, profile_id: ProfileId) -> Result<u64, AppError> {
        let params = self.launch_params(profile_id)?;
        let request = params.request_id;
        self.runtime.restart(params)?;
        Ok(request)
    }

    pub fn snapshot(&self, profile_id: ProfileId) -> Option<RuntimeSnapshot> {
        self.runtime.snapshot(profile_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        BrowserCore, BrowserProfile, CoreId, FingerprintProfile, ProfileId, ProxyId, ProxyOutbound,
        ProxyProfile, Socks5Outbound, StartTarget, WindowProfile,
    };
    use std::path::PathBuf;
    use std::sync::Mutex;
    use storage::{MemConfiguration, MemCoreRepository, MemProfileRepository, MemProxyRepository};

    /// What the runtime was asked to do, and with which records.
    struct Recorder {
        sent: Mutex<Vec<(&'static str, StartParams)>>,
    }

    impl RuntimeFacade for Recorder {
        fn start(&self, params: StartParams) -> Result<(), runtime::RuntimeCommandError> {
            self.sent.lock().expect("recorded").push(("start", params));
            Ok(())
        }

        fn stop(&self, profile_id: ProfileId) -> Result<(), runtime::RuntimeCommandError> {
            let mut sent = self.sent.lock().expect("recorded");
            sent.push((
                "stop",
                StartParams {
                    request_id: 0,
                    profile: profile(profile_id, CoreId::new()),
                    core: core(CoreId::new()),
                    proxy: None,
                },
            ));
            Ok(())
        }

        fn restart(&self, params: StartParams) -> Result<(), runtime::RuntimeCommandError> {
            self.sent
                .lock()
                .expect("recorded")
                .push(("restart", params));
            Ok(())
        }

        fn release_all(&self) -> Result<(), runtime::RuntimeCommandError> {
            Ok(())
        }

        fn snapshot(&self, _: ProfileId) -> Option<RuntimeSnapshot> {
            None
        }
    }

    fn core(id: CoreId) -> BrowserCore {
        BrowserCore {
            id,
            name: "Chromium".to_string(),
            executable: PathBuf::from("/opt/chromium/chrome"),
            version: "148.0.0.0".to_string(),
            major: 148,
        }
    }

    fn proxy(id: ProxyId) -> ProxyProfile {
        ProxyProfile {
            id,
            name: "Zurich exit".to_string(),
            outbound: ProxyOutbound::Socks5(Socks5Outbound {
                host: "127.0.0.1".to_string(),
                port: 1080,
                username: None,
                password: None,
            }),
        }
    }

    fn profile(id: ProfileId, core_id: CoreId) -> BrowserProfile {
        BrowserProfile {
            id,
            name: "Work laptop".to_string(),
            core_id,
            user_data_dir: PathBuf::from("/tmp/profile"),
            fingerprint: FingerprintProfile::new_random(7),
            proxy_id: None,
            window: WindowProfile::new(800, 600),
            start_target: StartTarget::Blank,
        }
    }

    struct Fixture {
        service: RuntimeService,
        cores: Arc<MemCoreRepository>,
        proxies: Arc<MemProxyRepository>,
        profiles: Arc<MemProfileRepository>,
        sent: Arc<Recorder>,
    }

    type Repos = (
        Arc<MemCoreRepository>,
        Arc<MemProxyRepository>,
        Arc<MemProfileRepository>,
    );

    fn repos() -> Repos {
        (
            Arc::new(MemCoreRepository::new()),
            Arc::new(MemProxyRepository::new()),
            Arc::new(MemProfileRepository::new()),
        )
    }

    /// Wires the three the way the program does, so what these tests insert is
    /// held to the rules the database enforces.
    fn wire((cores, proxies, profiles): &Repos) {
        let _configuration =
            MemConfiguration::new(Arc::clone(cores), Arc::clone(proxies), Arc::clone(profiles));
    }

    /// The service, over repositories and a runtime that records what it is asked.
    fn service_over((cores, proxies, profiles): Repos) -> Fixture {
        let sent = Arc::new(Recorder {
            sent: Mutex::new(Vec::new()),
        });
        Fixture {
            service: RuntimeService::new(
                Arc::clone(&profiles) as Arc<dyn ProfileRepository>,
                Arc::clone(&cores) as Arc<dyn CoreRepository>,
                Arc::clone(&proxies) as Arc<dyn ProxyRepository>,
                Arc::clone(&sent) as Arc<dyn RuntimeFacade>,
            ),
            cores,
            proxies,
            profiles,
            sent,
        }
    }

    fn fixture() -> Fixture {
        let repos = repos();
        wire(&repos);
        service_over(repos)
    }

    /// A configuration holding a profile that names a record which is not stored.
    ///
    /// The profile is written before the repositories are wired to each other,
    /// because that is the only way such a row exists: a database written before
    /// the constraint did, or one edited outside the program. The service still has
    /// to refuse to launch it rather than hand the runtime a profile it cannot
    /// start.
    fn dangling(
        profile: BrowserProfile,
        core: Option<BrowserCore>,
        proxy: Option<ProxyProfile>,
    ) -> Fixture {
        let repos = repos();
        if let Some(core) = core {
            repos.0.save(&core).expect("save the core");
        }
        if let Some(proxy) = proxy {
            repos.1.save(&proxy).expect("save the proxy");
        }
        repos
            .2
            .insert(&profile)
            .expect("a profile written before the checks existed");
        wire(&repos);
        service_over(repos)
    }

    /// A start and a restart carry the same three records: the profile, the core
    /// it runs on, and its proxy when it has one.
    #[test]
    fn both_commands_carry_the_same_resolved_records() {
        let f = fixture();
        let core = core(CoreId::new());
        f.cores.save(&core).expect("save the core");
        let proxy = proxy(ProxyId::new());
        f.proxies.save(&proxy).expect("save the proxy");
        let mut work = profile(ProfileId::new(), core.id);
        work.proxy_id = Some(proxy.id);
        f.profiles.insert(&work).expect("save the profile");

        f.service.start(work.id).expect("start");
        f.service.restart(work.id).expect("restart");

        let sent = f.sent.sent.lock().expect("recorded");
        assert_eq!(
            sent.iter().map(|(what, _)| *what).collect::<Vec<_>>(),
            vec!["start", "restart"]
        );
        for (what, params) in sent.iter() {
            assert_eq!(params.profile.id, work.id, "{what} carries the profile");
            assert_eq!(params.core.id, core.id, "{what} carries the core");
            assert_eq!(
                params.proxy.as_ref().map(|proxy| proxy.id),
                Some(proxy.id),
                "{what} carries the proxy"
            );
        }
    }

    /// Nothing is sent when a record the launch needs is missing, and the refusal
    /// names the one that was: "not found" about the wrong thing sends the reader
    /// to the wrong page.
    #[test]
    fn a_missing_record_refuses_the_command_before_the_runtime_hears_about_it() {
        // No profile at all.
        let f = fixture();
        let missing = ProfileId::new();
        let error = f.service.start(missing).expect_err("no such profile");
        assert!(error.to_string().contains(&missing.to_string()), "{error}");
        assert!(f.sent.sent.lock().expect("recorded").is_empty());

        // A profile whose core is not stored.
        let absent_core = CoreId::new();
        let orphan = profile(ProfileId::new(), absent_core);
        let f = dangling(orphan.clone(), None, None);
        let error = f.service.restart(orphan.id).expect_err("no such core");
        assert!(
            error.to_string().contains(&absent_core.to_string()),
            "{error}"
        );
        assert!(f.sent.sent.lock().expect("recorded").is_empty());

        // A profile whose proxy is not stored.
        let absent_proxy = ProxyId::new();
        let stored_core = core(CoreId::new());
        let mut orphan = profile(ProfileId::new(), stored_core.id);
        orphan.proxy_id = Some(absent_proxy);
        let f = dangling(orphan.clone(), Some(stored_core), None);
        let error = f.service.start(orphan.id).expect_err("no such proxy");
        assert!(
            error.to_string().contains(&absent_proxy.to_string()),
            "{error}"
        );
        assert!(f.sent.sent.lock().expect("recorded").is_empty());
    }

    /// A profile with no proxy launches without one, which is not a missing
    /// record: the field is optional upstream and has to stay optional here.
    #[test]
    fn a_profile_with_no_proxy_launches_without_one() {
        let f = fixture();
        let core = core(CoreId::new());
        f.cores.save(&core).expect("save the core");
        let work = profile(ProfileId::new(), core.id);
        f.profiles.insert(&work).expect("save the profile");

        f.service.start(work.id).expect("start");

        let sent = f.sent.sent.lock().expect("recorded");
        assert_eq!(sent.len(), 1);
        assert!(sent[0].1.proxy.is_none());
    }

    /// A stop needs no records at all: it names a session, not a launch.
    #[test]
    fn stopping_does_not_read_the_configuration() {
        let f = fixture();
        let unknown = ProfileId::new();
        f.service.stop(unknown).expect("a stop is not a lookup");
        assert_eq!(f.sent.sent.lock().expect("recorded").len(), 1);
    }
}
