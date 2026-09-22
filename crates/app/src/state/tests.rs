use crate::text::en;

use super::*;
use crate::state::testing::{CoreBinary, FakeRuntime, core, core_service};
use application::{DefaultProfileService, DefaultProxyService};
use domain::CoreId;
use runtime::FaultClass;
use std::path::PathBuf;
use storage::{
    CoreRepository as _, MemCoreRepository, MemProfileRepository, MemProxyRepository,
    ProfileRepository as _, ProxyRepository as _,
};

struct Fixture {
    state: AppState,
    runtime: Arc<FakeRuntime>,
    cores: Arc<MemCoreRepository>,
    profiles: Arc<MemProfileRepository>,
    proxies: Arc<MemProxyRepository>,
}

fn fixture() -> Fixture {
    fixture_with_config(
        &std::env::temp_dir()
            .join("fp-app-settings-fixture")
            .join("config.json"),
    )
}

/// The same fixture, with the settings file somewhere the test can read.
fn fixture_with_config(config: &std::path::Path) -> Fixture {
    fixture_full(config, None, None, None)
}

/// The same fixture, with its data directory somewhere the test can name.
///
/// An import re-points a profile's browser data at the local default under
/// the data directory, so a test of that has to know which directory the
/// settings believe in.
fn fixture_with_data_dir(data_dir: &std::path::Path) -> Fixture {
    fixture_full(
        &std::env::temp_dir()
            .join("fp-app-settings-fixture")
            .join("config.json"),
        Some(data_dir),
        None,
        None,
    )
}

/// The same fixture, with an activity log on disk in `log_dir`.
fn fixture_with_log(log_dir: &std::path::Path) -> Fixture {
    let file = crate::log_file::LogFile::open(log_dir).expect("open the log file");
    fixture_full(
        &std::env::temp_dir()
            .join("fp-app-settings-fixture")
            .join("config.json"),
        None,
        Some(file),
        None,
    )
}

fn fixture_full(
    config: &std::path::Path,
    data_dir: Option<&std::path::Path>,
    log_file: Option<crate::log_file::LogFile>,
    log_file_error: Option<String>,
) -> Fixture {
    let profile_repo: Arc<MemProfileRepository> = Arc::new(MemProfileRepository::new());
    let core_repo: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
    let proxy_repo: Arc<MemProxyRepository> = Arc::new(MemProxyRepository::new());
    let runtime = Arc::new(FakeRuntime::new());

    let service = Arc::new(DefaultProfileService::new(
        profile_repo.clone(),
        PathBuf::from("data"),
    ));
    let runtime_service = Arc::new(RuntimeService::new(
        profile_repo.clone(),
        core_repo.clone(),
        proxy_repo.clone(),
        runtime.clone(),
    ));

    let proxy_service: Arc<dyn ProxyService> = Arc::new(DefaultProxyService::new(
        proxy_repo.clone(),
        profile_repo.clone(),
    ));

    let core_service = core_service(core_repo.clone(), profile_repo.clone());
    let (settings, _) = Settings::load(crate::settings::Environment {
        config: Some(config.to_string_lossy().to_string()),
        data_dir: data_dir.map(|dir| dir.to_string_lossy().to_string()),
        ..crate::settings::Environment::default()
    });
    let state = AppState::with_log(
        Services {
            profiles: service,
            runtime: runtime_service,
            cores: core_service,
            proxies: proxy_service,
            configuration: Arc::new(storage::MemConfiguration::new(
                core_repo.clone(),
                proxy_repo.clone(),
                profile_repo.clone(),
            )),
        },
        settings,
        log_file,
        log_file_error,
    );

    Fixture {
        state,
        runtime,
        cores: core_repo,
        profiles: profile_repo,
        proxies: proxy_repo,
    }
}

fn seed_core(fixture: &Fixture) -> CoreId {
    let core = core(CoreId::new());
    fixture.cores.save(&core).expect("save core");
    core.id
}

/// A loaded profile with a name and a seed the test chose, so a filter has
/// something predictable to match on.
fn named_profile(fixture: &mut Fixture, name: &str, seed: u32) -> ProfileId {
    let id = fixture.state.create_profile(name).expect("create");
    let mut profile = fixture.state.profile(id).expect("loaded");
    profile.fingerprint.seed = seed;
    fixture.state.update_profile(profile).expect("save");
    id
}

/// A fixture with a core and three profiles to filter between.
fn filtered_fixture() -> (Fixture, ProfileId, ProfileId, ProfileId) {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let work = named_profile(&mut fixture, "Work laptop", 4242);
    let shopping = named_profile(&mut fixture, "Shopping", 7777);
    let staging = named_profile(&mut fixture, "Staging box", 1234);
    (fixture, work, shopping, staging)
}

fn socks5(host: &str, port: u16) -> ProxyOutbound {
    ProxyOutbound::Socks5(domain::Socks5Outbound {
        host: host.to_string(),
        port,
        username: None,
        password: None,
    })
}

/// A running profile with a debug port, ready to be verified.
fn running_profile(fixture: &mut Fixture) -> ProfileId {
    seed_core(fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_profile("verify me")
        .expect("create profile");
    fixture.runtime.set_state(id, RuntimeState::Running);
    fixture.runtime.set_cdp_port(id, 9333);
    fixture.state.refresh_runtime();
    id
}

/// A running profile whose traffic is meant to leave by a proxy.
fn proxied_profile(fixture: &mut Fixture) -> (ProfileId, ProxyId) {
    let id = running_profile(fixture);
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let mut profile = fixture.state.profile(id).expect("the profile is loaded");
    profile.proxy_id = Some(proxy_id);
    fixture.state.update_profile(profile).expect("assign");
    (id, proxy_id)
}

/// The log line for a verification that found nothing wrong.
fn confirmed_line_in(fixture: &Fixture) -> String {
    fixture
        .state
        .log_entries()
        .iter()
        .map(|entry| entry.message.clone())
        .find(|message| message.starts_with("fingerprint confirmed"))
        .unwrap_or_else(|| {
            panic!(
                "no record of a confirmed reading: {:?}",
                fixture
                    .state
                    .log_entries()
                    .iter()
                    .map(|entry| entry.message.clone())
                    .collect::<Vec<_>>()
            )
        })
}

/// A directory of the test's own to export into, removed when it ends.
///
/// An export writes a file, and the export tests are the one place in this
/// module that must, so they write here rather than anywhere the machine
/// keeps its own data.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("fp-app-export-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self { dir }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const EXPORT_SECRET: &str = "correct horse battery staple";

/// The last thing the window was told.
///
/// A success is a toast and a log line, not the banner: the banner is for
/// problems, which stay until they are dismissed. A failed export therefore
/// has to be read from `notice` and a successful one from here, and the
/// tests below do exactly that.
fn last_message(fixture: &Fixture) -> String {
    fixture
        .state
        .toasts()
        .last()
        .map(|toast| toast.message.clone())
        .expect("the window was told something")
}

fn seed_proxy_holding_a_password(fixture: &mut Fixture) -> ProxyId {
    fixture
        .state
        .create_proxy(
            "Zurich exit",
            ProxyOutbound::Socks5(domain::Socks5Outbound {
                host: "203.0.113.10".to_string(),
                port: 1080,
                username: Some("alice".to_string()),
                password: Some(EXPORT_SECRET.to_string()),
            }),
        )
        .expect("create proxy")
}

/// A configuration file to import, produced the honest way: by exporting
/// from a populated installation. Building one by hand here would quietly
/// decide what a real file looks like, and the export is the authority on
/// that.
fn write_backup_from(fixture: &mut Fixture, path: &std::path::Path) {
    fixture.state.load().expect("load");
    fixture
        .state
        .set_export_path(path.to_string_lossy().to_string());
    fixture.state.export_configuration().expect("export");
}

/// A file to restore, produced the honest way: by exporting from a populated
/// installation. Nothing hand-builds what the export is the authority on.
fn backup_with_one_profile(scratch: &Scratch) -> PathBuf {
    let mut source = fixture();
    seed_core(&source);
    source.state.load().expect("load");
    source.state.create_profile("Work laptop").expect("create");
    let backup = scratch.join("config.json");
    write_backup_from(&mut source, &backup);
    backup
}

/// The whole point of the lease: while a copy owns these profiles, nothing
/// else may touch them - not a second copy, not a start, not a configuration
/// replacement - and when the copy is over they are free again.
///
/// The window this closes is the one the snapshot cannot: `start` returns when
/// its command is queued, so a profile whose browser is coming up still reads
/// as stopped. The worker is what holds the copy's lease, so the test holds it
/// the same way - by keeping the value alive.
/// A stopped profile that leaves through a proxy, ready to be opened.
fn proxied(fixture: &mut Fixture, name: &str) -> (ProfileId, ProxyId) {
    seed_core(fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile(name).expect("create");
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let mut profile = fixture.state.profile(id).expect("the profile");
    profile.proxy_id = Some(proxy_id);
    fixture.state.update_profile(profile).expect("assign");
    (id, proxy_id)
}

/// The reading a passing test produces, for the tests that only care that
/// something came back.
fn through_the_proxy() -> Result<Diagnosis, Fault> {
    Ok(Diagnosis {
        exit_ip: "198.51.100.9".to_string(),
        elapsed: Duration::from_millis(120),
    })
}

fn commands(fixture: &Fixture) -> Vec<String> {
    fixture.runtime.commands.lock().expect("commands").clone()
}

mod activity;
mod cores;
mod maintenance;
mod profiles;
mod proxies;
mod settings;
mod verification;
