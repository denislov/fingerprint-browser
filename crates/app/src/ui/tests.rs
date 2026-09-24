use crate::text::en;

use super::AppView;
use crate::browser_data::testing::FakeBrowserDataCopier;
use crate::exit::ExitMode;
use crate::open_dir::testing::FakeOpener;
use crate::proxy_tester::testing::FakeProxyTester;
use crate::state::AppState;
use crate::state::Verification;
use crate::state::testing::{FakeRuntime, core};
use crate::theme::{Palette, ThemeChoice, palette};
use crate::verifier::testing::FakeVerifier;
use application::Direction;
use application::{DefaultProfileService, DefaultProxyService, ProxyService, RuntimeService};
use domain::{CoreId, ProfileId, ProxyId, RuntimeState};
use gpui_kit::component::Root;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::theme::Theme;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext, px, size};
use runtime::{Discrepancy, Fault, FaultClass, RuntimeEvent};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use storage::{CoreRepository as _, MemCoreRepository, MemProfileRepository, MemProxyRepository};

/// Builds the real view over in-memory storage, a synchronous façade and a
/// verifier the test drives.
fn view_with_verifier(
    cx: &mut TestAppContext,
    verifier: Arc<FakeVerifier>,
) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
    view_full(cx, verifier, None)
}

/// The same view, with the settings file somewhere the test can read it.
fn view_with_config(
    cx: &mut TestAppContext,
    config: &std::path::Path,
) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
    view_full(cx, Arc::new(FakeVerifier::passing()), Some(config))
}

fn view_full(
    cx: &mut TestAppContext,
    verifier: Arc<FakeVerifier>,
    config: Option<&std::path::Path>,
) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
    let (view, runtime, _) =
        view_with_opener(cx, verifier, config, Arc::new(FakeOpener::working()));
    (view, runtime)
}

/// The same view, with the directory opener the test drives.
fn view_with_opener(
    cx: &mut TestAppContext,
    verifier: Arc<FakeVerifier>,
    config: Option<&std::path::Path>,
    opener: Arc<FakeOpener>,
) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>, Arc<FakeOpener>) {
    view_with_log(cx, verifier, config, opener, None)
}

/// The same view, with an activity log the test can read back.
///
/// The proxy tester is a passing fake here: tests that are about something
/// else should not have to think about it.
fn view_with_log(
    cx: &mut TestAppContext,
    verifier: Arc<FakeVerifier>,
    config: Option<&std::path::Path>,
    opener: Arc<FakeOpener>,
    log_file: Option<crate::log_file::LogFile>,
) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>, Arc<FakeOpener>) {
    let (view, runtime, opener, _tester, _copier) = view_with_tester(
        cx,
        verifier,
        Arc::new(FakeProxyTester::passing()),
        Arc::new(FakeBrowserDataCopier::passing()),
        config,
        opener,
        log_file,
    );
    (view, runtime, opener)
}

/// Everything the harness builds: the view, and the fakes a test drives.
///
/// A named type rather than a five-part tuple so a test can destructure it
/// without repeating the shape, and so adding a fake does not change every
/// signature that returns one.
type Harness = (
    gpui_kit::Entity<AppView>,
    Arc<FakeRuntime>,
    Arc<FakeOpener>,
    Arc<FakeProxyTester>,
    Arc<FakeBrowserDataCopier>,
);

/// The same view, with the proxy tester the test drives.
fn view_with_tester(
    cx: &mut TestAppContext,
    verifier: Arc<FakeVerifier>,
    tester: Arc<FakeProxyTester>,
    copier: Arc<FakeBrowserDataCopier>,
    config: Option<&std::path::Path>,
    opener: Arc<FakeOpener>,
    log_file: Option<crate::log_file::LogFile>,
) -> Harness {
    let profile_repo: Arc<MemProfileRepository> = Arc::new(MemProfileRepository::new());
    let core_repo: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
    let proxy_repo: Arc<MemProxyRepository> = Arc::new(MemProxyRepository::new());
    core_repo
        .save(&core(CoreId::new()))
        .expect("seed a browser core");
    // Built from the same three repositories the services get, before they
    // are moved into them: the replacement a test drives has to be visible to
    // the services, which is what "one configuration" means.
    let configuration = Arc::new(storage::MemConfiguration::new(
        core_repo.clone(),
        proxy_repo.clone(),
        profile_repo.clone(),
    ));

    let runtime = Arc::new(FakeRuntime::new());
    let profiles = Arc::new(DefaultProfileService::new(
        profile_repo.clone(),
        PathBuf::from("data"),
    ));
    let runtime_service = Arc::new(RuntimeService::new(
        profile_repo.clone(),
        core_repo.clone(),
        proxy_repo.clone(),
        runtime.clone(),
    ));
    let cores = crate::state::testing::core_service(core_repo.clone(), profile_repo.clone());
    let proxies: Arc<dyn ProxyService> =
        Arc::new(DefaultProxyService::new(proxy_repo, profile_repo));
    let settings = match config {
        Some(config) => crate::state::testing::settings_at(config),
        None => crate::state::testing::settings(),
    };
    let state = AppState::with_log(
        crate::state::Services {
            profiles,
            runtime: runtime_service,
            cores,
            proxies,
            configuration,
        },
        settings,
        log_file,
        None,
    );
    let (_command_tx, event_rx) = crossbeam_channel::bounded(16);

    let view = cx.new(|cx| {
        let mut view = AppView::new(
            state,
            event_rx,
            verifier,
            tester.clone(),
            opener.clone(),
            copier.clone(),
            crate::tray::testing::fake_starter,
        );
        view.boot(cx);
        view
    });
    (view, runtime, opener, tester, copier)
}

fn view(cx: &mut TestAppContext) -> (gpui_kit::Entity<AppView>, Arc<FakeRuntime>) {
    view_with_verifier(cx, Arc::new(FakeVerifier::passing()))
}

/// A profile to work with, made without going through the form.
///
/// Tests that are about something else should not have to drive the New
/// Profile dialog for a row; creating through the window has its own test.
fn seed_profile<C: gpui_kit::AppContext>(
    cx: &mut C,
    view: &gpui_kit::Entity<AppView>,
) -> ProfileId {
    view.update(cx, |view, cx| {
        let id = view
            .state_mut()
            .create_profile("Profile 1")
            .expect("seed a profile");
        cx.notify();
        id
    })
}

/// A profile whose name a test chose, for the filter to match on.
fn seed_named_profile<C: gpui_kit::AppContext>(
    cx: &mut C,
    view: &gpui_kit::Entity<AppView>,
    name: &str,
) -> ProfileId {
    view.update(cx, |view, cx| {
        let id = view
            .state_mut()
            .create_profile(name)
            .expect("seed a profile");
        cx.notify();
        id
    })
}

/// Types into the Profiles page's filter, the way the field reports an edit.
fn type_filter(cx: &mut gpui_kit::VisualTestContext, view: &gpui_kit::Entity<AppView>, text: &str) {
    let field = view
        .read_with(cx, |view, _| view.filter_input())
        .expect("the first render builds the filter field");
    cx.update(|window, cx| {
        field.update(cx, |state, cx| state.set_value(text, window, cx));
    });
    settle(cx);
}

/// Types into the Settings page's export path field.
fn type_export_path(
    cx: &mut gpui_kit::VisualTestContext,
    view: &gpui_kit::Entity<AppView>,
    text: &str,
) {
    let field = view
        .read_with(cx, |view, _| view.export_input())
        .expect("rendering the Settings page builds the export field");
    cx.update(|window, cx| {
        field.update(cx, |state, cx| state.set_value(text, window, cx));
    });
    settle(cx);
}

/// The last thing the window was told, from the state rather than a toast
/// that has already been drained.
fn last_message(cx: &mut gpui_kit::VisualTestContext, view: &gpui_kit::Entity<AppView>) -> String {
    view.read_with(cx, |view, _| {
        view.state()
            .toasts()
            .last()
            .map(|toast| toast.message.clone())
    })
    .expect("the window was told something")
}

/// A proxy to work with, made without going through the dialog.
///
/// Building one through the form has its own test; a test about testing a
/// proxy should not have to drive a dialog to get one.
fn seed_proxy(
    cx: &mut gpui_kit::VisualTestContext,
    view: &gpui_kit::Entity<AppView>,
    name: &str,
) -> ProxyId {
    view.update(cx, |view, _| {
        view.state_mut()
            .create_proxy(
                name,
                domain::ProxyOutbound::Socks5(domain::Socks5Outbound {
                    host: "10.0.0.1".to_string(),
                    port: 1080,
                    username: None,
                    password: None,
                }),
            )
            .expect("seed a proxy")
    })
}

/// Drives the verification channel until the worker reports, so the test
/// never depends on the tick timer firing.
fn wait_for_verification<C: gpui_kit::AppContext>(cx: &mut C, view: &gpui_kit::Entity<AppView>) {
    wait_for_state(cx, view, |state| {
        state
            .selected()
            .and_then(|row| state.verification(row.profile.id))
            .is_some_and(|verification| !verification.is_running())
    });
}

/// Drives every background queue until the state says what the test is
/// waiting for. A background action reports on a later tick, and a test
/// should not have to wait for the timer to fire to see it.
fn wait_for_state<C: gpui_kit::AppContext>(
    cx: &mut C,
    view: &gpui_kit::Entity<AppView>,
    done: impl Fn(&AppState) -> bool,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if view.read_with(cx, |view, _| done(view.state())) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the background action never reported back"
        );
        view.update(cx, |view, cx| {
            view.drain_verifications();
            view.drain_proxy_tests();
            view.drain_open_results();
            view.drain_browser_data();
            view.drain_maintenance();
            cx.notify();
        });
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A running profile leaving by a proxy that has been tested.
///
/// The whole path a proxied profile takes to being verified: a proxy, its
/// pre-flight, the profile pointed at it, and the browser up with a debug
/// port. Tests about the address question should not each re-derive it.
fn started_proxied_profile(
    cx: &mut gpui_kit::VisualTestContext,
    view: &gpui_kit::Entity<AppView>,
    runtime: &Arc<FakeRuntime>,
) -> ProfileId {
    let id = seed_profile(cx, view);
    let proxy_id = seed_proxy(cx, view, "Office");
    view.update(cx, |view, cx| {
        let state = view.state_mut();
        let mut profile = state.profile(id).expect("the profile");
        profile.proxy_id = Some(proxy_id);
        state.update_profile(profile).expect("assign");
        cx.notify();
    });

    // Pressing Start on a proxied profile asks the proxy first now, so the
    // reading this profile is judged against is the one the start's own check
    // took - the harness's tester, which leaves from `FAKE_EXIT_IP` - and the
    // command is only queued once that answer is in.
    cx.update(|window, cx| window.click(format!("start-{id}"), cx));
    wait_for_state(cx, view, |state| {
        state
            .row(id)
            .is_some_and(|row| row.state() == RuntimeState::Running)
    });
    runtime.set_cdp_port(id, 9333);
    view.update(cx, |view, cx| {
        view.state_mut().refresh_runtime();
        cx.notify();
    });
    settle(cx);
    id
}

/// Opens the window the way the component library's own dialog tests do:
/// a `VisualTestContext` can park the async work a dialog needs to mount.
fn window<'a>(
    cx: &'a mut TestAppContext,
    view: &gpui_kit::Entity<AppView>,
) -> &'a mut gpui_kit::VisualTestContext {
    cx.update(|cx| cx.set_reduce_motion(true));
    // The window's own keys, bound once per window: the program binds them at
    // startup, and a test window is the only window there is.
    cx.update(crate::ui::keys::bind);
    let (_, cx) = cx.add_window_view({
        let view = view.clone();
        move |window, cx| Root::new(view, window, cx)
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx
}

/// A real link of the shape a provider hands out, remark and all.
const SHARE_LINK: &str = "vless://b831381d-6324-4d53-ad4f-8cda48b30811@node.example:443\
        ?encryption=none&security=reality&sni=front.example&fp=chrome\
        &pbk=LOLTG162EtSegCnMAofVY3oKrbrCvH8zOTZEPd1GRQU&sid=ab12#Tokyo";

fn set_link(cx: &mut gpui_kit::VisualTestContext, view: &gpui_kit::Entity<AppView>, link: &str) {
    let import = view
        .read_with(cx, |view, _| view.proxy_import())
        .expect("an importer");
    let field = import.read_with(cx, |import, _| import.link_input());
    cx.update(|window, cx| field.update(cx, |state, cx| state.set_value(link, window, cx)));
}

fn import_error(
    cx: &mut gpui_kit::VisualTestContext,
    view: &gpui_kit::Entity<AppView>,
) -> Option<String> {
    view.read_with(cx, |view, _| view.proxy_import())
        .expect("an importer")
        .read_with(cx, |import, _| import.error().map(str::to_string))
}

/// Show whatever the state has queued, the way the tick does.
fn flush_toasts(cx: &mut gpui_kit::VisualTestContext, view: &gpui_kit::Entity<AppView>) {
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.show_toasts(window, cx));
    });
}

/// Draws enough frames for a layer to mount and then paint at rest.
fn settle(cx: &mut gpui_kit::VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

/// Types into the Settings page's import path field.
fn type_import_path(
    cx: &mut gpui_kit::VisualTestContext,
    view: &gpui_kit::Entity<AppView>,
    text: &str,
) {
    let field = view
        .read_with(cx, |view, _| view.import_input())
        .expect("rendering the Settings page builds the import field");
    cx.update(|window, cx| {
        field.update(cx, |state, cx| state.set_value(text, window, cx));
    });
    settle(cx);
}

/// One item of a profile row's overflow menu.
///
/// Named rather than numbered at the call site: the menu is a list of meanings,
/// and a test that said "click 6" the day the menu gained an entry would be
/// asserting the wrong row's deletion. The numbering is the menu's own order,
/// separator included, and it lives here where the menu is what it describes.
#[derive(Clone, Copy)]
enum ProfileMenu {
    Edit,
    Duplicate,
    OpenDir,
    Verify,
    Delete,
}

impl ProfileMenu {
    /// Where the item sits in the open menu.
    #[allow(dead_code)]
    fn index(self) -> usize {
        match self {
            Self::Edit => 0,
            Self::Duplicate => 1,
            Self::OpenDir => 2,
            Self::Verify => 3,
            // The separator between restarting and removing takes a place of
            // its own in the popup's rows.
            Self::Delete => 6,
        }
    }
}

/// One item of a core row's overflow menu.
///
/// The same reason [`ProfileMenu`] is named rather than numbered: opening a
/// core's location is now the item before deletion, and a test that counted
/// rows would have gone on passing while deleting the wrong thing.
#[derive(Clone, Copy)]
enum CoreMenu {
    /// The first item, which no test presses yet: editing a core is reached
    /// through the form's own tests, and this is here so the items after it can
    /// be named by what they are.
    #[expect(dead_code)]
    Edit,
    OpenLocation,
    Delete,
}

impl CoreMenu {
    fn index(self) -> usize {
        match self {
            Self::Edit => 0,
            Self::OpenLocation => 1,
            // The separator between opening a location and removing the core
            // takes a place of its own in the popup's rows.
            Self::Delete => 3,
        }
    }
}

/// Opens a core row's menu and clicks one item of it.
fn core_menu(cx: &mut gpui_kit::VisualTestContext, index: usize, item: CoreMenu) {
    cx.update(|window, cx| window.click(format!("more-core-{index}"), cx));
    settle(cx);
    cx.update(|window, cx| window.within("popup-menu").click(item.index(), cx));
    settle(cx);
}

/// Commits a choice in the profile form's proxy selector.
///
/// The form's two selectors are searchable lists, and the component library
/// builds a row only once its popup has been laid out - which a test window does
/// not do for the layer above a dialog. The tests therefore commit the choice
/// through the selector's own state, which is where the form reads it from, and
/// the popup itself is checked in a real window.
///
/// `None` is the direct connection.
fn pick_proxy(
    cx: &mut gpui_kit::VisualTestContext,
    view: &gpui_kit::Entity<AppView>,
    id: Option<ProxyId>,
) {
    let editor = view.read_with(cx, |view, _| view.editor()).expect("a form");
    let select = editor.read_with(cx, |editor, _| editor.proxy_select());
    cx.update(|window, cx| {
        select.update(cx, |select, cx| {
            select.set_selected_values(&[id], window, cx);
        });
    });
}

/// Opens a profile row's menu and clicks one item of it.
///
/// This is the path a user takes for everything a row does not show: the menu
/// is opened by its own button and its items are popup rows, so a test has to
/// open it and click inside it rather than reaching for an id.
fn profile_menu(cx: &mut gpui_kit::VisualTestContext, id: ProfileId, item: ProfileMenu) {
    cx.update(|window, cx| window.click(format!("more-{id}"), cx));
    settle(cx);
    cx.update(|window, cx| window.within("popup-menu").click(item.index(), cx));
    settle(cx);
}

/// Two edges are the same edge, within the rounding of a layout pass.
///
/// Shared by the pages that have columns: a heading and the cell under it are
/// laid out by two different chains of boxes, and the failure this catches is a
/// table that reads as ragged at a glance without any one element being wrong.
pub(super) fn assert_aligned(left: gpui_kit::Pixels, right: gpui_kit::Pixels, what: &str) {
    let gap = (left - right).abs().as_f32();
    assert!(gap < 0.5, "{what} is out by {gap}px: {left:?} vs {right:?}");
}

/// The band along the bottom edge of the window where a click does not reach
/// what is under it.
///
/// Measured rather than read out of the component library: a button whose
/// middle is inside the window and whose bounds are inside the viewport is
/// still not clickable once it has scrolled to within about forty pixels of
/// the bottom - the library's window border keeps a resize band and an
/// overlay layer there, and a click that lands in it is swallowed without an
/// error. So a target has to be this clear of the bottom before it counts.
const CLICKABLE_BOTTOM_MARGIN: f32 = 48.;

/// Whether an element is fully on screen and clickable.
///
/// `visible()` alone is not enough, twice over. It is true for an element
/// with one pixel showing at the edge of the scrolled area, and a click is
/// aimed at the middle of the element and hit-tested there: an element that
/// has just scrolled into view by one step reports visible while half of it
/// is still past the bottom of the page, so the click lands on the scroll
/// view instead of on the button - and nothing reports an error, because the
/// window simply received a click somewhere else. And an element in the
/// bottom band above is swallowed even when it is entirely inside the
/// viewport. Requiring the whole element to be clear of both is what tells
/// those states apart from a click that will land.
///
/// The id is copied because `try_find` wants an owned one: the ids here are
/// built at the call site (`setting-data-dir`, and so on) rather than being
/// the `&'static str` the lookup asks for.
fn is_clickable(cx: &mut gpui_kit::VisualTestContext, id: &str) -> bool {
    let id = id.to_string();
    cx.update(|window, _| {
        let Some(found) = window.try_find(id) else {
            return false;
        };
        if !found.visible() {
            return false;
        }
        let size = window.viewport_size();
        let usable = gpui_kit::Bounds::new(
            gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)),
            gpui_kit::Size {
                width: size.width,
                height: size.height - gpui_kit::px(CLICKABLE_BOTTOM_MARGIN),
            },
        );
        found.bounds().is_contained_within(&usable)
    })
}

/// Scrolls the Settings page until `target` is on screen.
///
/// The page is longer than the test window and its cards sit below the fold,
/// and a click on an off-screen element is refused. The wheel event has to be
/// dispatched over a child of the scrolling container - the container is not
/// something the helpers can aim at - and that child has to be on screen
/// itself. A single long scroll aimed at one row stops working the moment the
/// page grows past it, which is what happened when a card was added at the
/// bottom; so each step aims at whichever row or card is on screen now, which
/// is also where the wheel would really land.
fn scroll_settings_to(cx: &mut gpui_kit::VisualTestContext, target: &str) {
    // The page shows one group at a time, so the control has to be in the group
    // on screen before a wheel event could ever reach it.
    cx.update(|window, cx| window.click(settings_group_of(target), cx));
    settle(cx);
    let mut previous = None;
    for _ in 0..40 {
        if is_clickable(cx, target) {
            return;
        }
        let now = cx.update(|window, _| window.try_find(target.to_string()).map(|id| id.bounds()));
        if now.is_some() && now == previous {
            // The page is at its end: the element is as far into view as it
            // is going to get, and another wheel event would only spin.
            panic!("{target} cannot be scrolled clear of the bottom of the window");
        }
        previous = now;
        let aim = crate::settings::SettingKey::ALL
            .iter()
            .map(|key| format!("setting-{}", key.id()))
            .chain(SETTINGS_CARDS.iter().map(|id| (*id).to_string()))
            .find(|id| is_clickable(cx, id))
            .unwrap_or_else(|| panic!("nothing on the Settings page is on screen to scroll from"));
        cx.update(|window, cx| {
            window.scroll(
                aim,
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(-200.),
                )),
                cx,
            );
        });
        settle(cx);
    }
    panic!("{target} never came into view");
}

/// Every card the Settings page can show, in the order they appear, for the
/// scroll helper to aim at.
///
/// All four groups at once: the helper asks whether each is on screen, and the
/// ones in the groups that are not open simply are not there. A card added to a
/// group without being added here costs the helper an aim, not a wrong answer.
const SETTINGS_CARDS: [&str; 8] = [
    "appearance",
    "exit-mode",
    "export-configuration",
    "import-configuration",
    "restore-configuration",
    "browser-data",
    "diagnostics",
    "about",
];

/// Which of the page's four groups a control the tests ask for lives in.
///
/// Spelled once here so a test can ask for the control it needs without also
/// having to know the page's taxonomy, and so a control that moves between
/// groups breaks one line rather than every test that presses it.
///
/// The fallback is the Runtime group, which is where the four `setting-*` rows
/// of a process's paths and endpoints are; every other id the suite asks for is
/// named above.
fn settings_group_of(target: &str) -> &'static str {
    match target {
        "appearance" | "exit-mode" | "theme-dark" | "theme-light" | "language-en"
        | "language-zh" | "exit-ask" | "exit-background" | "exit-keep-running"
        | "exit-exit-all" => "settings-group-general",
        "export-configuration"
        | "import-configuration"
        | "restore-configuration"
        | "browser-data"
        | "export-path"
        | "import-path"
        | "restore-path"
        | "browser-data-path"
        | "export-run"
        | "import-run"
        | "restore-run"
        | "browser-data-out"
        | "browser-data-in"
        | "setting-data-dir"
        | "setting-config-file"
        | "setting-runtime-dir" => "settings-group-data",
        "diagnostics" | "about" | "diagnostics-run" => "settings-group-diagnostics",
        _ => "settings-group-runtime",
    }
}

/// Shows one of the Settings page's four groups.
///
/// The group chips are the page's own navigation, so a test selects a group the
/// way a reader does - and a chip that stopped switching the page would fail
/// every test that needs a control inside it.
fn open_settings_group(cx: &mut gpui_kit::VisualTestContext, group: crate::settings::SettingGroup) {
    cx.update(|window, cx| window.click(group.id(), cx));
    settle(cx);
}

/// Types into the Settings page's restore path field.
fn type_restore_path(
    cx: &mut gpui_kit::VisualTestContext,
    view: &gpui_kit::Entity<AppView>,
    text: &str,
) {
    let field = view
        .read_with(cx, |view, _| view.restore_input())
        .expect("rendering the Settings page builds the restore field");
    cx.update(|window, cx| {
        field.update(cx, |state, cx| state.set_value(text, window, cx));
    });
    settle(cx);
}

/// A stopped profile that leaves through a proxy: the shape every pre-flight
/// test starts from.
fn seed_proxied_profile(
    cx: &mut gpui_kit::VisualTestContext,
    view: &gpui_kit::Entity<AppView>,
) -> (ProfileId, ProxyId) {
    let id = seed_profile(cx, view);
    let proxy_id = seed_proxy(cx, view, "Office");
    view.update(cx, |view, cx| {
        let state = view.state_mut();
        let mut profile = state.profile(id).expect("the profile");
        profile.proxy_id = Some(proxy_id);
        state.update_profile(profile).expect("assign");
        cx.notify();
    });
    (id, proxy_id)
}

/// The same view, with the browser-data copier the test drives.
fn view_with_browser_data(
    cx: &mut TestAppContext,
    copier: Arc<FakeBrowserDataCopier>,
) -> (
    gpui_kit::Entity<AppView>,
    Arc<FakeRuntime>,
    Arc<FakeBrowserDataCopier>,
) {
    let (view, runtime, _opener, _tester, copier) = view_with_tester(
        cx,
        Arc::new(FakeVerifier::passing()),
        Arc::new(FakeProxyTester::passing()),
        copier,
        None,
        Arc::new(FakeOpener::working()),
        None,
    );
    (view, runtime, copier)
}

mod activity;
mod cores;
mod keyboard;
mod maintenance;
mod profiles;
mod proxies;
mod settings;
mod verification;
