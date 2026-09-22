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

#[test]
fn restore_holds_the_installation_until_its_worker_finishes() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().unwrap();
    let id = fixture.state.create_profile("Original").unwrap();
    fixture
        .state
        .set_restore_path("/not-read-until-worker/config.json");
    fixture.state.set_browser_data_path("/backups/fp");
    let job = fixture.state.begin_restore(RestoreMode::Replace).unwrap();
    assert!(fixture.state.start(id).is_err());
    assert!(fixture.state.create_profile("During restore").is_err());
    assert!(fixture.state.delete_profile(id).is_err());
    let mut edited = fixture.state.profile(id).unwrap();
    edited.name = "Changed".into();
    // The service itself is guarded, independently of the UI entry point.
    assert!(fixture.state.profiles.update(edited).is_err());
    assert!(fixture.state.browser_data_job(Direction::ToBackup).is_err());
    assert!(fixture.runtime.commands.lock().unwrap().is_empty());
    drop(job);
    fixture
        .state
        .finish_restore(std::path::Path::new("unused"), &Err("cancelled".into()));
    assert!(fixture.state.create_profile("After restore").is_ok());
}

#[test]
fn editing_a_profile_writes_it_and_keeps_the_row_in_step() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Before").expect("create");

    let mut edited = fixture.state.profile(id).expect("the profile is loaded");
    edited.name = "After".to_string();
    edited.fingerprint.seed = 999;
    edited.window.width = 1600;
    fixture.state.update_profile(edited).expect("save");

    let row = fixture
        .state
        .rows()
        .iter()
        .find(|row| row.profile.id == id)
        .expect("the row is still listed");
    assert_eq!(row.profile.name, "After");
    assert_eq!(row.profile.fingerprint.seed, 999);
    assert_eq!(row.profile.window.width, 1600);
    assert_eq!(
        fixture.profiles.get(id).expect("stored").unwrap().name,
        "After",
        "the edit reached storage, not just the view"
    );
}

#[test]
fn a_refused_edit_leaves_the_stored_profile_alone() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Kept").expect("create");

    let mut broken = fixture.state.profile(id).expect("loaded");
    broken.name = "   ".to_string();

    assert!(fixture.state.update_profile(broken).is_err());
    assert!(
        fixture.state.notice().is_some_and(|notice| notice.error),
        "the refusal is shown in the banner"
    );
    assert_eq!(
        fixture.profiles.get(id).expect("stored").unwrap().name,
        "Kept"
    );
}

#[test]
fn a_duplicate_gets_its_own_identity_and_is_selected() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Source").expect("create");
    fixture.state.select(id);

    let copy = fixture.state.duplicate_profile(id).expect("duplicate");

    assert_ne!(copy, id);
    assert_eq!(fixture.state.selected_id(), Some(copy));
    let source = fixture.state.profile(id).expect("source");
    let duplicated = fixture.state.profile(copy).expect("copy");
    assert_ne!(duplicated.fingerprint.seed, source.fingerprint.seed);
    assert_ne!(duplicated.user_data_dir, source.user_data_dir);
    assert!(duplicated.name.contains("Source") || duplicated.name.contains("copy"));
}

#[test]
fn deleting_a_profile_removes_it_and_forgets_its_reading() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture
        .state
        .finish_verification(id, Ok(VerificationReport::default()));

    fixture.state.delete_profile(id).expect("delete");

    assert!(fixture.state.rows().is_empty(), "the row is gone");
    assert!(fixture.state.profile(id).is_none());
    assert!(
        fixture.state.verification(id).is_none(),
        "a deleted profile keeps no verification result"
    );
    assert_eq!(fixture.state.selected_id(), None);
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

#[test]
fn an_empty_filter_lists_every_profile() {
    let (mut fixture, _, _, _) = filtered_fixture();

    assert_eq!(fixture.state.visible_rows().len(), 3);

    // Whitespace is not a filter: it is a field someone tabbed through.
    fixture.state.set_profile_filter("   ");
    assert_eq!(fixture.state.visible_rows().len(), 3);
}

#[test]
fn a_filter_narrows_the_list_to_the_profiles_that_match() {
    let (mut fixture, work, _, _) = filtered_fixture();

    fixture.state.set_profile_filter("work");

    let visible = fixture.state.visible_rows();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].profile.id, work);
    assert_eq!(
        fixture.state.profile_filter(),
        "work",
        "the field holds what was typed, filter or not"
    );
}

#[test]
fn a_filter_ignores_case_and_the_space_around_it() {
    let (mut fixture, work, _, _) = filtered_fixture();

    fixture.state.set_profile_filter("  WORK  ");

    let visible = fixture.state.visible_rows();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].profile.id, work);
}

#[test]
fn a_filter_matches_the_seed_as_well_as_the_name() {
    let (mut fixture, _, _, staging) = filtered_fixture();

    fixture.state.set_profile_filter("1234");

    let visible = fixture.state.visible_rows();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].profile.id, staging);
}

#[test]
fn a_filter_matches_the_core_and_the_proxy_a_profile_runs_on() {
    let (mut fixture, _, shopping, _) = filtered_fixture();
    let zurich = fixture
        .state
        .create_proxy("Zurich exit", socks5("127.0.0.1", 1080))
        .expect("create a proxy");
    let mut with_proxy = fixture.state.profile(shopping).expect("loaded");
    with_proxy.proxy_id = Some(zurich);
    fixture.state.update_profile(with_proxy).expect("save");

    // The core is shared, so a term only it carries matches all three.
    fixture.state.set_profile_filter("test core");
    assert_eq!(fixture.state.visible_rows().len(), 3);

    // The proxy belongs to one profile and is in none of their names.
    fixture.state.set_profile_filter("zurich");
    let visible = fixture.state.visible_rows();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].profile.id, shopping);

    // The label the row prints is not a field the filter reads, so a
    // profile with no proxy cannot answer to the word for having none.
    fixture.state.set_profile_filter("direct");
    assert!(
        fixture.state.visible_rows().is_empty(),
        "the filter matches what is stored, not what the row renders"
    );
}

#[test]
fn a_filter_that_matches_nothing_hides_the_list_without_touching_a_profile() {
    let (mut fixture, _, _, _) = filtered_fixture();

    fixture.state.set_profile_filter("nothing is called this");

    assert!(fixture.state.visible_rows().is_empty());
    assert_eq!(
        fixture.state.rows().len(),
        3,
        "a filter is a view of the list, not a way to lose profiles"
    );
}

#[test]
fn a_filtered_out_profile_keeps_its_focus_and_keeps_running() {
    let (mut fixture, work, _, _) = filtered_fixture();
    fixture.state.select(work);
    fixture.state.start(work).expect("start");
    let started = fixture
        .state
        .rows()
        .iter()
        .find(|row| row.profile.id == work)
        .expect("the row is listed")
        .state();
    assert!(
        matches!(started, RuntimeState::Running),
        "the fixture really starts it"
    );

    fixture.state.set_profile_filter("shopping");

    assert_eq!(
        fixture.state.selected_id(),
        Some(work),
        "the panel answers for what was chosen, not for what is listed"
    );
    let visible = fixture.state.visible_rows();
    assert_eq!(visible.len(), 1);
    assert_ne!(visible[0].profile.id, work);
    let after = fixture
        .state
        .rows()
        .iter()
        .find(|row| row.profile.id == work)
        .expect("still listed")
        .state();
    assert!(
        matches!(after, RuntimeState::Running),
        "hiding a row cannot stop the browser behind it"
    );
}

fn socks5(host: &str, port: u16) -> ProxyOutbound {
    ProxyOutbound::Socks5(domain::Socks5Outbound {
        host: host.to_string(),
        port,
        username: None,
        password: None,
    })
}

/// A switch reaches the words the window *writes* afterwards, not only the
/// ones it looks up while rendering: a notice and a log line are built the
/// moment the event happens, so they have to be built in the language in
/// force then - and the file has to agree with the window, because the log
/// outlives it.
#[test]
fn switching_the_language_reaches_what_the_window_writes_next() {
    let dir = std::env::temp_dir().join(format!("fp-app-language-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    // The log file is handed in rather than opened by the fixture, so the
    // test can read the bytes the window wrote.
    let log = crate::log_file::LogFile::open(&dir.join("logs")).expect("log file");
    let mut fixture = fixture_full(&dir.join("config.json"), None, Some(log), None);

    fixture
        .state
        .set_language(Lang::Zh)
        .expect("the choice is stored");
    assert_eq!(fixture.state.language(), Lang::Zh);

    fixture
        .state
        .set_theme(ThemeChoice::Light)
        .expect("the window repaints");
    let toast = fixture
        .state
        .toasts()
        .last()
        .expect("a toast")
        .message
        .clone();
    assert!(toast.contains("主题"), "{toast}");

    fixture
        .state
        .append_log(LogLevel::Warning, None, "a warning".to_string());
    let line = std::fs::read_to_string(dir.join("logs").join("activity.log"))
        .expect("the line reached the file");
    assert!(line.contains("警告"), "{line}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_created_proxy_is_listed_and_stored() {
    let mut fixture = fixture();
    let id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");

    let rows = fixture.state.proxy_rows().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].proxy.name, "Office");
    assert_eq!(rows[0].endpoint(), "socks5://10.0.0.1:1080");
    assert_eq!(rows[0].usage_label(en()), "not assigned");
    assert!(fixture.proxies.get(id).expect("stored").is_some());
}

#[test]
fn a_broken_proxy_is_refused_and_reported() {
    let mut fixture = fixture();
    assert!(
        fixture
            .state
            .create_proxy("Broken", socks5("", 1080))
            .is_err()
    );
    assert!(fixture.state.proxy_rows().expect("rows").is_empty());
    assert!(
        fixture.state.notice().is_some_and(|notice| notice.error),
        "the refusal is shown in the banner"
    );
}

#[test]
fn the_picker_offers_every_stored_proxy() {
    let mut fixture = fixture();
    assert!(fixture.state.proxy_choices().is_empty());
    let id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create");
    assert_eq!(
        fixture.state.proxy_choices(),
        vec![(id, "Office".to_string())]
    );
}

#[test]
fn assigning_a_proxy_reaches_storage_and_the_usage_list() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let profile_id = fixture.state.create_profile("Proxied").expect("create");
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");

    let mut profile = fixture.state.profile(profile_id).expect("loaded");
    profile.proxy_id = Some(proxy_id);
    fixture.state.update_profile(profile).expect("assign");

    assert_eq!(
        fixture.proxies.get(proxy_id).expect("stored").map(|p| p.id),
        Some(proxy_id)
    );
    let rows = fixture.state.proxy_rows().expect("rows");
    assert_eq!(rows[0].used_by, vec!["Proxied".to_string()]);
    assert_eq!(rows[0].usage_label(en()), "used by Proxied");
    assert!(rows[0].is_used());
    assert_eq!(
        fixture
            .profiles
            .get(profile_id)
            .expect("stored")
            .and_then(|profile| profile.proxy_id),
        Some(proxy_id),
        "the assignment reached storage"
    );
    assert_eq!(
        fixture.state.rows()[0].proxy_name.as_deref(),
        Some("Office"),
        "the row shows the assigned proxy"
    );
}

/// Refused rather than quietly rewritten to Direct: the assignment would
/// otherwise be nulled out and that profile's traffic would leave unproxied.
#[test]
fn a_proxy_in_use_cannot_be_deleted() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let profile_id = fixture.state.create_profile("Proxied").expect("create");
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let mut profile = fixture.state.profile(profile_id).expect("loaded");
    profile.proxy_id = Some(proxy_id);
    fixture.state.update_profile(profile).expect("assign");

    let error = fixture.state.delete_proxy(proxy_id).unwrap_err();
    assert!(error.to_string().contains("Proxied"), "{error}");
    assert_eq!(fixture.state.proxy_rows().expect("rows").len(), 1);
    assert_eq!(
        fixture
            .profiles
            .get(profile_id)
            .expect("stored")
            .and_then(|profile| profile.proxy_id),
        Some(proxy_id),
        "the assignment survived the refused delete"
    );
}

#[test]
fn a_proxy_is_deleted_once_nothing_uses_it() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let profile_id = fixture.state.create_profile("Proxied").expect("create");
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let mut profile = fixture.state.profile(profile_id).expect("loaded");
    profile.proxy_id = Some(proxy_id);
    fixture.state.update_profile(profile).expect("assign");

    let mut unassigned = fixture.state.profile(profile_id).expect("loaded");
    unassigned.proxy_id = None;
    fixture.state.update_profile(unassigned).expect("unassign");
    fixture.state.delete_proxy(proxy_id).expect("delete");

    assert!(fixture.state.proxy_rows().expect("rows").is_empty());
    assert_eq!(
        fixture.state.rows()[0].proxy_name,
        None,
        "the row falls back to direct"
    );
}

#[test]
fn saving_a_proxy_keeps_a_running_profile_on_what_it_started_with() {
    let mut fixture = fixture();
    let profile_id = running_profile(&mut fixture);
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");

    let mut proxy = fixture.state.proxy(proxy_id).expect("stored");
    proxy.outbound = socks5("10.0.0.2", 1081);
    fixture.state.update_proxy(proxy).expect("save");

    assert_eq!(
        fixture.runtime.snapshot_of(profile_id).map(|s| s.state),
        Some(RuntimeState::Running),
        "editing a proxy does not disturb a running profile"
    );
    assert!(
        fixture
            .state
            .toasts()
            .iter()
            .any(|toast| toast.message.contains("next start") || toast.message.contains("keep")),
        "the toast says when the change takes effect"
    );
    assert!(
        fixture.state.notice().is_none(),
        "a success is not a banner: it is a toast and a log line"
    );
}

#[test]
fn an_added_core_carries_the_version_its_binary_reported() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("add", Some("Chromium 148.0.7778.215"));

    let id = fixture
        .state
        .add_core(None, binary.path_buf())
        .expect("add");

    let rows = fixture.state.core_rows().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].core.id, id);
    assert_eq!(rows[0].core.version, "Chromium 148.0.7778.215");
    assert_eq!(rows[0].core.major, 148);
    assert!(rows[0].present, "the binary is on disk");
    assert_eq!(rows[0].usage_label(en()), "not used");
    assert_eq!(
        rows[0].generation_label().as_deref(),
        Some("Chrome 144+ · spoofing exclusions honoured")
    );
}

/// The pivot made visible: a core below it is labelled as not offering the
/// switches this project only verified from 144.
#[test]
fn a_legacy_core_says_the_noise_switches_are_not_offered() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("legacy", Some("Chromium 128.0.0.0"));
    fixture
        .state
        .add_core(Some("Old".to_string()), binary.path_buf())
        .expect("add");

    let rows = fixture.state.core_rows().expect("rows");
    assert_eq!(rows[0].core.name, "Old");
    assert_eq!(rows[0].core.major, 128);
    assert_eq!(
        rows[0].generation_label().as_deref(),
        Some("Chrome 143 and older · spoofing exclusions not honoured")
    );
}

#[test]
fn a_binary_without_a_usable_version_is_refused_and_reported() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("silent", None);

    let error = fixture.state.add_core(None, binary.path_buf()).unwrap_err();

    assert!(error.to_string().contains("--version"), "{error}");
    assert!(fixture.state.core_rows().expect("rows").is_empty());
    assert!(fixture.state.notice().is_some_and(|notice| notice.error));
}

/// A core whose version was never read has no capability table. Asking for
/// one used to reach `CoreCapabilities::for_major(0)`, which is a debug
/// assertion: in a debug build the window would have panicked.
#[test]
fn verifying_through_a_core_without_a_version_is_refused_not_asserted() {
    let mut fixture = fixture();
    let unknown = domain::BrowserCore {
        id: CoreId::new(),
        name: "unknown 0".to_string(),
        executable: PathBuf::from("/tmp/whatever/chrome"),
        version: "unknown".to_string(),
        major: 0,
    };
    fixture.cores.save(&unknown).expect("save core");
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("unreadable").expect("create");
    fixture.runtime.set_state(id, RuntimeState::Running);
    fixture.runtime.set_cdp_port(id, 9333);
    fixture.state.refresh_runtime();

    let error = fixture.state.begin_verification(id).unwrap_err();
    assert!(error.to_string().contains("no detected version"), "{error}");
    assert_eq!(
        fixture.state.core_rows().expect("rows")[0].generation_label(),
        None,
        "there is no generation to show"
    );
}

#[test]
fn re_detecting_a_replaced_binary_updates_the_major_and_the_label() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("redetect", Some("Chromium 148.0.7778.215"));
    let id = fixture
        .state
        .add_core(None, binary.path_buf())
        .expect("add");

    // The same path now answers with another build.
    binary.replace(Some("Chromium 128.0.0.0"));
    fixture.state.redetect_core(id).expect("redetect");

    let rows = fixture.state.core_rows().expect("rows");
    assert_eq!(rows[0].core.major, 128);
    assert_eq!(
        rows[0].generation_label().as_deref(),
        Some("Chrome 143 and older · spoofing exclusions not honoured")
    );
    assert!(
        fixture
            .state
            .toasts()
            .iter()
            .any(|toast| toast.message.contains("128")),
        "the toast says what it found"
    );
}

#[test]
fn renaming_a_core_keeps_its_version() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("rename", Some("Chromium 148.0.7778.215"));
    let id = fixture
        .state
        .add_core(None, binary.path_buf())
        .expect("add");

    let mut core = fixture.state.core(id).expect("stored");
    core.name = "Work browser".to_string();
    fixture.state.update_core(core).expect("update");

    let rows = fixture.state.core_rows().expect("rows");
    assert_eq!(rows[0].core.name, "Work browser");
    assert_eq!(rows[0].core.major, 148);
}

#[test]
fn a_core_a_profile_launches_with_cannot_be_deleted() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let core_id = fixture.state.core_rows().expect("rows")[0].core.id;
    fixture.state.create_profile("Uses it").expect("create");

    let error = fixture.state.delete_core(core_id).unwrap_err();
    assert!(error.to_string().contains("Uses it"), "{error}");
    assert_eq!(fixture.state.core_rows().expect("rows").len(), 1);
}

#[test]
fn an_unused_core_is_deleted() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("unused", Some("Chromium 148.0.7778.215"));
    let id = fixture
        .state
        .add_core(None, binary.path_buf())
        .expect("add");
    fixture.state.delete_core(id).expect("delete");
    assert!(fixture.state.core_rows().expect("rows").is_empty());
    assert!(!fixture.state.has_core());
}

#[test]
fn the_settings_rows_say_where_each_value_came_from() {
    let fixture = fixture();
    let rows = fixture.state.setting_rows();

    assert_eq!(
        rows.len(),
        SettingKey::ALL.len(),
        "every setting is listed, whatever the list grows to"
    );
    let data_dir = rows
        .iter()
        .find(|row| row.key == SettingKey::DataDir)
        .expect("the data directory row");
    assert_eq!(data_dir.source, crate::settings::Source::Default);
    assert_eq!(data_dir.source_label(en()), "from the default");
    assert!(
        !data_dir.key.editable(),
        "the config file lives in the data directory, so the directory is \
             chosen by the environment or the platform, not from inside it"
    );
    assert_eq!(data_dir.key.effect().label(en()), "next start");
    assert!(
        data_dir.note.is_some(),
        "the row says how to move it, because the row cannot do it"
    );

    let chromium = rows
        .iter()
        .find(|row| row.key == SettingKey::ChromiumBin)
        .expect("the chromium row");
    assert!(
        !chromium.key.editable(),
        "the binary is chosen by the environment and shown on the cores page"
    );

    let endpoint = rows
        .iter()
        .find(|row| row.key == SettingKey::EchoUrl)
        .expect("the proxy test endpoint row");
    assert!(
        endpoint.key.editable(),
        "the endpoint a proxy test asks is the user's to choose"
    );
    assert_eq!(
        endpoint.key.effect().label(en()),
        "now",
        "a test asks whatever the endpoint is at the time, not what it was at startup"
    );
    assert!(
        endpoint.source == crate::settings::Source::Default,
        "the fixture sets no endpoint, so the built-in one is shown"
    );
}

#[test]
fn saving_a_setting_stores_it_and_says_when_it_applies() {
    let dir = std::env::temp_dir().join(format!("fp-app-settings-{}", CoreId::new()));
    let config = dir.join("config.json");
    let mut fixture = fixture_with_config(&config);

    fixture
        .state
        .update_setting(SettingKey::XrayExecutable, "/opt/xray")
        .expect("save");

    let stored = std::fs::read_to_string(&config).expect("the config file was written");
    assert!(stored.contains("/opt/xray"), "{stored}");
    let toast = fixture
        .state
        .toasts()
        .last()
        .expect("a toast saying what happened");
    assert!(
        toast.message.contains("next start"),
        "the toast says when it applies: {}",
        toast.message
    );
    assert!(
        fixture.state.notice().is_none(),
        "saving a setting is not a problem, so the banner stays clear"
    );
    assert_eq!(
        fixture
            .state
            .setting_rows()
            .iter()
            .find(|row| row.key == SettingKey::XrayExecutable)
            .expect("the row")
            .value,
        "/opt/xray",
        "the page shows what the next start will use"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The appearance is stored and reported. It is read back out of the state
/// rather than only out of the file, because the state is what the window
/// paints from: a change that reached the file alone would leave the window
/// showing one thing while the file said another.
#[test]
fn choosing_an_appearance_stores_it_and_says_so() {
    let dir = std::env::temp_dir().join(format!("fp-app-theme-{}", CoreId::new()));
    let config = dir.join("config.json");
    let mut fixture = fixture_with_config(&config);
    assert_eq!(fixture.state.theme(), ThemeChoice::Dark, "the default");

    fixture.state.set_theme(ThemeChoice::Light).expect("save");

    assert_eq!(fixture.state.theme(), ThemeChoice::Light);
    let stored = std::fs::read_to_string(&config).expect("the config file was written");
    assert!(stored.contains("\"light\""), "{stored}");
    let toast = fixture.state.toasts().last().expect("a toast saying so");
    assert!(toast.message.contains("light theme"), "{}", toast.message);
    assert!(
        fixture.state.notice().is_none(),
        "choosing an appearance is not a problem, so the banner stays clear"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_setting_is_reported_and_changes_nothing() {
    let mut fixture = fixture();
    fixture
        .state
        .update_setting(SettingKey::DataDir, "   ")
        .expect_err("an empty value is refused");
    assert!(fixture.state.notice().is_some_and(|notice| notice.error));
    assert_eq!(
        fixture
            .state
            .setting_rows()
            .iter()
            .find(|row| row.key == SettingKey::DataDir)
            .expect("the row")
            .source,
        crate::settings::Source::Default,
        "nothing was stored"
    );
}

#[test]
fn the_page_can_be_switched() {
    let mut fixture = fixture();
    assert_eq!(fixture.state.page(), Page::Profiles);
    fixture.state.set_page(Page::Proxies);
    assert_eq!(fixture.state.page(), Page::Proxies);
    assert!(Page::Proxies.is_ready());
    assert!(
        Page::Settings.is_ready(),
        "every page in the sidebar is built now"
    );
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

/// The address question is only put to a profile that makes a claim about
/// one, and the expectation behind it is what the proxy was *measured* at.
#[test]
fn a_proxied_profile_is_asked_about_the_address_its_proxy_was_tested_at() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied_profile(&mut fixture);
    fixture.state.finish_proxy_test(
        proxy_id,
        false,
        Ok(Diagnosis {
            exit_ip: "203.0.113.7".to_string(),
            elapsed: Duration::from_millis(120),
        }),
    );

    let job = fixture.state.begin_verification(id).expect("begin");

    let egress = job.egress.expect("a proxied profile is asked");
    // The endpoint is the one the Settings page shows, so the reading can
    // never be taken against something the user cannot see.
    let shown = fixture
        .state
        .setting_rows()
        .iter()
        .find(|row| row.key == SettingKey::EchoUrl)
        .map(|row| row.value.clone())
        .expect("the endpoint row");
    assert_eq!(egress.echo_url, shown);
    assert_eq!(
        egress.expected.as_deref(),
        Some("203.0.113.7"),
        "the expectation is the address the proxy was tested at"
    );
}

/// A profile that was never meant to leave by a proxy claims nothing about
/// an address, and the endpoint learns the address it is asked from: there
/// is no reason to spend that on a browser with nothing to check.
#[test]
fn a_profile_without_a_proxy_is_not_asked_about_an_address() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);

    let job = fixture.state.begin_verification(id).expect("begin");

    assert!(job.egress.is_none());
}

/// A reading is still worth having before the proxy has been tested - it is
/// the first thing anyone wants to know - but with nothing measured there
/// is no claim for it to contradict.
#[test]
fn an_untested_proxy_leaves_nothing_to_disagree_with() {
    let mut fixture = fixture();
    let (id, _) = proxied_profile(&mut fixture);

    let job = fixture.state.begin_verification(id).expect("begin");

    let egress = job.egress.expect("a proxied profile is asked");
    assert_eq!(egress.expected, None);
}

/// The half of the answer the row cannot show: the row says the fingerprint
/// was confirmed, and this is where the traffic went.
#[test]
fn a_confirmed_reading_is_logged_with_the_address_it_left_from() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.finish_verification(
        id,
        Ok(VerificationReport {
            exit_ip: Some("203.0.113.7".to_string()),
            ..VerificationReport::default()
        }),
    );

    let line = confirmed_line_in(&fixture);

    assert!(line.contains("203.0.113.7"), "{line}");
}

/// "Confirmed" on its own would read as though the address had been checked
/// too, so a reading that was not taken says so in the same line.
#[test]
fn a_confirmed_reading_that_could_not_read_an_address_says_so() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.finish_verification(
        id,
        Ok(VerificationReport {
            exit_unreadable: Some("the browser's page never committed".to_string()),
            ..VerificationReport::default()
        }),
    );

    let line = confirmed_line_in(&fixture);

    assert!(line.contains("exit address was not read"), "{line}");
    assert!(line.contains("never committed"), "{line}");
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

/// A wrong address is a finding, not a footnote: the profile claims to
/// leave by a proxy that was measured somewhere else.
#[test]
fn a_profile_that_leaves_from_elsewhere_is_not_confirmed() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.finish_verification(
        id,
        Ok(VerificationReport {
            discrepancies: vec![Discrepancy {
                claim: "exit address",
                expected: "203.0.113.7 (where the proxy was tested)".to_string(),
                observed: "198.51.100.9".to_string(),
            }],
            exit_ip: Some("198.51.100.9".to_string()),
            exit_unreadable: None,
        }),
    );

    let verification = fixture.state.verification(id).expect("recorded");

    assert_eq!(verification.label(en()), "1 claim not confirmed");
    assert_eq!(
        verification
            .report()
            .and_then(|report| report.exit_label(en())),
        Some("traffic left from 198.51.100.9".to_string()),
        "the address is still worth showing: it is where the traffic went"
    );
}

#[test]
fn a_verification_needs_a_running_browser_with_a_debug_port() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_profile("stopped")
        .expect("create profile");

    assert!(fixture.state.begin_verification(id).is_err());
    assert!(fixture.state.verification(id).is_none());
}

#[test]
fn a_verification_job_carries_the_profile_and_its_capabilities() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);

    let job = fixture.state.begin_verification(id).expect("begin");

    assert_eq!(job.port, 9333);
    assert_eq!(job.profile_id, id);
    assert_eq!(
        job.profile.seed,
        fixture.state.rows()[0].profile.fingerprint.seed
    );
    assert_eq!(
        job.capabilities.major, 144,
        "the capabilities come from the profile's own core"
    );
    assert!(
        fixture
            .state
            .verification(id)
            .is_some_and(Verification::is_running)
    );
}

#[test]
fn an_old_verification_does_not_replace_the_current_task() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    let old = fixture.state.begin_verification(id).unwrap();
    fixture.state.forget_verification(id);
    let current = fixture.state.begin_verification(id).unwrap();
    fixture
        .state
        .complete_verification(&old, Ok(VerificationReport::default()));
    assert!(fixture.state.verification(id).unwrap().is_running());
    fixture
        .state
        .complete_verification(&current, Err("current failure".into()));
    assert!(
        matches!(fixture.state.verification(id), Some(Verification::Unreadable(message)) if message == "current failure")
    );
}

#[test]
fn verification_from_a_browser_that_exited_is_discarded() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    let job = fixture.state.begin_verification(id).unwrap();
    fixture.runtime.set_state(id, RuntimeState::Stopped);
    fixture
        .state
        .complete_verification(&job, Ok(VerificationReport::default()));
    assert!(fixture.state.verification(id).is_none());
}

#[test]
fn a_second_verification_of_the_same_profile_is_refused() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.begin_verification(id).expect("first");

    assert!(fixture.state.begin_verification(id).is_err());
}

#[test]
fn an_outcome_records_confirmation_disagreement_or_failure() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);

    fixture
        .state
        .finish_verification(id, Ok(VerificationReport::default()));
    assert!(matches!(
        fixture.state.verification(id),
        Some(Verification::Confirmed(_))
    ));

    let disagreement = Discrepancy {
        claim: "platform",
        expected: "Win32".to_string(),
        observed: "Linux x86_64".to_string(),
    };
    fixture.state.finish_verification(
        id,
        Ok(VerificationReport::from_discrepancies(vec![
            disagreement.clone(),
        ])),
    );
    let recorded = fixture.state.verification(id).expect("recorded");
    assert_eq!(
        recorded.disagreements(),
        std::slice::from_ref(&disagreement)
    );
    assert_eq!(recorded.label(en()), "1 claim not confirmed");

    fixture
        .state
        .finish_verification(id, Err("no debug port".to_string()));
    let failed = fixture.state.verification(id).expect("recorded");
    assert_eq!(failed.failure(), Some("no debug port"));
    assert!(
        !failed.is_running(),
        "a failed reading is not a reading in flight"
    );
}

#[test]
fn stopping_or_restarting_a_profile_drops_a_stale_reading() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture
        .state
        .finish_verification(id, Ok(VerificationReport::default()));
    assert!(fixture.state.verification(id).is_some());

    fixture.state.restart(id).expect("restart");
    assert!(
        fixture.state.verification(id).is_none(),
        "a restarted browser has a new fingerprint"
    );

    fixture
        .state
        .finish_verification(id, Ok(VerificationReport::default()));
    fixture.state.stop(id).expect("stop");
    assert!(fixture.state.verification(id).is_none());
}

/// A proxy has to be judgeable before anything is launched with it, so a
/// test needs no running profile - it starts an engine of its own.
#[test]
fn a_proxy_can_be_tested_with_nothing_running() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");

    let job = fixture.state.begin_proxy_test(id).expect("begin");

    assert_eq!(job.proxy_id, id);
    assert_eq!(job.proxy.name, "Office");
    assert_eq!(
        job.live_port, None,
        "nothing is running, so the test starts its own engine"
    );
    assert!(!job.is_live());
    assert_eq!(
        job.echo_url,
        fixture.state.setting_rows()[2].value,
        "the endpoint is the configured one"
    );
    assert!(
        fixture
            .state
            .proxy_test(id)
            .is_some_and(ProxyTest::is_running)
    );
}

/// A test of a proxy that is not stored is a mistake, not a job.
#[test]
fn testing_a_proxy_that_is_not_there_is_refused() {
    let mut fixture = fixture();
    assert!(fixture.state.begin_proxy_test(ProxyId::new()).is_err());
}

#[test]
fn a_second_test_of_the_same_proxy_is_refused() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");

    fixture.state.begin_proxy_test(id).expect("first");
    assert!(
        fixture.state.begin_proxy_test(id).is_err(),
        "a second engine for an answer already on its way"
    );

    fixture.state.finish_proxy_test(
        id,
        false,
        Err(Fault::new(FaultClass::Unreachable, "no route")),
    );
    assert!(
        fixture.state.begin_proxy_test(id).is_ok(),
        "the slot is free once the answer is in"
    );
}

/// When a profile is already up, its engine is the one that would have to
/// forward, so the test asks that engine instead of starting a second one.
#[test]
fn a_running_profile_makes_the_test_probe_its_engine() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let profile_id = fixture.state.create_profile("Proxied").expect("create");
    let mut profile = fixture.state.profile(profile_id).expect("loaded");
    profile.proxy_id = Some(proxy_id);
    fixture.state.update_profile(profile).expect("assign");
    fixture.runtime.set_state(profile_id, RuntimeState::Running);
    fixture.runtime.set_socks_port(profile_id, 51234);
    fixture.state.refresh_runtime();

    let job = fixture.state.begin_proxy_test(proxy_id).expect("begin");

    assert_eq!(
        job.live_port,
        Some(51234),
        "the engine a profile is using is the one that has to carry the request"
    );
    assert!(job.is_live());
}

/// The port is cleared when a profile stops, so a test cannot probe a port
/// that nothing is listening on and call the answer a proxy failure.
#[test]
fn a_stopped_profile_leaves_no_port_to_probe() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let profile_id = fixture.state.create_profile("Proxied").expect("create");
    let mut profile = fixture.state.profile(profile_id).expect("loaded");
    profile.proxy_id = Some(proxy_id);
    fixture.state.update_profile(profile).expect("assign");
    fixture.runtime.set_state(profile_id, RuntimeState::Running);
    fixture.runtime.set_socks_port(profile_id, 51234);
    fixture.state.refresh_runtime();

    fixture.runtime.set_state(profile_id, RuntimeState::Stopped);
    fixture.state.refresh_runtime();

    // The snapshot still holds the port, which is exactly the trap: it is
    // the profile's own state that decides, so a stale port is never
    // dialled and a dead socket is never reported as a broken proxy.
    assert_eq!(
        fixture
            .runtime
            .snapshot_of(profile_id)
            .expect("snapshot")
            .socks_port,
        Some(51234),
        "the port outlives the profile in the snapshot"
    );

    let job = fixture.state.begin_proxy_test(proxy_id).expect("begin");
    assert_eq!(job.live_port, None);
}

#[test]
fn an_outcome_records_the_address_and_the_path_it_left_by() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    fixture.state.begin_proxy_test(id).expect("begin");

    fixture.state.finish_proxy_test(
        id,
        true,
        Ok(Diagnosis {
            exit_ip: "198.51.100.9".to_string(),
            elapsed: Duration::from_millis(431),
        }),
    );

    let test = fixture.state.proxy_test(id).expect("a result");
    let reading = test.reading().expect("a reading, not a fault");
    assert_eq!(reading.exit_ip, "198.51.100.9");
    assert_eq!(reading.elapsed, Duration::from_millis(431));
    assert!(reading.live, "the request went through the running engine");
    assert_eq!(test.label(en()), "exit 198.51.100.9");
    assert!(test.fault().is_none());
}

/// A fault is not a reading with an empty address: it says where the
/// request stopped, because that is what decides the fix.
#[test]
fn a_failed_test_keeps_the_class_and_the_evidence() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    fixture.state.begin_proxy_test(id).expect("begin");

    fixture.state.finish_proxy_test(
        id,
        false,
        Err(Fault::new(
            FaultClass::Auth,
            "upstream refused the credentials it was offered",
        )),
    );

    let test = fixture.state.proxy_test(id).expect("a result");
    assert!(
        test.reading().is_none(),
        "nothing left, so nothing was read"
    );
    let fault = test.fault().expect("the fault");
    assert_eq!(fault.class, FaultClass::Auth);
    assert!(fault.detail.contains("credentials"), "{fault}");
    assert_eq!(test.label(en()), "no traffic (authentication)");
    assert!(
        test.detail(en())
            .expect("a detail line")
            .contains("no traffic reached the endpoint"),
        "the row and the log both have to say nothing arrived"
    );
}

/// The address and the path are the record: a row is replaced by the next
/// test, and the log is not.
#[test]
fn the_activity_log_keeps_the_address_the_path_and_the_class() {
    let dir = std::env::temp_dir().join(format!("fp-proxy-test-log-{}", CoreId::new()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut fixture = fixture_with_log(&dir);
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let ok = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let bad = fixture
        .state
        .create_proxy("Home", socks5("10.0.0.2", 1080))
        .expect("create proxy");

    fixture.state.begin_proxy_test(ok).expect("begin");
    fixture.state.finish_proxy_test(
        ok,
        false,
        Ok(Diagnosis {
            exit_ip: "198.51.100.9".to_string(),
            elapsed: Duration::from_millis(90),
        }),
    );
    fixture.state.begin_proxy_test(bad).expect("begin");
    fixture.state.finish_proxy_test(
        bad,
        false,
        Err(Fault::new(
            FaultClass::Unreachable,
            "no route to the upstream",
        )),
    );

    let lines: Vec<String> = fixture
        .state
        .log_entries()
        .iter()
        .map(|entry| entry.message.clone())
        .collect();
    // `Created proxy Office` is also a line about Office, so the search is
    // for the test's own record rather than for the name.
    let mention = |name: &str| {
        lines
            .iter()
            .find(|line| line.starts_with("proxy test:") && line.contains(name))
            .unwrap_or_else(|| panic!("no record of testing {name}: {lines:?}"))
    };
    let passed = mention("Office");
    assert!(passed.contains("198.51.100.9"), "{passed}");
    assert!(
        passed.contains("temporary engine"),
        "the path is the half of the answer the row cannot keep: {passed}"
    );
    let failed = mention("Home");
    assert!(failed.contains("unreachable"), "{failed}");
    assert!(failed.contains("no route"), "{failed}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The class is the whole point of keeping them apart, so it is what the
/// immediate feedback leads with.
#[test]
fn a_failure_toasts_the_class_and_a_success_the_address() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");

    fixture.state.begin_proxy_test(id).expect("begin");
    fixture.state.finish_proxy_test(
        id,
        false,
        Err(Fault::new(
            FaultClass::Timeout,
            "the request ran out of time",
        )),
    );
    let toast = fixture.state.toasts().last().expect("a toast").clone();
    assert_eq!(toast.kind, ToastKind::Error);
    assert!(toast.message.contains("timeout"), "{}", toast.message);

    fixture.state.begin_proxy_test(id).expect("begin again");
    fixture.state.finish_proxy_test(
        id,
        false,
        Ok(Diagnosis {
            exit_ip: "198.51.100.9".to_string(),
            elapsed: Duration::from_millis(90),
        }),
    );
    let toast = fixture.state.toasts().last().expect("a toast").clone();
    assert_eq!(toast.kind, ToastKind::Success);
    assert!(toast.message.contains("198.51.100.9"), "{}", toast.message);
}

/// A result is about one upstream. Editing it leaves the old answer sitting
/// under a new address, which reads as a claim about the new one.
#[test]
fn editing_a_proxy_forgets_what_the_old_one_answered() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    fixture.state.begin_proxy_test(id).expect("begin");
    fixture.state.finish_proxy_test(
        id,
        false,
        Ok(Diagnosis {
            exit_ip: "198.51.100.9".to_string(),
            elapsed: Duration::from_millis(90),
        }),
    );
    assert!(fixture.state.proxy_test(id).is_some());

    let mut proxy = fixture.state.proxy(id).expect("stored");
    proxy.outbound = socks5("10.0.0.2", 1080);
    fixture.state.update_proxy(proxy).expect("edit");

    assert!(
        fixture.state.proxy_test(id).is_none(),
        "the address it left from was the old upstream's"
    );
}

#[test]
fn deleting_a_proxy_forgets_its_result() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    fixture.state.begin_proxy_test(id).expect("begin");
    fixture.state.finish_proxy_test(
        id,
        false,
        Ok(Diagnosis {
            exit_ip: "198.51.100.9".to_string(),
            elapsed: Duration::from_millis(90),
        }),
    );

    fixture.state.delete_proxy(id).expect("delete");
    assert!(fixture.state.proxy_test(id).is_none());
}

#[test]
fn create_lists_the_profile_and_selects_it() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("initial load");

    let id = fixture.state.create_profile("Primary").expect("create");

    assert_eq!(fixture.state.rows().len(), 1);
    let row = &fixture.state.rows()[0];
    assert_eq!(row.profile.name, "Primary");
    assert_eq!(row.profile.id, id);
    assert_eq!(row.state(), RuntimeState::Stopped);
    assert_eq!(row.state_label(en()), "Stopped");
    assert_eq!(row.core_name, "Test Core 144");
    assert_eq!(fixture.state.selected_id(), Some(id));
    assert!(row.can_start());
    assert!(!row.can_stop());
}

#[test]
fn create_without_a_core_records_an_error_notice() {
    let mut fixture = fixture();
    fixture.state.load().expect("initial load");

    let error = fixture
        .state
        .create_profile("Primary")
        .expect_err("must fail");

    assert!(matches!(error, AppError::Conflict(_)));
    assert!(fixture.state.rows().is_empty());
    let notice = fixture.state.notice().expect("notice");
    assert!(notice.error);
    assert!(notice.message.contains("no browser core"));
}

#[test]
fn start_and_stop_update_the_row_from_the_snapshot() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Primary").expect("create");

    fixture.state.start(id).expect("start");
    let row = fixture.state.selected().expect("selected row");
    assert_eq!(row.state(), RuntimeState::Running);
    assert!(row.can_stop());
    assert!(!row.can_start());

    fixture.state.stop(id).expect("stop");
    assert_eq!(
        fixture.state.selected().expect("selected row").state(),
        RuntimeState::Stopped
    );

    let commands = fixture.runtime.commands.lock().expect("commands");
    assert_eq!(
        commands.as_slice(),
        [format!("start:{id}"), format!("stop:{id}")]
    );
}

#[test]
fn starting_an_unknown_profile_records_an_error_notice() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");

    let unknown = ProfileId::new();
    let error = fixture.state.start(unknown).expect_err("must fail");

    assert!(matches!(error, AppError::NotFound(_)));
    assert!(fixture.state.notice().expect("notice").error);
}

#[test]
fn refresh_runtime_picks_up_external_state_changes() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Primary").expect("create");

    // Simulate a supervisor-side crash that the UI never received as an event.
    fixture.runtime.set_state(
        id,
        RuntimeState::Crashed {
            message: "browser exited".to_string(),
        },
    );
    fixture.state.refresh_runtime();

    let row = fixture.state.selected().expect("selected row");
    assert_eq!(row.state_label(en()), "Crashed");
    assert_eq!(row.state_message(), Some("browser exited"));
    assert!(row.can_start());
}

#[test]
fn load_drops_a_selection_whose_profile_was_deleted() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Primary").expect("create");

    fixture.profiles.delete(id).expect("delete");
    fixture.state.load().expect("reload");

    assert!(fixture.state.rows().is_empty());
    assert_eq!(fixture.state.selected_id(), None);
}

#[test]
fn rows_are_sorted_by_name_and_renumbered_for_new_profiles() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");

    assert_eq!(fixture.state.next_profile_name(), "Profile 1");
    fixture.state.create_profile("Bravo").expect("create");
    assert_eq!(fixture.state.next_profile_name(), "Profile 2");
    fixture.state.create_profile("Alpha").expect("create");

    let names: Vec<&str> = fixture
        .state
        .rows()
        .iter()
        .map(|row| row.profile.name.as_str())
        .collect();
    assert_eq!(names, ["Alpha", "Bravo"]);
}

#[test]
fn a_success_is_a_toast_and_leaves_the_banner_clear() {
    let mut fixture = fixture();

    fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");

    let toast = fixture.state.toasts().last().expect("a toast");
    assert_eq!(toast.kind, ToastKind::Success);
    assert!(toast.message.contains("Office"), "{}", toast.message);
    assert!(
        fixture.state.notice().is_none(),
        "a success does not need an acknowledgement"
    );
}

#[test]
fn a_problem_owns_the_banner_until_it_is_dismissed() {
    let mut fixture = fixture();

    fixture
        .state
        .create_proxy("Broken", socks5("", 1080))
        .expect_err("refused");

    let notice = fixture.state.notice().expect("the banner");
    assert!(notice.error);
    assert_eq!(
        fixture.state.toasts().last().map(|toast| toast.kind),
        Some(ToastKind::Error),
        "the problem is a toast as well, so it is seen while it happens"
    );

    // A later success clears the problem: the banner is the current
    // problem, not every problem ever seen.
    fixture.state.dismiss_notice();
    assert!(fixture.state.notice().is_none());
    fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create");
    assert!(fixture.state.notice().is_none());
}

#[test]
fn draining_toasts_leaves_nothing_to_show_twice() {
    let mut fixture = fixture();
    fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create");
    assert!(!fixture.state.toasts().is_empty());

    let drained = fixture.state.drain_toasts();

    assert_eq!(drained.len(), 1);
    assert!(drained[0].message.contains("Office"));
    assert!(fixture.state.toasts().is_empty());
    assert!(fixture.state.drain_toasts().is_empty());
}

#[test]
fn every_notice_is_written_to_the_log() {
    let mut fixture = fixture();
    assert!(fixture.state.log_entries().is_empty());

    fixture.state.push_notice("Saved.", false);
    fixture.state.push_notice("it broke", true);

    let entries = fixture.state.log_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].level, LogLevel::Info);
    assert_eq!(entries[0].message, "Saved.");
    assert_eq!(entries[0].profile_id, None, "a window-level line");
    assert_eq!(entries[1].level, LogLevel::Error);
    assert_eq!(entries[1].message, "it broke");
}

#[test]
fn runtime_events_are_written_to_the_log_once_each() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    let profile_id = id;
    // The profile creation is its own line; this test is about events.
    fixture.state.clear_log();

    for event in [
        RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Starting,
        },
        RuntimeEvent::EffectiveLaunchArgs {
            profile_id,
            args: vec!["a".into(), "b".into(), "c".into()],
        },
        RuntimeEvent::Started {
            profile_id,
            browser_pid: 4242,
            xray_pid: Some(4343),
            cdp_port: 9222,
            socks_port: Some(1080),
        },
        RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Running,
        },
        RuntimeEvent::Warning {
            profile_id,
            message: "legacy core".into(),
        },
        RuntimeEvent::Stopped { profile_id },
        RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Stopped,
        },
    ] {
        fixture.state.record_event(&event);
    }

    let messages: Vec<(&str, String)> = fixture
        .state
        .log_entries()
        .iter()
        .map(|entry| (entry.level.label(en()), entry.message.clone()))
        .collect();
    assert_eq!(
        messages,
        [
            ("info", "starting".to_string()),
            ("info", "launching with 3 arguments".to_string()),
            (
                "info",
                "browser started (pid 4242, cdp port 9222, socks port 1080, xray pid 4343)"
                    .to_string()
            ),
            ("warning", "legacy core".to_string()),
            ("info", "browser stopped".to_string()),
        ],
        "running and stopped state changes are the events' own lines, not extra ones"
    );
    assert!(
        fixture
            .state
            .log_entries()
            .iter()
            .all(|entry| entry.profile_id == Some(profile_id))
    );
}

#[test]
fn a_crash_and_a_refused_start_are_logged_as_errors() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Primary").expect("create");
    // Creating the profile is its own (informational) line.
    fixture.state.clear_log();

    fixture.state.record_event(&RuntimeEvent::Crashed {
        profile_id: id,
        component: RuntimeComponent::Xray,
        message: "process exited unexpectedly: signal: 11".into(),
    });
    fixture.state.record_event(&RuntimeEvent::StateChanged {
        profile_id: id,
        state: RuntimeState::Failed {
            message: "no browser core".into(),
        },
    });

    let entries = fixture.state.log_entries();
    assert_eq!(entries[0].level, LogLevel::Error);
    assert_eq!(
        entries[0].message,
        "xray crashed: process exited unexpectedly: signal: 11"
    );
    assert_eq!(entries[1].level, LogLevel::Error);
    assert_eq!(entries[1].message, "failed: no browser core");
}

#[test]
fn a_failed_reading_is_logged_as_an_error() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);

    fixture
        .state
        .finish_verification(id, Err("no debug port".to_string()));

    let entry = fixture.state.log_entries().last().expect("a line");
    assert_eq!(entry.level, LogLevel::Error);
    assert!(entry.message.contains("no debug port"), "{}", entry.message);
}

#[test]
fn the_log_is_capped_and_the_newest_line_survives() {
    let mut fixture = fixture();

    for index in 0..LOG_CAPACITY + 100 {
        fixture.state.push_notice(format!("line {index}"), false);
    }

    let entries = fixture.state.log_entries();
    assert_eq!(entries.len(), LOG_CAPACITY);
    assert_eq!(entries[0].message, "line 100", "the oldest lines fell off");
    assert_eq!(
        entries[LOG_CAPACITY - 1].message,
        format!("line {}", LOG_CAPACITY + 99)
    );
}

#[test]
fn log_rows_name_the_profile_and_put_the_newest_line_first() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.clear_log();

    fixture
        .state
        .record_event(&RuntimeEvent::Stopped { profile_id: id });
    fixture.state.push_notice("Saved.", false);

    let rows = fixture.state.log_rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].who, "app", "the newest line is a window-level one");
    assert_eq!(rows[0].message, "Saved.");
    assert_eq!(rows[1].who, "verify me", "the profile is named, not its id");
    assert_eq!(rows[1].message, "browser stopped");

    fixture.state.clear_log();
    assert!(fixture.state.log_rows().is_empty());
    assert!(
        fixture.state.notice().is_none(),
        "clearing the history does not silence a current problem"
    );
}

#[test]
fn the_panel_log_tail_is_one_profile_and_newest_first() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.clear_log();
    let other = ProfileId::new();

    fixture.state.push_notice("app level", false);
    fixture
        .state
        .record_event(&RuntimeEvent::Stopped { profile_id: id });
    fixture.state.record_event(&RuntimeEvent::Warning {
        profile_id: other,
        message: "another profile".into(),
    });
    fixture.state.record_event(&RuntimeEvent::Warning {
        profile_id: id,
        message: "this profile".into(),
    });

    let tail = fixture.state.log_tail(id);
    assert_eq!(tail.len(), 2, "{tail:?}");
    assert_eq!(tail[0].message, "this profile", "newest first");
    assert_eq!(tail[0].who, "verify me", "and named");
    assert_eq!(tail[1].message, "browser stopped");
    assert!(
        tail.iter().all(|row| row.message != "app level"),
        "the panel shows this profile only; the Log page has everything"
    );
}

#[test]
fn the_details_panel_starts_on_details() {
    let mut fixture = fixture();
    assert_eq!(fixture.state.details_tab(), DetailsTab::Details);
    fixture.state.set_details_tab(DetailsTab::Args);
    assert_eq!(fixture.state.details_tab(), DetailsTab::Args);
}

#[test]
fn the_log_page_filter_hides_lines_without_losing_them() {
    let mut fixture = fixture();
    fixture.state.push_notice("Saved.", false);
    fixture.state.push_notice("careful", true);
    let id = running_profile(&mut fixture);
    fixture.state.record_event(&RuntimeEvent::Warning {
        profile_id: id,
        message: "legacy core".into(),
    });

    assert_eq!(fixture.state.log_filter(), LogFilter::All);
    assert_eq!(fixture.state.log_rows().len(), fixture.state.log_len());

    fixture.state.set_log_filter(LogFilter::Warnings);
    let warnings: Vec<&str> = fixture
        .state
        .log_rows()
        .iter()
        .map(|row| row.level.label(en()))
        .collect();
    assert!(!warnings.contains(&"info"), "{warnings:?}");
    assert!(
        fixture.state.log_len() > fixture.state.log_rows().len(),
        "the filter hides lines, it does not drop them"
    );

    fixture.state.set_log_filter(LogFilter::Errors);
    assert!(
        fixture
            .state
            .log_rows()
            .iter()
            .all(|row| row.level == LogLevel::Error),
        "the narrowest filter keeps only errors"
    );
    assert!(fixture.state.log_len() >= 4, "nothing was cleared");
}

#[test]
fn the_activity_log_is_written_to_the_file_as_well() {
    let dir = std::env::temp_dir().join(format!("fp-app-log-{}", CoreId::new()));
    let path = dir.join(crate::log_file::LOG_FILE);
    let mut fixture = fixture_with_log(&dir);
    let id = running_profile(&mut fixture);
    fixture.state.clear_log();

    fixture.state.record_event(&RuntimeEvent::Started {
        profile_id: id,
        browser_pid: 4242,
        xray_pid: None,
        cdp_port: 9222,
        socks_port: None,
    });
    fixture.state.push_notice("Saved.", false);
    fixture.state.push_notice("it broke", true);

    let text = std::fs::read_to_string(&path).expect("the log file was written");
    let lines: Vec<&str> = text.lines().collect();
    // The profile creation line is already in the file: `clear_log` empties
    // the window's history, not what was written down.
    assert_eq!(lines.len(), 4, "{text}");
    assert!(lines[0].contains("app: Created verify me"), "{}", lines[0]);
    assert!(
        lines[1].contains("info")
            && lines[1].contains("verify me")
            && lines[1].contains("browser started (pid 4242, cdp port 9222)"),
        "a line names the profile and what happened: {}",
        lines[1]
    );
    assert!(
        lines[2].contains("info") && lines[2].ends_with("app: Saved."),
        "{}",
        lines[2]
    );
    assert!(
        lines[3].contains("error") && lines[3].ends_with("app: it broke"),
        "{}",
        lines[3]
    );
    assert!(
        lines
            .iter()
            .all(|line| line.starts_with("20") && line.contains('T')),
        "every line carries an absolute UTC time: {text}"
    );
    assert_eq!(
        fixture.state.log_file_status().ok(),
        Some(path.as_path()),
        "the page can say where the file is"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The window must keep working when the log cannot be written, and the
/// failure must be said once rather than per line.
#[test]
fn a_log_file_that_cannot_be_written_is_reported_once() {
    let dir = std::env::temp_dir().join(format!("fp-app-log-bad-{}", CoreId::new()));
    let mut fixture = fixture_full(
        &dir.join("config.json"),
        None,
        Some(crate::log_file::LogFile::at(
            dir.join("missing").join("activity.log"),
        )),
        None,
    );

    fixture.state.push_notice("first", false);
    fixture.state.push_notice("second", false);
    fixture.state.push_notice("third", true);

    let error = fixture
        .state
        .log_file_status()
        .expect_err("the sink failed");
    assert!(error.contains("could not open"), "{error}");
    assert_eq!(
        fixture
            .state
            .toasts()
            .iter()
            .filter(|toast| toast.message.contains("activity log could not be written"))
            .count(),
        1,
        "the failure is reported once, not once per line"
    );
    assert_eq!(
        fixture.state.log_len(),
        3,
        "the in-memory log is unaffected"
    );
    let _ = std::fs::remove_dir_all(&dir);
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

/// One configuration task at a time, and the marker is what makes it so.
///
/// The four of them read and write the same rows and the same file, and two
/// clicks arriving before the first answer would otherwise interleave an
/// import with the restore that is replacing everything it is importing into.
#[test]
fn a_second_configuration_task_is_refused_while_one_is_running() {
    let scratch = Scratch::new("one-at-a-time");
    let mut fixture = fixture();
    fixture.state.set_export_path(
        scratch
            .dir
            .join("config.json")
            .to_string_lossy()
            .to_string(),
    );

    // A path, so the refusal below is about the task in flight rather than
    // about an empty field.
    fixture.state.set_import_path(
        scratch
            .dir
            .join("elsewhere.json")
            .to_string_lossy()
            .to_string(),
    );
    let job = fixture.state.begin_export().expect("the first task");
    // No answer has come back yet, which is exactly when a second click
    // happens.
    let refused = fixture
        .state
        .begin_import()
        .err()
        .expect("one configuration task at a time");
    assert!(refused.contains("still running"), "{refused}");
    let notice = fixture.state.notice().expect("a notice");
    assert!(notice.error, "{notice:?}");
    assert_eq!(notice.message, refused);

    // The answer lets the next one in - a failure as much as a success, or the
    // window would refuse every task after the first that went wrong.
    let destination = job.destination.clone();
    let result = maintenance::run_export(job);
    fixture
        .state
        .finish_maintenance(maintenance::Outcome::Exported {
            destination,
            result,
        });
    fixture.state.begin_export().expect("the marker was let go");
}

#[test]
fn an_export_leaves_the_passwords_out_unless_asked_otherwise() {
    let scratch = Scratch::new("default-choice");
    let mut fixture = fixture();
    let core = seed_core(&fixture);
    let proxy = seed_proxy_holding_a_password(&mut fixture);
    fixture.state.load().expect("load");
    let profile = fixture.state.create_profile("Work laptop").expect("create");
    let mut draft = fixture.state.profile(profile).expect("the profile");
    draft.proxy_id = Some(proxy);
    draft.core_id = core;
    fixture
        .state
        .update_profile(draft)
        .expect("assign the proxy");

    let path = scratch.join("config.json");
    fixture
        .state
        .set_export_path(path.to_string_lossy().to_string());

    // The default is the safe one: a file carrying plain-text passwords is
    // the thing to reach for on purpose, not the thing to get by not reading
    // a checkbox.
    assert!(!fixture.state.export_includes_credentials());
    let report = fixture.state.export_configuration().expect("export");

    assert_eq!(report.profiles, 1);
    assert_eq!(report.credentials, Credentials::Excluded);
    assert_eq!(report.credentials_removed, 1);

    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(!text.contains(EXPORT_SECRET), "{text}");
    // And the profile's references still point at what travelled with it.
    let document = application::ConfigBackup::from_json(&text).expect("parse");
    assert_eq!(document.profiles[0].core_id, document.cores[0].id);
    assert_eq!(document.profiles[0].proxy_id, Some(document.proxies[0].id));
}

#[test]
fn asking_for_the_credentials_writes_them_and_says_so() {
    let scratch = Scratch::new("asked-for");
    let mut fixture = fixture();
    seed_core(&fixture);
    seed_proxy_holding_a_password(&mut fixture);
    fixture.state.load().expect("load");

    let path = scratch.join("config.json");
    fixture
        .state
        .set_export_path(path.to_string_lossy().to_string());
    fixture.state.set_export_includes_credentials(true);
    let report = fixture.state.export_configuration().expect("export");

    assert_eq!(report.credentials, Credentials::Included);
    assert_eq!(report.credentials_removed, 0);
    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(text.contains(EXPORT_SECRET));

    // The warning is part of the answer, not decoration: a file holding
    // passwords in plain text has to say so where the user is looking.
    let message = last_message(&fixture);
    assert!(message.contains("plain text"), "{message}");
    assert!(message.contains("config.json"), "{message}");
}

#[test]
fn an_export_with_nothing_to_leave_out_says_that_instead() {
    // The opposite sentence, and the reason the count is reported: "left
    // out" over a configuration that had nothing to leave out would describe
    // a file that is missing something it is not.
    let scratch = Scratch::new("nothing-to-leave-out");
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");

    let path = scratch.join("config.json");
    fixture
        .state
        .set_export_path(path.to_string_lossy().to_string());
    let report = fixture.state.export_configuration().expect("export");

    assert_eq!(report.credentials, Credentials::Excluded);
    assert_eq!(report.credentials_removed, 0);
    assert_eq!(report.proxies, 0);

    let message = last_message(&fixture);
    assert!(
        message.contains("no proxy credentials to leave out"),
        "{message}"
    );
}

#[test]
fn a_failed_export_is_reported_as_an_error_and_writes_nothing() {
    let scratch = Scratch::new("failed");
    let blocker = scratch.join("not-a-directory");
    std::fs::write(&blocker, b"").expect("write the blocker");
    let mut fixture = fixture();
    fixture.state.load().expect("load");

    fixture
        .state
        .set_export_path(blocker.join("config.json").to_string_lossy().to_string());
    let error = fixture
        .state
        .export_configuration()
        .expect_err("cannot write");

    assert!(error.contains("could not be written"), "{error}");
    assert!(error.contains("not-a-directory"), "{error}");
    let notice = fixture.state.notice().expect("a banner");
    assert!(notice.error, "{}", notice.message);
}

#[test]
fn an_empty_export_field_means_the_default_and_a_typed_one_means_itself() {
    let mut fixture = fixture();

    // Empty, and whitespace, both mean "the default": a field someone has
    // cleared is not a request to write a file called nothing.
    assert_eq!(
        fixture.state.export_destination(),
        fixture.state.export_default_path()
    );
    fixture.state.set_export_path("   ");
    assert_eq!(
        fixture.state.export_destination(),
        fixture.state.export_default_path()
    );

    fixture.state.set_export_path("  /tmp/my-backup.json  ");
    assert_eq!(
        fixture.state.export_destination(),
        PathBuf::from("/tmp/my-backup.json")
    );
}

#[test]
fn the_default_export_lands_under_the_data_directory_and_names_a_new_file() {
    // Nothing is written here - the default is somewhere the machine keeps
    // real data, and this test only reads what it would be.
    let fixture = fixture();
    let data_dir = fixture.state.export_default_path();
    let parent = data_dir.parent().expect("a parent").to_path_buf();

    assert_eq!(
        parent.file_name().unwrap(),
        crate::paths::EXPORT_DIR,
        "{}",
        parent.display()
    );
    assert!(
        data_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("fp-browser-config-"),
        "{}",
        data_dir.display()
    );
    assert_eq!(
        data_dir.extension().unwrap(),
        "json",
        "{}",
        data_dir.display()
    );
}

#[test]
fn the_export_path_field_survives_a_trip_to_another_page() {
    // The field itself lives in the view, which is not what is tested here;
    // what is testable without a window is that the state behind it keeps
    // what was typed rather than forgetting it between renders.
    let mut fixture = fixture();
    fixture.state.set_export_path("/tmp/kept.json");
    fixture.state.set_page(crate::state::Page::Proxies);
    assert_eq!(fixture.state.export_path(), "/tmp/kept.json");
    fixture.state.set_page(crate::state::Page::Settings);
    assert_eq!(fixture.state.export_path(), "/tmp/kept.json");
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

#[test]
fn an_export_imports_back_into_an_empty_installation() {
    let scratch = Scratch::new("import-round-trip");
    let mut source = fixture();
    let core = seed_core(&source);
    let proxy = seed_proxy_holding_a_password(&mut source);
    let profile = source.state.create_profile("Work laptop").expect("create");
    let mut draft = source.state.profile(profile).expect("the profile");
    draft.proxy_id = Some(proxy);
    draft.core_id = core;
    source
        .state
        .update_profile(draft)
        .expect("assign the proxy");

    let backup = scratch.join("config.json");
    write_backup_from(&mut source, &backup);

    let data_dir = scratch.join("data");
    let mut arriving = fixture_with_data_dir(&data_dir);
    arriving
        .state
        .set_import_path(backup.to_string_lossy().to_string());
    let report = arriving.state.import_configuration().expect("import");

    assert_eq!(report.added.cores, 1);
    assert_eq!(report.added.proxies, 1);
    assert_eq!(report.added.profiles, 1);
    assert!(!report.needs_attention(), "{:?}", report.notes);

    // The message names the file and says what arrived.
    let message = last_message(&arriving);
    assert!(message.contains("config.json"), "{message}");
    assert!(message.contains("were added"), "{message}");

    // The rows are reloaded, so the page shows what just arrived.
    assert_eq!(arriving.state.rows().len(), 1);

    // The profile's directory moved to this machine's data directory:
    // the one the file recorded is not here, and a directory that is not
    // on this machine was never going to be right.
    let stored = arriving
        .profiles
        .get(profile)
        .expect("stored")
        .expect("the profile arrived");
    assert_eq!(
        stored.user_data_dir,
        data_dir.join("profiles").join(profile.to_string())
    );

    // And the proxy arrived without the password the export left out.
    let stored_proxy = arriving
        .proxies
        .get(proxy)
        .expect("stored")
        .expect("the proxy arrived");
    match &stored_proxy.outbound {
        ProxyOutbound::Socks5(socks5) => {
            assert_eq!(socks5.password, None, "{:?}", stored_proxy.outbound);
        }
        other => panic!("expected the socks5 proxy back, got {other:?}"),
    }
}

#[test]
fn importing_the_same_file_twice_adds_nothing_the_second_time() {
    // The ordinary result of importing a file twice, and the reason the
    // report distinguishes "nothing was added" from a refusal: the second
    // import succeeded, it just had nothing to do.
    let scratch = Scratch::new("import-twice");
    let mut source = fixture();
    seed_core(&source);
    source.state.load().expect("load");
    source.state.create_profile("Work laptop").expect("create");

    let backup = scratch.join("config.json");
    write_backup_from(&mut source, &backup);

    let mut arriving = fixture();
    arriving
        .state
        .set_import_path(backup.to_string_lossy().to_string());
    let first = arriving.state.import_configuration().expect("first import");
    assert_eq!(first.added.total(), 2);

    let second = arriving
        .state
        .import_configuration()
        .expect("second import");
    assert_eq!(second.added.total(), 0);
    assert!(!second.needs_attention(), "{:?}", second.notes);
    let message = last_message(&arriving);
    assert!(message.contains("nothing was added"), "{message}");
    assert_eq!(arriving.state.rows().len(), 1);
}

#[test]
fn an_import_whose_core_is_not_in_the_file_skips_its_profiles() {
    // A hand-edited file: the export always pairs a profile with its core,
    // but a file is a file a person can edit, and the rule has to hold
    // when the pairing is broken.
    let scratch = Scratch::new("import-missing-core");
    let mut source = fixture();
    seed_core(&source);
    source.state.load().expect("load");
    source.state.create_profile("Work laptop").expect("create");

    let backup = scratch.join("config.json");
    write_backup_from(&mut source, &backup);

    let mut document = application::ConfigBackup::from_json(
        &std::fs::read_to_string(&backup).expect("read the backup"),
    )
    .expect("parse");
    document.cores.clear();
    let edited = scratch.join("edited.json");
    std::fs::write(&edited, document.to_json().expect("serialise")).expect("write");

    let mut arriving = fixture();
    arriving
        .state
        .set_import_path(edited.to_string_lossy().to_string());
    let report = arriving.state.import_configuration().expect("import");

    assert_eq!(report.added.profiles, 0);
    assert_eq!(report.notes.missing_core, vec!["Work laptop".to_string()]);
    assert!(report.needs_attention());
    // A partial import is a problem the reader has to dismiss, not a
    // toast that drifts away: what was skipped is the thing they came to
    // find out.
    assert!(
        arriving.state.notice().is_some_and(|notice| notice.error),
        "the shortfall is in the banner"
    );
    assert_eq!(arriving.state.rows().len(), 0);
}

#[test]
fn an_import_without_a_path_is_refused_where_the_field_is() {
    let mut arriving = fixture();
    let result = arriving.state.import_configuration();

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Type the path"));
    assert!(arriving.state.notice().is_some_and(|notice| notice.error));
}

#[test]
fn a_file_that_is_not_a_backup_is_said_so() {
    let scratch = Scratch::new("import-foreign");
    let elsewhere = scratch.join("notes.txt");
    std::fs::write(&elsewhere, "not a configuration backup").expect("write");

    let mut arriving = fixture();
    arriving
        .state
        .set_import_path(elsewhere.to_string_lossy().to_string());
    let result = arriving.state.import_configuration();

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("could not be read"));
    assert_eq!(arriving.state.rows().len(), 0);
}

#[test]
fn is_configuration_empty_says_what_is_here() {
    let mut fixture = fixture();
    assert!(fixture.state.is_configuration_empty().expect("read"));

    seed_core(&fixture);
    fixture.state.load().expect("load");
    fixture.state.create_profile("Work laptop").expect("create");

    assert!(!fixture.state.is_configuration_empty().expect("read"));
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

#[test]
fn a_restore_onto_an_empty_installation_reads_the_file() {
    let scratch = Scratch::new("restore-empty");
    let backup = backup_with_one_profile(&scratch);

    let data_dir = scratch.join("data");
    let mut arriving = fixture_with_data_dir(&data_dir);
    arriving
        .state
        .set_restore_path(backup.to_string_lossy().to_string());
    let report = arriving
        .state
        .restore_configuration(RestoreMode::OnlyWhenEmpty)
        .expect("restore");

    assert_eq!(report.added.cores, 1);
    assert_eq!(report.added.profiles, 1);
    assert_eq!(report.removed.total(), 0, "there was nothing to replace");
    assert!(
        arriving.state.notice().is_none(),
        "a clean restore is a toast, not a banner"
    );
    let message = last_message(&arriving);
    assert!(message.contains("were added"), "{message}");
    assert_eq!(arriving.state.rows().len(), 1);
}

/// The precondition, and the whole difference from an import: a populated
/// installation is never replaced without being asked.
#[test]
fn a_restore_without_confirmation_refuses_a_populated_installation() {
    let scratch = Scratch::new("restore-refuses");
    let backup = backup_with_one_profile(&scratch);

    let mut arriving = fixture();
    seed_core(&arriving);
    arriving.state.load().expect("load");
    let kept = arriving.state.create_profile("Keep me").expect("create");
    arriving
        .state
        .set_restore_path(backup.to_string_lossy().to_string());

    let error = arriving
        .state
        .restore_configuration(RestoreMode::OnlyWhenEmpty)
        .expect_err("a populated installation cannot be quietly replaced");

    assert!(error.contains("already holds"), "{error}");
    assert!(
        arriving.state.notice().is_some_and(|notice| notice.error),
        "the refusal owns the banner"
    );
    assert_eq!(arriving.state.rows().len(), 1);
    assert_eq!(arriving.state.rows()[0].profile.name, "Keep me");
    assert!(
        arriving.state.profile(kept).is_some(),
        "nothing was removed"
    );
}

#[test]
fn a_confirmed_restore_replaces_what_is_here() {
    let scratch = Scratch::new("restore-replaces");
    let backup = backup_with_one_profile(&scratch);

    let mut arriving = fixture();
    seed_core(&arriving);
    arriving.state.load().expect("load");
    arriving.state.create_profile("Replace me").expect("create");
    arriving
        .state
        .set_restore_path(backup.to_string_lossy().to_string());

    let report = arriving
        .state
        .restore_configuration(RestoreMode::Replace)
        .expect("restore");

    assert_eq!(
        report.removed.total(),
        2,
        "the core and profile that were here"
    );
    assert_eq!(report.added.total(), 2);
    assert_eq!(arriving.state.rows().len(), 1);
    assert_eq!(arriving.state.rows()[0].profile.name, "Work laptop");
}

#[test]
fn a_restore_without_a_path_is_refused_where_the_field_is() {
    let mut arriving = fixture();
    let result = arriving
        .state
        .restore_configuration(RestoreMode::OnlyWhenEmpty);

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Type the path"));
    assert!(arriving.state.notice().is_some_and(|notice| notice.error));
}

/// Restoring would delete the row of a live browser, leaving a process the
/// window can no longer stop; the refusal names what to stop.
#[test]
fn a_restore_is_blocked_while_a_profile_is_running() {
    let scratch = Scratch::new("restore-running");
    let backup = backup_with_one_profile(&scratch);

    let mut arriving = fixture();
    running_profile(&mut arriving);
    arriving
        .state
        .set_restore_path(backup.to_string_lossy().to_string());

    let error = arriving
        .state
        .restore_configuration(RestoreMode::Replace)
        .expect_err("a running profile blocks the restore");

    assert!(error.contains("Stop these profiles"), "{error}");
    assert!(error.contains("verify me"), "{error}");
    assert_eq!(arriving.state.rows().len(), 1, "nothing was replaced");
}

/// The restore and import fields are separate: a path left over from one verb
/// must not become a path the other acts on.
#[test]
fn the_restore_and_import_paths_are_separate_fields() {
    let mut fixture = fixture();
    fixture.state.set_import_path("/tmp/import.json");
    fixture.state.set_restore_path("/tmp/restore.json");

    assert_eq!(
        fixture.state.import_source(),
        Some(PathBuf::from("/tmp/import.json"))
    );
    assert_eq!(
        fixture.state.restore_source(),
        Some(PathBuf::from("/tmp/restore.json"))
    );
}

#[test]
fn a_browser_data_job_needs_a_directory_and_gathers_every_profile() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    fixture.state.create_profile("Work laptop").expect("create");

    assert!(
        fixture.state.browser_data_job(Direction::ToBackup).is_err(),
        "a copy needs somewhere to go"
    );

    fixture.state.set_browser_data_path("  /backups/fp  ");
    let (job, lease) = fixture
        .state
        .browser_data_job(Direction::ToBackup)
        .expect("a job");
    assert_eq!(job.directory, PathBuf::from("/backups/fp"), "trimmed");
    assert_eq!(job.profiles.len(), 1);
    assert!(job.running.is_empty());
    assert_eq!(job.direction, Direction::ToBackup);
    drop(lease);
}

#[test]
fn a_browser_data_job_is_refused_while_a_profile_is_running() {
    let mut fixture = fixture();
    running_profile(&mut fixture);
    fixture.state.set_browser_data_path("/backups/fp");

    let error = fixture
        .state
        .browser_data_job(Direction::ToBackup)
        .expect_err("a running profile blocks the copy");

    assert!(error.contains("Stop these profiles"), "{error}");
    assert!(error.contains("verify me"), "{error}");
}

#[test]
fn a_browser_data_job_with_no_profiles_is_refused() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    fixture.state.set_browser_data_path("/backups/fp");

    let error = fixture
        .state
        .browser_data_job(Direction::FromBackup)
        .expect_err("nothing to copy");

    assert!(error.contains("no profiles"), "{error}");
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

#[test]
fn an_old_proxy_result_cannot_release_a_new_start_gate() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work");
    let old = fixture.state.begin_proxy_test(proxy_id).unwrap();
    let mut proxy = fixture.state.proxy(proxy_id).unwrap();
    proxy.name = "Edited".into();
    fixture.state.update_proxy(proxy).unwrap();
    let StartGate::Checking(current) = fixture.state.begin_opening(id, Opening::Start).unwrap()
    else {
        panic!("new check")
    };
    fixture.state.complete_proxy_test(&old, through_the_proxy());
    assert!(commands(&fixture).is_empty());
    assert!(fixture.state.proxy_test(proxy_id).unwrap().is_running());
    fixture
        .state
        .complete_proxy_test(&current, through_the_proxy());
    assert_eq!(commands(&fixture), [format!("start:{id}")]);
    fixture
        .state
        .complete_proxy_test(&old, Err(Fault::new(FaultClass::Timeout, "late")));
    assert!(
        fixture
            .state
            .proxy_test(proxy_id)
            .unwrap()
            .exit_ip()
            .is_some()
    );
}

/// A profile whose traffic leaves through a proxy is only as usable as that
/// proxy: the command waits until a request has come back through it.
///
/// The window is the whole reason the gate is where it is. `start` returns as
/// soon as its command is queued, so a launch that went ahead first and asked
/// afterwards would produce a browser that is already running by the time
/// anybody knows the proxy is down - which is a browser whose traffic leaks,
/// or one that shows nothing but network errors.
#[test]
fn opening_a_proxied_profile_asks_its_proxy_first() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work laptop");

    let gate = fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("the gate");
    let StartGate::Checking(job) = gate else {
        panic!("a proxied profile is checked first: {gate:?}");
    };
    assert_eq!(job.proxy_id, proxy_id);
    assert_eq!(job.proxy.name, "Office");
    assert!(
        job.live_port.is_none(),
        "nothing is up for this proxy, so the check is a rehearsal"
    );
    assert_eq!(
        fixture.state.proxy_test(proxy_id),
        Some(&ProxyTest::Running),
        "the proxy row shows the check that is in flight"
    );
    assert!(
        commands(&fixture).is_empty(),
        "no command reaches the runtime before the answer"
    );
    assert_eq!(
        fixture.state.row(id).expect("the row").state(),
        RuntimeState::Starting,
        "a profile waiting for its proxy is on its way, not stopped"
    );

    // The answer is what queues it, and the sentence says both halves.
    fixture
        .state
        .finish_proxy_test(proxy_id, false, through_the_proxy());
    assert_eq!(commands(&fixture), [format!("start:{id}")]);
    let message = last_message(&fixture);
    assert!(
        message.contains("Started Work laptop through Office"),
        "{message}"
    );
    assert!(message.contains("198.51.100.9"), "{message}");
    assert_eq!(
        fixture.state.row(id).expect("the row").state(),
        RuntimeState::Running
    );
    assert_eq!(
        fixture.state.operations.held(id),
        None,
        "the snapshot answered, so the start's lease is done"
    );
}

/// A proxy that carries nothing refuses the start, names both ends, and gives
/// the profile back so the next attempt is possible.
#[test]
fn opening_a_proxied_profile_is_refused_when_its_proxy_has_no_traffic() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work laptop");
    fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("the gate");

    fixture.state.finish_proxy_test(
        proxy_id,
        false,
        Err(Fault::new(FaultClass::Unreachable, "no route to host")),
    );

    assert!(
        commands(&fixture).is_empty(),
        "a refused start queues nothing"
    );
    assert_eq!(
        fixture.state.row(id).expect("the row").state(),
        RuntimeState::Stopped,
        "the profile is stopped again, not left reading as starting"
    );
    let message = last_message(&fixture);
    assert!(message.contains("Work laptop"), "{message}");
    assert!(message.contains("Office"), "{message}");
    assert!(message.contains("no route to host"), "{message}");
    assert_eq!(
        fixture.state.operations.held(id),
        None,
        "a proxy that is down now may be up in a minute: the profile is free"
    );

    // Which is what the next press of Start needs.
    let again = fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("a second attempt");
    assert!(matches!(again, StartGate::Checking(_)));
}

#[test]
fn opening_a_profile_without_a_proxy_asks_nothing() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Plain").expect("create");

    let gate = fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("the gate");

    assert!(matches!(gate, StartGate::Queued));
    assert_eq!(commands(&fixture), [format!("start:{id}")]);
    assert_eq!(
        fixture.state.row(id).expect("the row").state(),
        RuntimeState::Running
    );
}

/// A test of this proxy is already in flight - the Proxies page's own button.
/// Its answer is the answer the start needs, so the start waits for it rather
/// than refusing and making the reader press Start again.
#[test]
fn a_start_joins_a_proxy_test_that_is_already_in_flight() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work laptop");
    fixture
        .state
        .begin_proxy_test(proxy_id)
        .expect("the row's own test");

    let gate = fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("the gate");
    assert!(matches!(gate, StartGate::Awaiting));
    assert!(commands(&fixture).is_empty());

    fixture
        .state
        .finish_proxy_test(proxy_id, false, through_the_proxy());
    assert_eq!(commands(&fixture), [format!("start:{id}")]);
}

/// A restart goes through the same gate: it is the same browser on the same
/// proxy.
#[test]
fn restarting_a_proxied_profile_asks_its_proxy_too() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work laptop");

    let gate = fixture
        .state
        .begin_opening(id, Opening::Restart)
        .expect("the gate");
    assert!(matches!(gate, StartGate::Checking(_)));
    assert!(commands(&fixture).is_empty());

    fixture
        .state
        .finish_proxy_test(proxy_id, false, through_the_proxy());
    assert_eq!(commands(&fixture), [format!("restart:{id}")]);
}

/// A stop pressed while the proxy is still being asked calls the start off.
/// The command would reach a runtime with nothing to stop, and the answer to
/// the check would then start the profile the reader just called off.
#[test]
fn stopping_calls_off_a_start_that_is_waiting_for_its_proxy() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work laptop");
    fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("the gate");

    fixture.state.stop(id).expect("stop");

    assert_eq!(
        fixture.state.row(id).expect("the row").state(),
        RuntimeState::Stopped
    );
    assert!(
        commands(&fixture).is_empty(),
        "nothing was started, and there was nothing to stop"
    );

    // The answer arrives late and starts nothing.
    fixture
        .state
        .finish_proxy_test(proxy_id, false, through_the_proxy());
    assert!(commands(&fixture).is_empty());
    assert_eq!(fixture.state.operations.held(id), None);
}

/// Every wait has to end somewhere. The answer resolves it, a stop calls it
/// off, and this is the third way out: the proxy is edited while the check is
/// in flight, so the answer - when it comes - is a claim about an upstream
/// that no longer exists and is dropped instead of matched to this start.
#[test]
fn editing_a_proxy_calls_off_the_start_that_was_waiting_for_it() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work laptop");
    fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("the gate");

    let mut proxy = fixture.state.proxy(proxy_id).expect("the proxy");
    proxy.outbound = socks5("10.0.0.2", 1080);
    fixture.state.update_proxy(proxy).expect("edit");

    assert_eq!(
        fixture.state.row(id).expect("the row").state(),
        RuntimeState::Stopped
    );
    assert_eq!(
        fixture.state.operations.held(id),
        None,
        "a start nobody will answer must not hold its profile"
    );
    let message = last_message(&fixture);
    assert!(message.contains("Work laptop"), "{message}");
    assert!(
        message.contains("changed while it was being checked"),
        "{message}"
    );

    // The late answer is dropped with the reading it was for, so nothing
    // starts and nothing is left over.
    fixture
        .state
        .finish_proxy_test(proxy_id, false, through_the_proxy());
    assert!(commands(&fixture).is_empty());
}

/// And the fourth: a worker that never reports at all. The wait is bounded
/// rather than kept until the window closes, because a lease that outlives
/// the thing that took it is a profile that can never be started again.
#[test]
fn a_check_that_never_answers_gives_its_profile_back() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work laptop");
    fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("the gate");
    // The check is as old as the bound allows, which is what it looks like
    // from here when the worker died.
    fixture.state.pending_starts.insert(
        id,
        PendingStart {
            proxy: proxy_id,
            how: Opening::Start,
            asked: Instant::now()
                .checked_sub(CHECK_LEASE + Duration::from_secs(1))
                .expect("the clock goes back"),
        },
    );

    fixture.state.refresh_runtime();

    assert_eq!(
        fixture.state.row(id).expect("the row").state(),
        RuntimeState::Stopped
    );
    assert_eq!(fixture.state.operations.held(id), None);
    assert_eq!(
        fixture.state.proxy_test(proxy_id),
        Some(&ProxyTest::Running),
        "the reading is still the row's business, not the start's"
    );
    let message = last_message(&fixture);
    assert!(message.contains("did not answer in time"), "{message}");

    // And the profile can be started again, which is the whole point. The
    // reading is still in flight, so the second attempt waits for it rather
    // than asking a second time - what matters is that it is not refused.
    let again = fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("a second attempt");
    assert!(matches!(again, StartGate::Awaiting));
}

/// While the check runs the profile is busy, which is the lease doing its
/// job: a copy of its browser data would read a directory the browser is
/// about to be given.
#[test]
fn a_profile_waiting_for_its_proxy_is_busy() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work laptop");
    fixture.state.set_browser_data_path("/backups/fp");
    fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("the gate");

    let error = fixture
        .state
        .browser_data_job(Direction::ToBackup)
        .expect_err("the start is already under way");

    assert!(error.contains("Work laptop"), "{error}");
    assert_eq!(
        fixture.state.operations.held(id),
        Some(Operation::Starting),
        "the lease is what the refusal is made of"
    );

    // And a second press of Start says so rather than asking twice.
    let error = fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect_err("already under way");
    assert!(error.to_string().contains("Work laptop"), "{error}");

    let _ = proxy_id;
}

#[test]
fn a_copy_holds_its_profiles_until_the_worker_gives_them_back() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Work laptop").expect("create");
    fixture.state.set_browser_data_path("/backups/fp");

    let (job, lease) = fixture
        .state
        .browser_data_job(Direction::ToBackup)
        .expect("the first copy");
    assert_eq!(job.profiles.len(), 1);

    let error = fixture
        .state
        .browser_data_job(Direction::FromBackup)
        .expect_err("a second copy is refused while the first runs");
    assert!(error.contains("Work laptop"), "{error}");
    assert!(error.contains("copied"), "{error}");

    let error = fixture
        .state
        .start(id)
        .expect_err("starting one of the profiles is refused");
    assert!(error.to_string().contains("Work laptop"), "{error}");

    // A replacement reads its file first, so the path has to be there for the
    // refusal to be about the busy profile rather than about the field.
    fixture.state.set_restore_path("/backups/fp/config.json");
    let error = fixture
        .state
        .restore_configuration(RestoreMode::Replace)
        .expect_err("replacing the configuration is refused");
    assert!(error.contains("Work laptop"), "{error}");

    // The worker finishes: everything it held is free again.
    drop(lease);
    let (job, lease) = fixture
        .state
        .browser_data_job(Direction::FromBackup)
        .expect("the copy after it finishes");
    assert_eq!(job.profiles.len(), 1);
    drop(lease);
}

/// A start holds its profile from the command until the runtime has answered
/// for it. A stopped snapshot is the window between the two, and a copy is
/// refused for its whole width - which the snapshot alone could not do, since
/// it is stopped for the whole of it.
#[test]
fn a_queued_start_holds_its_profile_until_the_snapshot_answers() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Work laptop").expect("create");
    fixture.state.set_browser_data_path("/backups/fp");

    // The command is queued and nothing has answered: the profile reads as
    // stopped, which is exactly the window the lease is for.
    fixture.runtime.answer_nothing();
    fixture.state.start(id).expect("the command is queued");
    assert_eq!(
        fixture.state.operations.held(id),
        Some(Operation::Starting),
        "a start that has not been answered holds its profile"
    );

    let error = fixture
        .state
        .browser_data_job(Direction::ToBackup)
        .expect_err("the queued start holds the profile");
    assert!(error.contains("starting up"), "{error}");

    // The runtime answers. The snapshot refuses a copy from here on, and the
    // lease is gone - which is what lets the profile be restarted at all.
    fixture.runtime.set_state(id, RuntimeState::Running);
    fixture.state.refresh_runtime();
    assert_eq!(
        fixture.state.operations.held(id),
        None,
        "an answered start gives its profile back"
    );
    fixture
        .state
        .restart(id)
        .expect("a restart is not refused by the start before it");
}

#[test]
fn retrying_a_failed_start_keeps_its_lease_until_its_own_answer() {
    for old in [
        RuntimeState::Failed {
            message: "old failure".into(),
        },
        RuntimeState::Crashed {
            message: "old crash".into(),
        },
    ] {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().unwrap();
        let id = fixture.state.create_profile("Retry").unwrap();
        fixture.runtime.set_state(id, old);
        fixture.runtime.answer_nothing();
        fixture.state.start(id).unwrap();
        fixture.state.refresh_runtime();
        fixture.state.set_browser_data_path("/backups/fp");
        assert_eq!(fixture.state.operations.held(id), Some(Operation::Starting));
        assert!(fixture.state.browser_data_job(Direction::ToBackup).is_err());
        // Even a failed retry ends its lease, once it is this request's answer.
        fixture.runtime.set_state(
            id,
            RuntimeState::Failed {
                message: "new failure".into(),
            },
        );
        fixture.state.refresh_runtime();
        assert_eq!(fixture.state.operations.held(id), None);
        assert!(fixture.state.browser_data_job(Direction::ToBackup).is_ok());
    }
}

#[test]
fn finishing_a_browser_data_copy_says_what_happened() {
    let mut fixture = fixture();
    fixture.state.finish_browser_data(
        Direction::ToBackup,
        Ok(BrowserDataReport {
            directory: PathBuf::from("/backups/fp"),
            copied: vec!["Work laptop".to_string()],
            skipped: vec!["Fresh".to_string()],
            bytes: 2 * 1024 * 1024,
        }),
    );

    let message = last_message(&fixture);
    assert!(
        message.contains("Copied the browser data of 1 profile"),
        "{message}"
    );
    assert!(message.contains("/backups/fp"), "{message}");
    assert!(message.contains("2.0 MiB"), "{message}");
    assert!(
        message.contains("Fresh"),
        "a skipped profile is named: {message}"
    );
}

#[test]
fn a_failed_browser_data_copy_is_a_banner() {
    let mut fixture = fixture();
    fixture.state.finish_browser_data(
        Direction::FromBackup,
        Err("Stop these profiles first.".to_string()),
    );

    let notice = fixture.state.notice().expect("a banner");
    assert!(notice.error, "{}", notice.message);
    assert!(
        notice.message.contains("Stop these profiles"),
        "{}",
        notice.message
    );
}

/// The two directions are empty for opposite reasons, and the sentence has to
/// say which one happened: blaming the backup directory for profiles that
/// have never been started would send the reader to the wrong place.
#[test]
fn an_empty_browser_data_copy_blames_the_end_that_was_empty() {
    let empty = BrowserDataReport {
        directory: PathBuf::from("/backups/fp"),
        copied: Vec::new(),
        skipped: vec!["Fresh".to_string()],
        bytes: 0,
    };

    let out = browser_data_summary(Direction::ToBackup, &empty, en());
    assert!(out.contains("no profile has browser data yet"), "{out}");
    assert!(
        !out.contains("/backups/fp"),
        "the source is at fault: {out}"
    );

    let back = browser_data_summary(Direction::FromBackup, &empty, en());
    assert!(back.contains("/backups/fp"), "{back}");
    assert!(back.contains("holds no browser data"), "{back}");
}

/// A file written without credentials restores proxies that no longer carry
/// them. An import already says so; a restore that stayed silent would leave
/// the reader with a proxy that fails authentication and no explanation.
#[test]
fn a_restore_from_a_credential_free_file_says_a_proxy_may_need_them_again() {
    let mut report = RestoreReport::default();
    report.added.proxies = 1;
    report.notes.credentials_excluded = true;

    let sentence = restore_summary(&report, std::path::Path::new("/tmp/config.json"), en());

    assert!(sentence.contains("were added"), "{sentence}");
    assert!(sentence.contains("without proxy credentials"), "{sentence}");
    assert!(
        sentence.contains("may need them typed in again"),
        "{sentence}"
    );
}
