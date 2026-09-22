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

/// The Settings card switches what closing the window does, and the choice is
/// written down rather than only shown.
#[gpui_kit::test]
fn the_exit_mode_card_switches_and_stores_the_choice(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    // A config file of this test's own: the choice is stored, and the shared
    // one would make every other test that reads it see this test's answer.
    let dir = std::env::temp_dir().join(format!("fp-ui-exit-card-{}", std::process::id()));
    let (view, _) = view_with_config(cx, &dir.join("config.json"));
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("exit-keep-running").is_some()),
        "the card offers the answers, not just the one in force"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().exit_mode()),
        ExitMode::Ask,
        "a fresh installation asks rather than deciding"
    );

    scroll_settings_to(cx, "exit-keep-running");
    cx.update(|window, cx| window.click("exit-keep-running", cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.state().exit_mode()),
        ExitMode::KeepRunning,
        "the chip stores the answer it names"
    );

    scroll_settings_to(cx, "exit-exit-all");
    cx.update(|window, cx| window.click("exit-exit-all", cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.state().exit_mode()),
        ExitMode::ExitAll
    );
}

/// "Ask" is answered with a question rather than a guess: a close request
/// while the mode is "ask" opens the three choices and refuses the close.
#[gpui_kit::test]
fn closing_the_window_asks_when_the_mode_is_ask(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-exit-ask-{}", std::process::id()));
    let (view, _) = view_with_config(cx, &dir.join("config.json"));
    let cx = window(cx, &view);
    let ask = |cx: &mut gpui_kit::VisualTestContext| {
        cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)))
    };

    assert!(!ask(cx), "an unanswered question must not close the window");
    assert!(
        cx.update(|window, _| window.try_find("exit-choice-exit-all").is_some()),
        "the three answers are the dialog"
    );

    // Choosing one carries it out and takes the dialog away, and nothing is
    // remembered unless the box was ticked. "Stop everything" is the answer
    // this test presses because the harness window cannot be minimized, which
    // is what "keep running" asks of it.
    cx.update(|window, cx| window.click("exit-choice-exit-all", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("exit-choice-exit-all").is_none()),
        "the answer closes the question"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().exit_mode()),
        ExitMode::Ask,
        "without the box ticked, nothing is remembered"
    );
}

/// The three answers are three of the same thing, and the last one is whole.
///
/// Both halves were wrong before this was a test: the last row came out a text
/// line shorter than the two above it and its own note painted over where its
/// bottom border was. The text, the order and the line height were each ruled
/// out by measuring, which is why the height is now given rather than derived.
#[gpui_kit::test]
fn the_exit_dialog_shows_three_equal_answers_inside_itself(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-exit-equal-{}", std::process::id()));
    let (view, _) = view_with_config(cx, &dir.join("config.json"));
    let cx = window(cx, &view);

    let _ = cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
    settle(cx);

    let bounds = |id: &str, cx: &mut gpui_kit::VisualTestContext| {
        cx.update(|window, _| window.try_find(id.to_string()).expect(id).bounds())
    };
    let surface = cx.debug_bounds("dialog-0").expect("the dialog's surface");
    let rows = [
        "exit-choice-background",
        "exit-choice-keep-running",
        "exit-choice-exit-all",
    ]
    .map(|id| bounds(id, cx));

    assert_eq!(rows[0].size.height, rows[1].size.height, "{rows:?}");
    assert_eq!(rows[1].size.height, rows[2].size.height, "{rows:?}");
    for (index, row) in rows.iter().enumerate() {
        assert!(
            row.bottom() <= surface.bottom(),
            "answer {index} is outside the dialog: {row:?} in {surface:?}"
        );
        if let Some(next) = rows.get(index + 1) {
            assert!(
                row.bottom() <= next.origin.y,
                "answer {index} overlaps the next one: {rows:?}"
            );
        }
    }
}

/// A remembered mode is an answer, so no question is asked: the close is
/// allowed (or, for "keep running", refused) without a dialog.
#[gpui_kit::test]
fn a_remembered_exit_mode_is_carried_out_without_asking(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-exit-remember-{}", std::process::id()));
    let (view, runtime) = view_with_config(cx, &dir.join("config.json"));
    let cx = window(cx, &view);
    let commands = || runtime.commands.lock().expect("commands").clone();

    // "Stop everything" says nothing to the runtime: what this program has
    // always done at exit is stop its children, and `main` does that after
    // the window loop returns.
    view.update(cx, |view, _| {
        view.state_mut()
            .set_exit_mode(ExitMode::ExitAll)
            .expect("store the answer")
    });
    let allowed =
        cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
    assert!(allowed, "a remembered answer closes the window");
    assert!(
        cx.update(|window, _| window.try_find("exit-remember").is_none()),
        "and it was not asked about"
    );
    assert!(
        !commands().iter().any(|command| command == "release-all"),
        "stopping everything does not release anything: {:?}",
        commands()
    );

    // "Leave browsers running" does: it is the one answer the runtime has to
    // be told about, and the program is about to end.
    view.update(cx, |view, _| {
        view.state_mut()
            .set_exit_mode(ExitMode::KeepRunning)
            .expect("store the answer")
    });
    let allowed =
        cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
    assert!(allowed);
    assert!(
        commands().iter().any(|command| command == "release-all"),
        "leaving with the browsers running must tell the runtime: {:?}",
        commands()
    );

    // "Background" keeps running, refuses the close, hides the window and starts the tray icon.
    view.update(cx, |view, _| {
        view.state_mut()
            .set_exit_mode(ExitMode::Background)
            .expect("store the answer")
    });
    let allowed =
        cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
    assert!(
        !allowed,
        "background mode refuses the close so app keeps running"
    );
    view.update(cx, |view, _| {
        assert!(
            view.tray.is_some(),
            "entering background starts the tray icon"
        );
    });

    // TrayEvent::Show wakes/restores the window.
    cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.handle_tray(crate::tray::TrayEvent::Show, window, cx);
        });
    });
}

#[gpui_kit::test]
fn profiles_can_be_created_started_and_stopped_from_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, runtime) = view(cx);
    let cx = window(cx, &view);

    assert!(
        cx.update(|window, _| window.try_find("quit").is_some()),
        "the header renders a quit affordance"
    );
    assert_eq!(view.read_with(cx, |view, _| view.state().rows().len()), 0);

    let label = |cx: &mut gpui_kit::VisualTestContext, id: ProfileId| {
        cx.update(|window, _| {
            window
                .find(format!("state-{id}"))
                .label()
                .map(|label| label.to_string())
        })
    };

    // New Profile opens the form: the row appears when the form is accepted,
    // not when the button is clicked.
    cx.update(|window, cx| window.click("new-profile", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("editor-name").is_some()),
        "the New Profile button opens the form"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().rows().len()),
        0,
        "nothing is written while the form is still open"
    );
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    cx.update(|window, cx| window.click("new-profile", cx));
    settle(cx);
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    let ids = view.read_with(cx, |view, _| {
        view.state()
            .rows()
            .iter()
            .map(|row| row.profile.id)
            .collect::<Vec<_>>()
    });
    assert_eq!(ids.len(), 2, "two profiles after two accepted forms");

    let (first, second) = (ids[0], ids[1]);
    assert_eq!(label(cx, first).as_deref(), Some("Stopped"));
    assert_eq!(label(cx, second).as_deref(), Some("Stopped"));

    cx.update(|window, cx| window.click(format!("start-{first}"), cx));
    settle(cx);
    assert_eq!(label(cx, first).as_deref(), Some("Running"));
    assert_eq!(
        label(cx, second).as_deref(),
        Some("Stopped"),
        "starting one profile leaves the other alone"
    );

    cx.update(|window, cx| window.click(format!("stop-{first}"), cx));
    settle(cx);
    assert_eq!(label(cx, first).as_deref(), Some("Stopped"));

    let commands = runtime.commands.lock().expect("command log").clone();
    assert_eq!(
        commands.len(),
        2,
        "one start and one stop reached the façade"
    );
    assert!(commands[0].starts_with("start:"));
    assert!(commands[1].starts_with("stop:"));
}

/// A new profile is the one that was configured, on the core that was picked.
#[gpui_kit::test]
fn a_new_profile_is_created_from_the_form_with_the_core_it_was_given(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    let seeded_core = view.read_with(cx, |view, _| {
        view.state()
            .core_rows()
            .expect("rows")
            .first()
            .expect("the fixture seeds a core")
            .core
            .id
    });

    cx.update(|window, cx| window.click("new-profile", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("editor-core-0").is_some()),
        "the form offers the core the profile will run on"
    );

    let editor = view.read_with(cx, |view, _| view.editor()).expect("a form");
    let name = editor.read_with(cx, |editor, _| editor.name_input());
    cx.update(|window, cx| {
        name.update(cx, |state, cx| state.set_value("Shop account", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    let row = view.read_with(cx, |view, _| view.state().rows()[0].clone());
    assert_eq!(row.profile.name, "Shop account");
    assert_eq!(
        row.profile.core_id, seeded_core,
        "the profile runs on the core the form was opened with"
    );
    assert!(
        cx.update(|window, _| window.try_find("editor-name").is_none()),
        "accepting the form closes it"
    );
}

#[gpui_kit::test]
fn a_long_compatibility_warning_stays_inside_the_row(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, runtime) = view(cx);

    let handle = cx.open_window(size(px(1200.), px(800.)), |window, cx| {
        Root::new(view.clone(), window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let id = seed_profile(cx, &view);

        // The longest warning the compatibility layer produces.
        runtime.set_warning(
            id,
            "chrome 128 (major 128) is older than the verified fingerprint generation \
                 (major 144) and omits --fingerprinting-canvas-image-data-noise,\
                 --fingerprinting-client-rects-noise; requested by profile canvas and \
                 client-rects spoofing",
        );
        view.update(cx, |view, cx| {
            view.state_mut().refresh_runtime();
            cx.notify();
        });
        window.render_frame(cx);

        let warning = window.find(format!("warning-{id}"));
        assert!(warning.visible(), "the row reports the warning");
        assert!(
            warning.bounds().size.width <= px(420.0),
            "the row warning is capped instead of overflowing: {:?}",
            warning.bounds().size
        );
    })
    .unwrap();
}

/// One request through a proxy, and the address it left from on the row.
///
/// This is the whole capability: without it the window only ever knows that
/// the engine's local port is open, and a proxy that accepts the connection
/// and carries nothing is indistinguishable from a working one.
#[gpui_kit::test]
fn testing_a_proxy_from_the_window_reports_where_the_traffic_left(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime, _opener, tester, _copier) = view_with_tester(
        cx,
        Arc::new(FakeVerifier::passing()),
        Arc::new(FakeProxyTester::passing_from("198.51.100.9")),
        Arc::new(FakeBrowserDataCopier::passing()),
        None,
        Arc::new(FakeOpener::working()),
        None,
    );
    let cx = window(cx, &view);
    let proxy_id = seed_proxy(cx, &view, "Office");

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("test-proxy-0", cx));
    settle(cx);

    wait_for_state(cx, &view, |state| {
        state
            .proxy_test(proxy_id)
            .is_some_and(|test| !test.is_running())
    });

    let test = view
        .read_with(cx, |view, _| view.state().proxy_test(proxy_id).cloned())
        .expect("a result");
    let reading = test.reading().expect("a reading, not a fault");
    assert_eq!(reading.exit_ip, "198.51.100.9");
    assert!(
        !reading.live,
        "nothing was running, so the test started an engine of its own"
    );
    assert_eq!(tester.calls(), 1);
    assert_eq!(
        tester.ports_asked(),
        vec![None],
        "nothing was running, so no port was handed to the tester"
    );

    // And the row says what the request found, not just that it ran.
    let row = cx.update(|window, _| window.find(format!("proxy-test-{proxy_id}")));
    assert!(row.visible(), "the row reports the reading");
}

/// A proxy that carries nothing must not read as a working one, and the row
/// must name the class rather than only that something failed.
#[gpui_kit::test]
fn a_proxy_that_carries_nothing_says_so_on_the_row(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime, _opener, tester, _copier) = view_with_tester(
        cx,
        Arc::new(FakeVerifier::passing()),
        Arc::new(FakeProxyTester::with_outcome(Err(Fault::new(
            FaultClass::Unreachable,
            "no route to the upstream",
        )))),
        Arc::new(FakeBrowserDataCopier::passing()),
        None,
        Arc::new(FakeOpener::working()),
        None,
    );
    let cx = window(cx, &view);
    let proxy_id = seed_proxy(cx, &view, "Office");

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("test-proxy-0", cx));
    settle(cx);

    wait_for_state(cx, &view, |state| {
        state
            .proxy_test(proxy_id)
            .is_some_and(|test| !test.is_running())
    });

    let test = view
        .read_with(cx, |view, _| view.state().proxy_test(proxy_id).cloned())
        .expect("a result");
    assert!(
        test.reading().is_none(),
        "nothing left, so nothing may be reported as a reading"
    );
    assert_eq!(
        test.fault().expect("the fault").class,
        FaultClass::Unreachable
    );
    assert_eq!(test.label(en()), "no traffic (unreachable)");
    assert_eq!(tester.calls(), 1);
    cx.update(|window, _| {
        assert!(
            window.find(format!("proxy-test-{proxy_id}")).visible(),
            "the row reports the failure"
        )
    });
}

/// A proxy still being tested cannot be tested again: that would start a
/// second engine for an answer already on its way.
#[gpui_kit::test]
fn a_proxy_already_being_tested_is_not_tested_twice(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime, _opener, tester, _copier) = view_with_tester(
        cx,
        Arc::new(FakeVerifier::passing()),
        Arc::new(FakeProxyTester::passing()),
        Arc::new(FakeBrowserDataCopier::passing()),
        None,
        Arc::new(FakeOpener::working()),
        None,
    );
    let cx = window(cx, &view);
    let proxy_id = seed_proxy(cx, &view, "Office");

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("test-proxy-0", cx));
    settle(cx);
    // The first click's worker has not reported yet, so the slot is taken.
    cx.update(|window, cx| window.click("test-proxy-0", cx));
    settle(cx);

    wait_for_state(cx, &view, |state| {
        state
            .proxy_test(proxy_id)
            .is_some_and(|test| !test.is_running())
    });
    assert_eq!(tester.calls(), 1, "the second click was refused");
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

#[gpui_kit::test]
fn a_confirmed_fingerprint_is_reported_in_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let verifier = Arc::new(FakeVerifier::passing());
    let (view, runtime) = view_with_verifier(cx, verifier.clone());

    let handle = cx.open_window(size(px(1200.), px(900.)), |window, cx| {
        Root::new(view.clone(), window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let id = seed_profile(cx, &view);

        // A stopped profile has no browser to read.
        assert!(
            view.read_with(cx, |view, _| view.state().verification(id).is_none()),
            "nothing is verified before it is asked for"
        );
        window.click(format!("start-{id}"), cx);
        runtime.set_cdp_port(id, 9333);
        view.update(cx, |view, cx| {
            view.state_mut().refresh_runtime();
            cx.notify();
        });
        window.render_frame(cx);

        window.click(format!("verify-{id}"), cx);
        window.render_frame(cx);

        wait_for_verification(cx, &view);
        window.render_frame(cx);

        let verification = view
            .read_with(cx, |view, _| view.state().verification(id).cloned())
            .expect("a result is recorded");
        assert!(matches!(verification, Verification::Confirmed(_)));
        assert_eq!(verifier.calls(), 1);
    })
    .unwrap();
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

/// Verifying a proxied profile also asks where its traffic leaves from. The
/// expectation is the address the proxy was measured at, because that is the
/// only place in the product that knows where the traffic should have gone.
#[gpui_kit::test]
fn verifying_a_proxied_profile_asks_the_endpoint_its_proxy_was_tested_at(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let verifier = Arc::new(FakeVerifier::passing_from("203.0.113.7"));
    let (view, runtime) = view_with_verifier(cx, verifier.clone());
    let cx = window(cx, &view);
    let id = started_proxied_profile(cx, &view, &runtime);

    cx.update(|window, cx| window.click(format!("verify-{id}"), cx));
    wait_for_verification(cx, &view);

    let asked = verifier.egrees_asked();
    assert_eq!(asked.len(), 1, "the address question is asked once");
    let egress = asked[0]
        .clone()
        .expect("a proxied profile is asked about its address");
    assert_eq!(
        egress.expected.as_deref(),
        Some(crate::proxy_tester::testing::FAKE_EXIT_IP),
        "the expectation is what the proxy was measured at, not the reading"
    );
    assert!(
        egress.echo_url.starts_with("http://"),
        "the same endpoint the pre-flight used: {}",
        egress.echo_url
    );
}

/// Where the traffic went is the half of the answer that says the path is
/// the intended one, so it is shown rather than left in the log.
#[gpui_kit::test]
fn the_address_a_verified_profile_left_from_is_shown_on_the_profile(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, runtime) =
        view_with_verifier(cx, Arc::new(FakeVerifier::passing_from("203.0.113.7")));
    let cx = window(cx, &view);
    let id = started_proxied_profile(cx, &view, &runtime);

    cx.update(|window, cx| window.click(format!("verify-{id}"), cx));
    wait_for_verification(cx, &view);
    settle(cx);

    let reading = cx.update(|window, _| window.find("exit-address"));
    assert!(
        reading.visible(),
        "the address the traffic left from belongs on the profile"
    );
}

#[gpui_kit::test]
fn disagreements_are_listed_claim_by_claim(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let verifier = Arc::new(FakeVerifier::disagreeing(vec![Discrepancy {
        claim: "platform",
        expected: "Win32".to_string(),
        observed: "Linux x86_64".to_string(),
    }]));
    let (view, runtime) = view_with_verifier(cx, verifier);

    let handle = cx.open_window(size(px(1200.), px(900.)), |window, cx| {
        Root::new(view.clone(), window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let id = seed_profile(cx, &view);
        window.click(format!("start-{id}"), cx);
        runtime.set_cdp_port(id, 9333);
        view.update(cx, |view, cx| {
            view.state_mut().refresh_runtime();
            cx.notify();
        });
        window.render_frame(cx);

        window.click(format!("verify-{id}"), cx);

        wait_for_verification(cx, &view);
        window.render_frame(cx);

        let verification = view
            .read_with(cx, |view, _| view.state().verification(id).cloned())
            .expect("a result is recorded");
        assert_eq!(verification.label(en()), "1 claim not confirmed");
        assert_eq!(verification.disagreements()[0].observed, "Linux x86_64");
        assert!(
            !view.read_with(cx, |view, _| view
                .state()
                .verification(id)
                .is_some_and(Verification::is_running)),
            "the result replacement is not a spinner left behind"
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn every_disagreement_is_reachable_from_a_short_panel(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let found: Vec<Discrepancy> = (0usize..7)
        .map(|index| Discrepancy {
            claim: "platform",
            expected: format!("expected-{index}"),
            observed: format!("observed-{index}"),
        })
        .collect();
    let (view, runtime) = view_with_verifier(cx, Arc::new(FakeVerifier::disagreeing(found)));

    let handle = cx.open_window(size(px(1200.), px(700.)), |window, cx| {
        Root::new(view.clone(), window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let id = seed_profile(cx, &view);
        window.click(format!("start-{id}"), cx);
        runtime.set_cdp_port(id, 9333);
        view.update(cx, |view, cx| {
            view.state_mut().refresh_runtime();
            cx.notify();
        });
        window.render_frame(cx);

        window.click(format!("verify-{id}"), cx);
        wait_for_verification(cx, &view);
        window.render_frame(cx);

        // A panel that only ever showed the first few claims would hide
        // the rest of the answer. The list is longer than the panel, so the
        // later claims become visible by scrolling, not by being dropped.
        assert!(
            window.find(("disagreement", 0usize)).visible(),
            "the first claim is rendered"
        );
        let recorded = view.read_with(cx, |view, _| {
            view.state()
                .verification(id)
                .map(|verification| verification.disagreements().len())
        });
        assert_eq!(recorded, Some(7), "no claim is dropped before rendering");
        window.scroll(
            "details-scroll",
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.0), px(-400.0))),
            cx,
        );
        window.render_frame(cx);
        assert!(
            window.find(("disagreement", 6usize)).visible(),
            "the last claim is reachable by scrolling"
        );
    })
    .unwrap();
}

/// Opens the window the way the component library's own dialog tests do:
/// a `VisualTestContext` can park the async work a dialog needs to mount.
fn window<'a>(
    cx: &'a mut TestAppContext,
    view: &gpui_kit::Entity<AppView>,
) -> &'a mut gpui_kit::VisualTestContext {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (_, cx) = cx.add_window_view({
        let view = view.clone();
        move |window, cx| Root::new(view, window, cx)
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx
}

#[gpui_kit::test]
fn the_sidebar_switches_to_the_proxies_page(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    assert!(
        cx.update(|window, _| window.try_find("proxies-scroll").is_none()),
        "the proxies page is not shown first"
    );

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("new-proxy").is_some()),
        "the proxies page is shown"
    );
    assert!(
        cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
        "the profiles body is not rendered on the proxies page"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        crate::state::Page::Proxies
    );
}

#[gpui_kit::test]
fn a_proxy_can_be_created_from_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("new-proxy", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("proxy-name").is_some()),
        "the proxy form opens"
    );

    let editor = view
        .read_with(cx, |view, _| view.proxy_editor())
        .expect("a proxy editor");
    let (name, host) = editor.read_with(cx, |editor, _| (editor.name_input(), editor.host_input()));
    cx.update(|window, cx| {
        name.update(cx, |state, cx| state.set_value("Office", window, cx));
        host.update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    let rows = view.read_with(cx, |view, cx| {
        let _ = cx;
        view.state().proxy_rows().expect("rows")
    });
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].proxy.name, "Office");
    assert_eq!(rows[0].endpoint(), "socks5://10.0.0.1:1080");
    assert!(
        cx.update(|window, _| window.try_find("proxy-name").is_none()),
        "saving closes the dialog"
    );
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

#[gpui_kit::test]
fn a_pasted_link_becomes_a_proxy(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("import-proxy", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("proxy-link").is_some()),
        "the import dialog opens"
    );

    set_link(cx, &view, SHARE_LINK);
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    let rows = view.read_with(cx, |view, cx| {
        let _ = cx;
        view.state().proxy_rows().expect("rows")
    });
    assert_eq!(rows.len(), 1);
    // The remark in the link is the name, and the endpoint is what the link
    // said rather than what a form would have rebuilt.
    assert_eq!(rows[0].proxy.name, "Tokyo");
    assert_eq!(rows[0].endpoint(), "vless://node.example:443");
    assert!(
        cx.update(|window, _| window.try_find("proxy-link").is_none()),
        "importing closes the dialog"
    );
}

#[gpui_kit::test]
fn a_link_this_program_cannot_read_says_so_and_creates_nothing(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("import-proxy", cx));
    settle(cx);

    set_link(cx, &view, "hysteria2://secret@node.example:443");
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    let error = import_error(cx, &view).expect("a refusal");
    assert!(error.contains("hysteria2"), "{error}");
    assert!(
        cx.update(|window, _| window.try_find("proxy-import-error").is_some()),
        "the dialog shows the reason"
    );
    assert!(
        cx.update(|window, _| window.try_find("proxy-link").is_some()),
        "the dialog stays open so the link can be fixed"
    );
    let rows = view.read_with(cx, |view, cx| {
        let _ = cx;
        view.state().proxy_rows().expect("rows")
    });
    assert!(rows.is_empty(), "a refused link creates nothing");
}

#[gpui_kit::test]
fn a_refused_proxy_form_says_why_and_stays_open(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("new-proxy", cx));
    settle(cx);

    let editor = view
        .read_with(cx, |view, _| view.proxy_editor())
        .expect("a proxy editor");
    let name = editor.read_with(cx, |editor, _| editor.name_input());
    cx.update(|window, cx| {
        name.update(cx, |state, cx| state.set_value("No host", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("proxy-error").is_some()),
        "the form says what is wrong"
    );
    let shown = editor
        .read_with(cx, |editor, _| editor.error().map(str::to_string))
        .expect("the reason is recorded on the form");
    assert!(shown.contains("host"), "{shown}");
    assert!(
        cx.update(|window, _| window.try_find("proxy-name").is_some()),
        "a refused save leaves the form open"
    );
    assert!(
        view.read_with(cx, |view, cx| {
            let _ = cx;
            view.state().proxy_rows().expect("rows").is_empty()
        }),
        "nothing was written"
    );
}

#[gpui_kit::test]
fn assigning_a_proxy_from_the_profile_editor_reaches_storage(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    // A proxy to choose, made through its own page.
    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("new-proxy", cx));
    settle(cx);
    let proxy_editor = view
        .read_with(cx, |view, _| view.proxy_editor())
        .expect("a proxy editor");
    let (name, host) =
        proxy_editor.read_with(cx, |editor, _| (editor.name_input(), editor.host_input()));
    cx.update(|window, cx| {
        name.update(cx, |state, cx| state.set_value("Office", window, cx));
        host.update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);
    let proxy_id = view.read_with(cx, |view, cx| {
        let _ = cx;
        view.state().proxy_rows().expect("rows")[0].proxy.id
    });

    // A profile to assign it to.
    cx.update(|window, cx| window.click("nav-Profiles", cx));
    settle(cx);
    let id = seed_profile(cx, &view);
    cx.update(|window, cx| window.click(format!("edit-{id}"), cx));
    settle(cx);

    cx.update(|window, cx| window.click("editor-proxy-0", cx));
    settle(cx);
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    assert_eq!(
        view.read_with(cx, |view, _| view
            .state()
            .profile(id)
            .expect("the profile is loaded")
            .proxy_id),
        Some(proxy_id),
        "the assignment reached the state"
    );
}

#[gpui_kit::test]
fn deleting_a_proxy_in_use_is_refused_in_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("new-proxy", cx));
    settle(cx);
    let proxy_editor = view
        .read_with(cx, |view, _| view.proxy_editor())
        .expect("a proxy editor");
    let (name, host) =
        proxy_editor.read_with(cx, |editor, _| (editor.name_input(), editor.host_input()));
    cx.update(|window, cx| {
        name.update(cx, |state, cx| state.set_value("Office", window, cx));
        host.update(cx, |state, cx| state.set_value("10.0.0.1", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    cx.update(|window, cx| window.click("nav-Profiles", cx));
    settle(cx);
    let id = seed_profile(cx, &view);
    cx.update(|window, cx| window.click(format!("edit-{id}"), cx));
    settle(cx);
    cx.update(|window, cx| window.click("editor-proxy-0", cx));
    settle(cx);
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("delete-proxy-0", cx));
    settle(cx);
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    assert_eq!(
        view.read_with(cx, |view, cx| {
            let _ = cx;
            view.state().proxy_rows().expect("rows").len()
        }),
        1,
        "the proxy is still there"
    );
    let notice = view
        .read_with(cx, |view, _| view.state().notice().cloned())
        .expect("the refusal is shown");
    assert!(notice.error, "the refusal is an error");
    assert!(
        notice.message.contains("Office") && notice.message.contains("Profile 1"),
        "the message names the proxy and who holds it: {}",
        notice.message
    );
}

#[gpui_kit::test]
fn the_sidebar_switches_to_the_cores_page(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    assert!(
        cx.update(|window, _| window.try_find("cores-scroll").is_none()),
        "the cores page is not shown first"
    );

    cx.update(|window, cx| window.click("nav-Cores", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("new-core").is_some()),
        "the cores page is shown"
    );
    // Only one page is rendered at a time: the profiles list and its
    // details panel used to follow the cores page onto the screen.
    assert!(
        cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
        "the profiles body is not rendered on the cores page"
    );
    assert!(
        cx.update(|window, _| window.try_find("new-profile").is_none()),
        "the profiles header is not rendered on the cores page"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        crate::state::Page::Cores
    );
}

#[gpui_kit::test]
fn a_core_can_be_added_from_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    let binary = crate::state::testing::CoreBinary::new("ui-add", Some("Chromium 148.0.7778.215"));
    let path = binary.path_buf();

    cx.update(|window, cx| window.click("nav-Cores", cx));
    settle(cx);
    cx.update(|window, cx| window.click("new-core", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("core-executable").is_some()),
        "the core form opens"
    );

    let editor = view
        .read_with(cx, |view, _| view.core_editor())
        .expect("a core editor");
    let executable = editor.read_with(cx, |editor, _| editor.executable_input());
    cx.update(|window, cx| {
        executable.update(cx, |state, cx| {
            state.set_value(path.to_string_lossy().to_string(), window, cx)
        });
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    let rows = view.read_with(cx, |view, _| view.state().core_rows().expect("rows"));
    assert_eq!(rows.len(), 2, "the seeded core plus the added one");
    let added = rows
        .iter()
        .find(|row| row.core.executable == path)
        .expect("the added core is listed");
    assert_eq!(added.core.major, 148);
    assert_eq!(added.core.version, "Chromium 148.0.7778.215");
    assert!(
        cx.update(|window, _| window.try_find("core-executable").is_none()),
        "saving closes the dialog"
    );
}

/// The walk a fresh installation asks for, in one test.
///
/// Every step of it is covered on its own elsewhere; what this pins is the
/// handover between them - that the button the empty state offers lands on
/// the page where a core can be added, that adding one stops the list asking
/// for it and lets a profile be created, and that the profile created that
/// way really starts. First-run guidance is a path, and a path is what a
/// per-step test cannot check.
#[gpui_kit::test]
fn a_fresh_installation_can_be_walked_from_no_core_to_a_running_profile(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, runtime) = view(cx);
    let cx = window(cx, &view);
    let binary =
        crate::state::testing::CoreBinary::new("ui-first-run", Some("Chromium 148.0.7778.215"));

    // A first start: no cores at all, which the fixture does not build.
    view.update(cx, |view, _| {
        let seeded = view.state().core_rows().expect("rows")[0].core.id;
        view.state_mut().delete_core(seeded).expect("remove it");
    });
    settle(cx);

    // 1. The list says what is missing and offers the way to fix it. The
    //    sentence has to name both routes, because the button is only one.
    let hint = cx.update(|window, _| {
        window
            .find("empty-hint")
            .label()
            .map(|label| label.to_string())
    });
    let hint = hint.expect("an empty state with no core says so");
    assert!(hint.contains("FP_BROWSER_CHROMIUM_BIN"), "{hint}");
    assert!(hint.contains("Browser Cores"), "{hint}");
    assert_eq!(
        view.read_with(cx, |view, _| view.state().rows().len()),
        0,
        "there is nothing to list yet"
    );

    // 2. The button the empty state offers goes where a core is added.
    cx.update(|window, cx| window.click("empty-add-core", cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        crate::state::Page::Cores
    );

    // 3. A core, through its own form, ending with a detected version.
    cx.update(|window, cx| window.click("new-core", cx));
    settle(cx);
    let editor = view
        .read_with(cx, |view, _| view.core_editor())
        .expect("the Add Core button opens the form");
    let executable = editor.read_with(cx, |editor, _| editor.executable_input());
    cx.update(|window, cx| {
        executable.update(cx, |state, cx| {
            state.set_value(binary.path_buf().to_string_lossy().to_string(), window, cx)
        });
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);
    let rows = view.read_with(cx, |view, _| view.state().core_rows().expect("rows"));
    assert_eq!(rows.len(), 1, "the installation has a core now");
    assert_eq!(rows[0].core.major, 148);

    // 4. Back on the list, the same hint now says something else, and the
    //    button that could not work before is offered.
    cx.update(|window, cx| window.click("nav-Profiles", cx));
    settle(cx);
    let hint = cx
        .update(|window, _| {
            window
                .find("empty-hint")
                .label()
                .map(|label| label.to_string())
        })
        .expect("the list is still empty, and now for the other reason");
    assert!(!hint.contains("FP_BROWSER_CHROMIUM_BIN"), "{hint}");

    // 5. And a profile can be created with the core that was just added.
    cx.update(|window, cx| window.click("new-profile", cx));
    settle(cx);
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);
    let ids: Vec<ProfileId> = view.read_with(cx, |view, _| {
        view.state()
            .rows()
            .iter()
            .map(|row| row.profile.id)
            .collect()
    });
    assert_eq!(ids.len(), 1, "the form was accepted");
    let id = ids[0];
    assert_eq!(
        view.read_with(cx, |view, _| view.state().profile(id).map(|p| p.core_id)),
        Some(rows[0].core.id),
        "the profile was created with the core the walk added"
    );

    // 6. And it starts, which is the end of the path this test is about.
    cx.update(|window, cx| window.click(format!("start-{id}"), cx));
    settle(cx);
    // `Running` rather than `Starting`: the fake façade applies a launch as
    // soon as it is asked, where the real one would publish `Starting` and
    // the browser later. What this asserts is that the click reached it.
    assert_eq!(
        view.read_with(cx, |view, _| view
            .state()
            .rows()
            .first()
            .and_then(|row| row.snapshot.as_ref())
            .map(|snapshot| snapshot.state.clone())),
        Some(domain::RuntimeState::Running),
        "the launch reached the runtime"
    );
    let commands = runtime.commands.lock().expect("command log").clone();
    assert_eq!(
        commands.len(),
        1,
        "one launch was asked for, through the supervisor channel"
    );
    assert!(commands[0].starts_with("start:"), "{commands:?}");
}

#[gpui_kit::test]
fn a_refused_core_form_says_why_and_stays_open(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    // A file that answers nothing when asked for --version.
    let binary = crate::state::testing::CoreBinary::new("ui-silent", None);
    let path = binary.path_buf();

    cx.update(|window, cx| window.click("nav-Cores", cx));
    settle(cx);
    let before = view.read_with(cx, |view, _| view.state().core_rows().expect("rows").len());
    cx.update(|window, cx| window.click("new-core", cx));
    settle(cx);

    let editor = view
        .read_with(cx, |view, _| view.core_editor())
        .expect("a core editor");
    let executable = editor.read_with(cx, |editor, _| editor.executable_input());
    cx.update(|window, cx| {
        executable.update(cx, |state, cx| {
            state.set_value(path.to_string_lossy().to_string(), window, cx)
        });
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("core-executable").is_some()),
        "a refused save leaves the form open"
    );
    assert!(
        cx.update(|window, _| window.try_find("core-error").is_some()),
        "the form says what is wrong"
    );
    let shown = editor
        .read_with(cx, |editor, _| editor.error().map(str::to_string))
        .expect("the reason is recorded on the form");
    assert!(shown.contains("--version"), "{shown}");
    assert!(
        shown.contains("FP_BROWSER_CHROMIUM_MAJOR"),
        "the refusal says how to record it anyway: {shown}"
    );
    assert!(
        shown.split("; ").count() == 2,
        "the message is written as two clauses, one line each: {shown}"
    );
    let notice = view
        .read_with(cx, |view, _| view.state().notice().cloned())
        .expect("the refusal is shown");
    assert!(notice.error, "the refusal is an error");
    assert!(notice.message.contains("--version"), "{}", notice.message);
    assert_eq!(
        view.read_with(cx, |view, _| view.state().core_rows().expect("rows").len()),
        before,
        "nothing was stored"
    );
}

#[gpui_kit::test]
fn deleting_a_core_in_use_is_refused_in_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    // The fixture seeds one core; a profile is created against it.
    seed_profile(cx, &view);
    settle(cx);
    cx.update(|window, cx| window.click("nav-Cores", cx));
    settle(cx);
    cx.update(|window, cx| window.click("delete-core-0", cx));
    settle(cx);
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    assert_eq!(
        view.read_with(cx, |view, _| view.state().core_rows().expect("rows").len()),
        1,
        "the core is still there"
    );
    let notice = view
        .read_with(cx, |view, _| view.state().notice().cloned())
        .expect("the refusal is shown");
    assert!(notice.error);
    assert!(
        notice.message.contains("Profile 1"),
        "the message names the profile holding it: {}",
        notice.message
    );
}

#[gpui_kit::test]
fn re_detecting_from_the_window_reports_what_it_found(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    let binary =
        crate::state::testing::CoreBinary::new("ui-redetect", Some("Chromium 148.0.7778.215"));
    let path = binary.path_buf();

    cx.update(|window, cx| window.click("nav-Cores", cx));
    settle(cx);
    cx.update(|window, cx| window.click("new-core", cx));
    settle(cx);
    let editor = view
        .read_with(cx, |view, _| view.core_editor())
        .expect("a core editor");
    let executable = editor.read_with(cx, |editor, _| editor.executable_input());
    cx.update(|window, cx| {
        executable.update(cx, |state, cx| {
            state.set_value(path.to_string_lossy().to_string(), window, cx)
        });
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    // The binary behind that path is replaced by an older build.
    binary.replace(Some("Chromium 128.0.0.0"));
    // The listing order is not part of the contract, so the button is found
    // by the row it belongs to.
    let index = view
        .read_with(cx, |view, _| view.state().core_rows().expect("rows"))
        .iter()
        .position(|row| row.core.executable == path)
        .expect("the added core is listed");
    cx.update(|window, cx| window.click(format!("redetect-core-{index}"), cx));
    settle(cx);

    let rows = view.read_with(cx, |view, _| view.state().core_rows().expect("rows"));
    let redetected = rows
        .iter()
        .find(|row| row.core.executable == path)
        .expect("the core is listed");
    assert_eq!(redetected.core.major, 128);
    assert_eq!(
        redetected.generation_label().as_deref(),
        Some("Chrome 143 and older · spoofing exclusions not honoured")
    );
}

#[gpui_kit::test]
fn the_sidebar_switches_to_the_settings_page(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    assert!(
        cx.update(|window, _| window.try_find("settings-scroll").is_none()),
        "the settings page is not shown first"
    );

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("setting-data-dir").is_some()),
        "the settings page is shown"
    );
    assert!(
        cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
        "the profiles body is not rendered on the settings page"
    );
    assert!(
        cx.update(|window, _| window.try_find("setting-chromium-bin").is_some()),
        "the read-only rows are listed too"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        crate::state::Page::Settings
    );
}

#[gpui_kit::test]
fn only_the_editable_settings_offer_a_button(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);

    for key in ["xray-executable", "echo-url"] {
        assert!(
            cx.update(|window, _| window.try_find(format!("edit-setting-{key}")).is_some()),
            "{key} can be changed"
        );
    }
    for key in [
        "data-dir",
        "chromium-bin",
        "chromium-major",
        "config-file",
        "runtime-dir",
    ] {
        assert!(
            cx.update(|window, _| window.try_find(format!("edit-setting-{key}")).is_none()),
            "{key} is read-only"
        );
    }
    let _ = view;
}

#[gpui_kit::test]
fn a_setting_can_be_changed_from_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-settings-{}", std::process::id()));
    let config = dir.join("config.json");
    let (view, _runtime) = view_with_config(cx, &config);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    cx.update(|window, cx| window.click("edit-setting-xray-executable", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("setting-field-xray-executable").is_some()),
        "the field is open"
    );
    assert!(
        cx.update(|window, _| window.try_find("setting-xray-executable").is_some()),
        "the row it belongs to is still there"
    );

    let field = view
        .read_with(cx, |view, _| view.setting_editor())
        .expect("the settings field");
    cx.update(|window, cx| {
        field.update(cx, |state, cx| state.set_value("/opt/xray", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    let stored = std::fs::read_to_string(&config).expect("the config file was written");
    assert!(stored.contains("/opt/xray"), "{stored}");
    let row = view.read_with(cx, |view, _| {
        view.state()
            .setting_rows()
            .into_iter()
            .find(|row| row.key == crate::settings::SettingKey::XrayExecutable)
            .expect("the row")
    });
    assert_eq!(row.value, "/opt/xray");
    assert_eq!(row.source, crate::settings::Source::ConfigFile);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Choosing an appearance repaints the window at once and stores the choice.
///
/// This is the one setting on the page whose effect is now rather than at the
/// next start, so it has to be shown to be now: the assertion is on the
/// palette in force, not on the stored value, and on the file as well,
/// because a repaint that is forgotten by the next start is the failure the
/// two halves of the test exist to separate.
#[gpui_kit::test]
fn choosing_an_appearance_repaints_and_is_remembered(cx: &mut TestAppContext) {
    // The component theme is process-wide; see `crate::theme::testing`.
    let _exclusive = crate::theme::testing::exclusive();
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-theme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = dir.join("config.json");
    let (view, _runtime) = view_with_config(cx, &config);
    let cx = window(cx, &view);

    // Named rather than assumed: the library's own default is not this
    // program's default, so the starting point is set to what the window
    // would be showing at boot.
    cx.update(|_, cx| Theme::change(ThemeChoice::Dark.mode(), None, cx));
    cx.update(|_, cx| assert_eq!(palette(cx), Palette::DARK, "the starting palette"));
    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("theme-light").is_some()),
        "the appearance card is on the page"
    );

    cx.update(|window, cx| window.click("theme-light", cx));
    settle(cx);

    cx.update(|_, cx| assert_eq!(palette(cx), Palette::LIGHT, "the window repainted"));
    let stored = std::fs::read_to_string(&config).expect("the config file was written");
    assert!(stored.contains("\"light\""), "{stored}");
    view.read_with(cx, |view, _| {
        assert_eq!(view.state().theme(), ThemeChoice::Light);
    });
    let _ = std::fs::remove_dir_all(&dir);
}

/// The switch repaints the window, and the identifiers it is addressed by do
/// not move with the language: every test that clicks `nav-Settings`, and
/// every script built on top of them, keeps working in either language.
#[gpui_kit::test]
fn choosing_a_language_repaints_and_leaves_the_identifiers_alone(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-language-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = dir.join("config.json");
    let (view, _runtime) = view_with_config(cx, &config);
    let cx = window(cx, &view);
    seed_named_profile(cx, &view, "Work");

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    scroll_settings_to(cx, "language-zh");
    assert!(
        cx.update(|window, _| window.try_find("language-zh").is_some()),
        "the language sits beside the appearance"
    );

    cx.update(|window, cx| window.click("language-zh", cx));
    settle(cx);

    view.read_with(cx, |view, _| {
        assert_eq!(
            view.state().language(),
            crate::text::Lang::Zh,
            "the choice took effect"
        );
    });
    let stored = std::fs::read_to_string(&config).expect("the config file was written");
    assert!(stored.contains("\"zh\""), "{stored}");

    // The frozen identifiers: a language is a way of reading the window, not
    // a second window with its own names.
    assert!(
        cx.update(|window, _| window.try_find("nav-Settings").is_some()),
        "the page identifier did not move"
    );
    assert!(
        cx.update(|window, _| window.try_find("setting-data-dir").is_some()),
        "a setting's row identifier did not move either"
    );

    // A sentence the window computed rather than a label it looked up: the
    // profile count is built from the table on every render.
    cx.update(|window, cx| window.click("nav-Profiles", cx));
    settle(cx);
    assert_eq!(
        cx.update(|window, _| window.find("profile-count").label().map(str::to_string)),
        Some("共 1 个档案。".to_string()),
        "the header counts in the language now in force"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Show whatever the state has queued, the way the tick does.
fn flush_toasts(cx: &mut gpui_kit::VisualTestContext, view: &gpui_kit::Entity<AppView>) {
    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.show_toasts(window, cx));
    });
}

#[gpui_kit::test]
fn an_action_is_acknowledged_with_a_toast(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    flush_toasts(cx, &view);

    assert_eq!(
        cx.update(|window, cx| window.notifications(cx).len()),
        1,
        "creating a profile is acknowledged"
    );

    // A toast is shown once. Draining what was already shown must not
    // duplicate it on the next tick.
    flush_toasts(cx, &view);
    assert_eq!(cx.update(|window, cx| window.notifications(cx).len()), 1);
}

#[gpui_kit::test]
fn a_problem_keeps_the_banner_and_still_gets_a_toast(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    // A value the settings file refuses is a problem and not a success: it
    // stays in the banner and is toasted while it happens.
    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    cx.update(|window, cx| window.click("edit-setting-echo-url", cx));
    settle(cx);
    let field = view
        .read_with(cx, |view, _| view.setting_editor())
        .expect("the settings field");
    cx.update(|window, cx| {
        field.update(cx, |state, cx| state.set_value("   ", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("dismiss-notice").is_some()),
        "a problem stays in the banner until it is dismissed"
    );
    flush_toasts(cx, &view);
    assert!(
        cx.update(|window, cx| window.notifications(cx).len()) >= 1,
        "the problem is also toasted while it happens"
    );
}

#[gpui_kit::test]
fn the_sidebar_switches_to_the_log_page(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    assert!(
        cx.update(|window, _| window.try_find("logs-scroll").is_none()),
        "the log page is not shown first"
    );

    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("logs-scroll").is_some()),
        "the log page is shown"
    );
    assert!(
        cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
        "only one page is rendered at a time"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        crate::state::Page::Log
    );
}

#[gpui_kit::test]
fn the_log_page_lists_what_happened_and_can_be_cleared(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);

    let label = cx
        .update(|window, _| window.find("log-0").label().map(|label| label.to_string()))
        .expect("the newest line is rendered");
    assert!(
        label.contains("Created Profile 1"),
        "the line says what happened: {label}"
    );
    assert!(label.contains("[info]"), "and at what level: {label}");

    cx.update(|window, cx| window.click("clear-log", cx));
    settle(cx);
    assert!(
        view.read_with(cx, |view, _| view.state().log_rows().is_empty()),
        "clearing empties the history"
    );
    assert!(
        cx.update(|window, _| window.try_find("logs-scroll").is_some()),
        "the page stays put with an empty history"
    );
}

#[gpui_kit::test]
fn the_details_panel_switches_between_views(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, runtime) = view(cx);
    let cx = window(cx, &view);

    let id = seed_profile(cx, &view);
    // A launch line to show, and a line of this profile's own history.
    runtime.set_args(
        id,
        vec!["--fingerprint=1".to_string(), "about:blank".to_string()],
    );
    view.update(cx, |view, cx| {
        view.state_mut()
            .record_event(&RuntimeEvent::Stopped { profile_id: id });
        view.state_mut().refresh_runtime();
        cx.notify();
    });
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("details-grid").is_some()),
        "the panel opens on the profile itself"
    );
    assert!(cx.update(|window, _| window.try_find("args-body").is_none()));

    cx.update(|window, cx| window.click("details-tab-args", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("args-body").is_some()),
        "the launch line has a view of its own"
    );
    assert!(
        cx.update(|window, _| window.try_find("details-grid").is_none()),
        "one view at a time: the panel is no longer one long scroll"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view
            .state()
            .selected()
            .map(|row| row.effective_args().len())),
        Some(2)
    );

    cx.update(|window, cx| window.click("details-tab-log", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("panel-log-body").is_some()),
        "the third view is this profile's own history"
    );
    assert!(
        cx.update(|window, _| window.try_find(("panel-log", 0usize)).is_some()),
        "with the newest line in it"
    );
    assert!(cx.update(|window, _| window.try_find("details-grid").is_none()));
    assert_eq!(
        view.read_with(cx, |view, _| view.state().details_tab()),
        crate::state::DetailsTab::Log
    );

    cx.update(|window, cx| window.click("details-tab-details", cx));
    settle(cx);
    assert!(cx.update(|window, _| window.try_find("details-grid").is_some()));
    assert!(cx.update(|window, _| window.try_find("panel-log-body").is_none()));
}

#[gpui_kit::test]
fn the_log_page_filter_can_be_narrowed(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("log-0").is_some()),
        "the created profile is an info line"
    );

    cx.update(|window, cx| window.click("log-filter-errors", cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("log-0").is_none()),
        "an info line is hidden by the error filter"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().log_filter()),
        crate::state::LogFilter::Errors
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().log_len()),
        1,
        "the line is hidden, not dropped"
    );

    cx.update(|window, cx| window.click("log-filter-all", cx));
    settle(cx);
    assert!(cx.update(|window, _| window.try_find("log-0").is_some()));
}

/// The page has to say where the log is being kept, and the file has to
/// really hold what the page shows.
#[gpui_kit::test]
fn the_log_page_names_the_file_it_writes_to(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let log = crate::log_file::LogFile::open(&dir).expect("open the log file");
    let path = log.path().to_path_buf();
    let (view, _runtime, _opener) = view_with_log(
        cx,
        Arc::new(FakeVerifier::passing()),
        None,
        Arc::new(FakeOpener::working()),
        Some(log),
    );
    let cx = window(cx, &view);

    seed_profile(cx, &view);
    cx.update(|window, cx| window.click("nav-Log", cx));
    settle(cx);

    let label = cx
        .update(|window, _| {
            window
                .find("log-file-status")
                .label()
                .map(|label| label.to_string())
        })
        .expect("the page says where the log is");
    assert!(label.contains(&path.display().to_string()), "{label}");

    let text = std::fs::read_to_string(&path).expect("the file was written");
    assert!(text.contains("Created Profile 1"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[gpui_kit::test]
fn a_profile_data_directory_can_be_opened(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let opener = Arc::new(FakeOpener::working());
    let (view, _runtime, opener) =
        view_with_opener(cx, Arc::new(FakeVerifier::passing()), None, opener);
    let cx = window(cx, &view);

    let id = seed_profile(cx, &view);
    let path = view.read_with(cx, |view, _| {
        view.state().profile(id).expect("the profile").user_data_dir
    });

    cx.update(|window, cx| window.click(format!("open-dir-{id}"), cx));
    wait_for_state(cx, &view, |state| {
        state
            .log_rows()
            .iter()
            .any(|row| row.message.contains("Opened"))
    });

    assert_eq!(
        opener.opened(),
        vec![path.clone()],
        "the button opens the data directory of the selected profile"
    );
    let toast = view
        .read_with(cx, |view, _| view.state().toasts().last().cloned())
        .expect("the open is acknowledged");
    assert!(
        toast.message.contains(&path.display().to_string()),
        "{}",
        toast.message
    );
    assert!(
        cx.update(|window, cx| window.notifications(cx).len()) == 0,
        "the click queues a toast; the tick shows it, not the click itself"
    );
}

#[gpui_kit::test]
fn a_failed_open_is_refused_with_the_reason(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let opener = Arc::new(FakeOpener::failing("could not run xdg-open: not found"));
    let (view, _runtime, _opener) =
        view_with_opener(cx, Arc::new(FakeVerifier::passing()), None, opener);
    let cx = window(cx, &view);

    let id = seed_profile(cx, &view);
    cx.update(|window, cx| window.click(format!("open-dir-{id}"), cx));
    wait_for_state(cx, &view, |state| state.notice().is_some());

    let notice = view
        .read_with(cx, |view, _| view.state().notice().cloned())
        .expect("the failure is reported");
    assert!(notice.error);
    assert!(notice.message.contains("xdg-open"), "{}", notice.message);
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

#[gpui_kit::test]
fn a_profile_can_be_edited_from_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let id = seed_profile(cx, &view);

    cx.update(|window, cx| window.click(format!("edit-{id}"), cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("editor-name").is_some()),
        "the editor opens on the profile"
    );

    let editor = view
        .read_with(cx, |view, _| view.editor())
        .expect("an editor");
    let name = editor.read_with(cx, |editor, _| editor.name_input());
    cx.update(|window, cx| {
        name.update(cx, |state, cx| state.set_value("Renamed", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    let row = view.read_with(cx, |view, _| view.state().rows()[0].clone());
    assert_eq!(row.profile.name, "Renamed");
    assert!(
        cx.update(|window, _| window.try_find("editor-name").is_none()),
        "saving closes the dialog"
    );
}

#[gpui_kit::test]
fn the_filter_narrows_the_profile_list(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let work = seed_named_profile(cx, &view, "Work laptop");
    let shopping = seed_named_profile(cx, &view, "Shopping");
    settle(cx);
    assert_eq!(
        cx.update(|window, _| window.find("profile-count").label().map(str::to_string)),
        Some("2 profiles.".to_string()),
        "without a filter the header says how many there are"
    );

    type_filter(cx, &view, "work");

    assert!(
        cx.update(|window, _| window.try_find(format!("profile-{work}")).is_some()),
        "the profile that matches is listed"
    );
    assert!(
        cx.update(|window, _| window.try_find(format!("profile-{shopping}")).is_none()),
        "the one that does not match is not"
    );
    assert_eq!(
        cx.update(|window, _| window.find("profile-count").label().map(str::to_string)),
        Some("Showing 1 of 2 profiles, filtered by \"work\".".to_string()),
        "the header says the list is not the whole list"
    );
}

#[gpui_kit::test]
fn a_filter_that_matches_nothing_offers_the_way_back(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let id = seed_named_profile(cx, &view, "Work laptop");
    settle(cx);

    type_filter(cx, &view, "nothing is called this");

    assert!(
        cx.update(|window, _| window.try_find(format!("profile-{id}")).is_none()),
        "the list is empty"
    );
    assert!(
        cx.update(|window, _| window.try_find("clear-filter").is_some()),
        "an empty list says which kind of empty it is, and offers the way out"
    );

    cx.update(|window, cx| window.click("clear-filter", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find(format!("profile-{id}")).is_some()),
        "clearing brings the profile back"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().profile_filter().to_string()),
        "",
        "the list is unfiltered again"
    );
    let field = view
        .read_with(cx, |view, _| view.filter_input())
        .expect("the field is built by now");
    assert_eq!(
        field.read_with(cx, |state, _| state.value().to_string()),
        "",
        "the field was emptied too: setting a value emits no change event"
    );
    assert!(
        cx.update(|window, _| window.try_find("clear-filter").is_none()),
        "and the hint goes with it"
    );
}

#[gpui_kit::test]
fn the_filter_survives_a_trip_to_another_page(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let work = seed_named_profile(cx, &view, "Work laptop");
    seed_named_profile(cx, &view, "Shopping");
    settle(cx);

    type_filter(cx, &view, "work");

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| window.click("nav-Profiles", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find(format!("profile-{work}")).is_some()),
        "the filter is still narrowing the list"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().visible_rows().len()),
        1
    );
    let field = view
        .read_with(cx, |view, _| view.filter_input())
        .expect("the field outlives the page it was built for");
    assert_eq!(
        field.read_with(cx, |state, _| state.value().to_string()),
        "work",
        "and it still shows the term"
    );
}

#[gpui_kit::test]
fn a_refused_edit_keeps_the_dialog_open_and_shows_why(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let id = seed_profile(cx, &view);
    let name_before = view.read_with(cx, |view, _| view.state().rows()[0].profile.name.clone());

    cx.update(|window, cx| window.click(format!("edit-{id}"), cx));
    settle(cx);

    let editor = view
        .read_with(cx, |view, _| view.editor())
        .expect("an editor");
    let seed = editor.read_with(cx, |editor, _| editor.seed_input());
    cx.update(|window, cx| {
        seed.update(cx, |state, cx| state.set_value("not a number", window, cx));
    });
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("editor-name").is_some()),
        "a refused save leaves the form open"
    );
    assert!(
        cx.update(|window, _| window.try_find("editor-error").is_some()),
        "the form says what is wrong"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().rows()[0].profile.name.clone()),
        name_before,
        "nothing was written"
    );
}

#[gpui_kit::test]
fn duplicating_adds_a_profile_and_deleting_removes_one(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let id = seed_profile(cx, &view);

    cx.update(|window, cx| window.click(format!("duplicate-{id}"), cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.state().rows().len()),
        2,
        "the copy is listed"
    );
    let copy = view
        .read_with(cx, |view, _| view.state().selected_id())
        .expect("selected");
    assert_ne!(copy, id);

    cx.update(|window, cx| window.click(format!("delete-{copy}"), cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("ok").is_some()),
        "deleting asks first"
    );

    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.state().rows().len()),
        1,
        "only the copy was removed"
    );
    assert!(
        view.read_with(cx, |view, _| view.state().profile(id).is_some()),
        "the original is untouched"
    );
}

#[gpui_kit::test]
fn starting_without_a_core_says_so_and_offers_the_way_there(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let profile_repo: Arc<MemProfileRepository> = Arc::new(MemProfileRepository::new());
    let core_repo: Arc<MemCoreRepository> = Arc::new(MemCoreRepository::new());
    let proxy_repo: Arc<MemProxyRepository> = Arc::new(MemProxyRepository::new());
    let runtime = Arc::new(FakeRuntime::new());

    let profiles = Arc::new(DefaultProfileService::new(
        profile_repo.clone(),
        PathBuf::from("data"),
    ));
    let runtime_service = Arc::new(RuntimeService::new(
        profile_repo.clone(),
        core_repo.clone(),
        proxy_repo.clone(),
        runtime,
    ));
    let cores = crate::state::testing::core_service(core_repo.clone(), profile_repo.clone());
    let configuration = Arc::new(storage::MemConfiguration::new(
        core_repo.clone(),
        proxy_repo.clone(),
        profile_repo.clone(),
    ));
    let proxies: Arc<dyn ProxyService> =
        Arc::new(DefaultProxyService::new(proxy_repo, profile_repo));
    let settings = crate::state::testing::settings();
    let state = AppState::for_test(
        crate::state::Services {
            profiles,
            runtime: runtime_service,
            cores,
            proxies,
            configuration,
        },
        settings,
    );
    let (_command_tx, event_rx) = crossbeam_channel::bounded(16);
    let view = cx.new(|cx| {
        let mut view = AppView::new(
            state,
            event_rx,
            Arc::new(FakeVerifier::passing()),
            Arc::new(FakeProxyTester::passing()),
            Arc::new(FakeOpener::working()),
            Arc::new(FakeBrowserDataCopier::passing()),
        );
        view.boot(cx);
        view
    });

    let handle = cx.open_window(size(px(900.), px(600.)), |window, cx| {
        Root::new(view.clone(), window, cx)
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        // The situation is legible where it matters, without a click: the
        // list says there is no core, it names both of the ways to supply
        // one, and the button that would open a form the service refuses is
        // disabled rather than describing the refusal after it happens.
        let hint = window
            .find("empty-hint")
            .label()
            .map(str::to_string)
            .expect("the empty state says what is missing");
        assert!(hint.contains("No browser core yet"), "{hint}");
        assert!(hint.contains("Browser Cores"), "{hint}");
        assert!(hint.contains("FP_BROWSER_CHROMIUM_BIN"), "{hint}");
        window.click("new-profile", cx);
        assert!(
            window.try_find("editor-name").is_none(),
            "a profile needs a core to launch, so the form does not open"
        );

        // And the one thing the reader can do about it is one click away,
        // on a page that is not the one they are looking at.
        assert!(
            window.try_find("empty-add-core").is_some(),
            "the empty state carries the way to the fix"
        );
    })
    .unwrap();

    // The button is wired to the page switch; the state is what says it
    // worked, because a click on a button inside a scrolled container is a
    // question about the harness rather than about this program.
    view.update(cx, |view, cx| view.on_page(crate::state::Page::Cores, cx));
    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        crate::state::Page::Cores
    );
}

#[gpui_kit::test]
fn exporting_from_the_settings_page_writes_the_file_and_says_what_it_did(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let dir = std::env::temp_dir().join(format!("fp-ui-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("config.json");

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    // The export card is below the fold of the test window.
    scroll_settings_to(cx, "export-run");
    // The field exists only after the page has been rendered, which is where
    // an `InputState` gets the window it needs.
    type_export_path(cx, &view, path.to_string_lossy().as_ref());
    cx.update(|window, cx| window.click("export-run", cx));
    // The write happens on a worker: the answer arrives on a later tick, and
    // the message below is the proof that it did.
    wait_for_state(cx, &view, |state| {
        state
            .toasts()
            .last()
            .is_some_and(|toast| toast.message.contains("config.json"))
    });

    assert!(path.exists(), "{} should exist", path.display());
    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(text.contains("fp-browser/config-backup"), "{text}");

    // This fixture has no proxies, so the sentence is the one about there
    // having been nothing to leave out - not a claim about credentials that
    // were not there.
    let message = last_message(cx, &view);
    assert!(message.contains("config.json"), "{message}");
    assert!(
        message.contains("no proxy credentials to leave out"),
        "{message}"
    );

    std::fs::remove_dir_all(&dir).expect("clean up");
}

/// The card's whole job: one press, one file, and the window says where.
///
/// The file it looks for is the one the window says it wrote, not the one the
/// card named before the press. The name carries the second, and the card
/// computes it while it is being drawn, so a press that lands in the next
/// second writes a different name - which is not worth a mechanism in the
/// card, but is worth not asserting on in a test that runs for hours next to
/// a hundred others.
#[gpui_kit::test]
fn writing_a_diagnostics_report_from_the_settings_page_says_where_it_went(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    scroll_settings_to(cx, "diagnostics-run");
    // The folder is the card's claim that does not move: one report per
    // press, in `diagnostics` under the data directory.
    let folder = view
        .read_with(cx, |view, _| view.state().diagnostics_destination())
        .parent()
        .expect("a diagnostics folder")
        .to_path_buf();
    let _ = std::fs::remove_dir_all(&folder);

    cx.update(|window, cx| window.click("diagnostics-run", cx));
    settle(cx);

    let written: Vec<PathBuf> = std::fs::read_dir(&folder)
        .expect("the card's folder exists")
        .map(|entry| entry.expect("an entry").path())
        .collect();
    assert_eq!(written.len(), 1, "one press writes one report");
    let written = &written[0];
    assert!(
        written
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .starts_with("fp-browser-diagnostics-"),
        "{}",
        written.display()
    );

    let text = std::fs::read_to_string(written).expect("read back");
    assert!(text.contains(crate::version::VERSION), "{text}");
    assert!(text.contains("Fingerprint Browser diagnostics"), "{text}");

    let message = last_message(cx, &view);
    assert!(
        message.contains(&written.display().to_string()),
        "the window names the file it wrote: {message}"
    );

    // The report is the only thing this test leaves, and it leaves nothing:
    // the fixture's data directory is shared with the rest of the suite.
    std::fs::remove_file(written).expect("clean up");
    let _ = std::fs::remove_dir(&folder);
}

#[gpui_kit::test]
fn the_export_card_offers_the_choice_about_credentials(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);

    // Left out by default, because a file carrying plain-text passwords is
    // the thing to choose deliberately.
    assert!(!view.read_with(cx, |view, _| view.state().export_includes_credentials()));

    // And the line under the field names the file an empty field would
    // write, so the default is shown rather than described.
    let shown = view
        .read_with(cx, |view, _| view.state().export_destination())
        .display()
        .to_string();
    assert!(shown.ends_with(".json"), "{shown}");
    assert!(shown.contains("exports"), "{shown}");
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

/// The Settings cards, in the order they appear, for the scroll helper to aim
/// at once the setting rows above them have scrolled away.
const SETTINGS_CARDS: [&str; 6] = [
    "appearance",
    "export-configuration",
    "import-configuration",
    "restore-configuration",
    "browser-data",
    "diagnostics",
];

#[gpui_kit::test]
fn importing_from_the_settings_page_reads_the_file_and_says_what_it_did(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    // The file to import is written the honest way: through the same
    // document the export writes, for an installation that holds nothing.
    // The state-level tests cover a full round trip; what is under test
    // here is the wiring from the card to the use case.
    let dir = std::env::temp_dir().join(format!("fp-ui-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("config.json");
    let snapshot = application::ConfigSnapshot {
        cores: Vec::new(),
        proxies: Vec::new(),
        profiles: Vec::new(),
    };
    let document = application::ConfigBackup::build(
        snapshot,
        application::Credentials::Excluded,
        application::ExportOrigin {
            exported_at: "2026-09-21T00:00:00Z".to_string(),
            source_data_dir: dir.display().to_string(),
        },
    );
    std::fs::write(&path, document.to_json().expect("serialise")).expect("write");

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    type_import_path(cx, &view, path.to_string_lossy().as_ref());
    scroll_settings_to(cx, "import-run");
    cx.update(|window, cx| window.click("import-run", cx));
    // The read and the writes happen on a worker, so the answer arrives on a
    // later tick rather than inside the click.
    wait_for_state(cx, &view, |state| {
        state
            .toasts()
            .last()
            .is_some_and(|toast| toast.message.contains("config.json"))
    });

    // An empty installation importing an empty file is the quiet case: a
    // toast that says so, not a banner that pretends something happened.
    let message = last_message(cx, &view);
    assert!(message.contains("config.json"), "{message}");
    assert!(message.contains("nothing was added"), "{message}");
    assert!(
        view.read_with(cx, |view, _| view.state().notice().is_none()),
        "the quiet case does not sit in the banner"
    );

    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[gpui_kit::test]
fn importing_without_a_path_is_refused_in_the_banner(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    scroll_settings_to(cx, "import-run");
    cx.update(|window, cx| window.click("import-run", cx));
    settle(cx);

    // Refused where the field is, and stayed to be dismissed - the same
    // treatment every other refusal in the window gets.
    let notice = view.read_with(cx, |view, _| view.state().notice().cloned());
    let notice = notice.expect("the refusal is shown");
    assert!(notice.error, "{:?}", notice.message);
    assert!(
        notice.message.contains("Type the path"),
        "{:?}",
        notice.message
    );
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

#[gpui_kit::test]
fn restoring_without_a_path_is_refused_in_the_banner(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    // The restore card sits below the import one; the same downward scroll
    // brings it into view.
    scroll_settings_to(cx, "restore-run");
    cx.update(|window, cx| window.click("restore-run", cx));
    settle(cx);

    // An empty field is refused where it is, the same way an import is.
    let notice = view.read_with(cx, |view, _| view.state().notice().cloned());
    let notice = notice.expect("the refusal is shown");
    assert!(notice.error, "{:?}", notice.message);
    assert!(
        notice.message.contains("Type the path"),
        "{:?}",
        notice.message
    );
}

/// A populated installation is not replaced without being asked: the button
/// opens a confirmation, and only the confirmation runs the replacement.
#[gpui_kit::test]
fn restoring_over_a_populated_installation_asks_before_replacing(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    // The fixture seeds a core, so there is already something to replace.
    seed_profile(cx, &view);

    // A file to restore, written through the document the export writes.
    let dir = std::env::temp_dir().join(format!("fp-ui-restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let path = dir.join("config.json");
    let document = application::ConfigBackup::build(
        application::ConfigSnapshot::default(),
        application::Credentials::Excluded,
        application::ExportOrigin {
            exported_at: "2026-09-21T00:00:00Z".to_string(),
            source_data_dir: dir.display().to_string(),
        },
    );
    std::fs::write(&path, document.to_json().expect("serialise")).expect("write");

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    type_restore_path(cx, &view, path.to_string_lossy().as_ref());
    scroll_settings_to(cx, "restore-run");
    cx.update(|window, cx| window.click("restore-run", cx));
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("ok").is_some()),
        "restoring a populated installation asks first"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().rows().len()),
        1,
        "nothing is replaced before the confirmation"
    );

    cx.update(|window, cx| window.click("ok", cx));
    // The replacement is a transaction over the whole configuration, and it
    // runs on a worker: the rows change when the answer arrives.
    wait_for_state(cx, &view, |state| state.rows().is_empty());

    // The file held nothing, so the replacement took what was here away.
    assert_eq!(
        view.read_with(cx, |view, _| view.state().rows().len()),
        0,
        "the confirmed restore replaced the configuration"
    );
    let message = last_message(cx, &view);
    assert!(message.contains("nothing was added"), "{message}");
    assert!(message.contains("were replaced"), "{message}");

    std::fs::remove_dir_all(&dir).ok();
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

/// Pressing Start on a proxied profile asks the proxy first, and a proxy that
/// carries nothing means the browser is never launched.
///
/// This is the whole point of the gate at the level the user meets it: the
/// window must not be able to produce a browser whose traffic has nowhere to
/// go, and the sentence has to say which link of the chain failed.
#[gpui_kit::test]
fn starting_a_proxied_profile_is_refused_when_its_proxy_has_no_traffic(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let tester = Arc::new(FakeProxyTester::with_outcome(Err(Fault::new(
        FaultClass::Unreachable,
        "no route to the upstream",
    ))));
    let (view, runtime, _opener, tester, _copier) = view_with_tester(
        cx,
        Arc::new(FakeVerifier::passing()),
        tester,
        Arc::new(FakeBrowserDataCopier::passing()),
        None,
        Arc::new(FakeOpener::working()),
        None,
    );
    let cx = window(cx, &view);
    let (id, proxy_id) = seed_proxied_profile(cx, &view);

    cx.update(|window, cx| window.click(format!("start-{id}"), cx));
    wait_for_state(cx, &view, |state| {
        state
            .proxy_test(proxy_id)
            .is_some_and(|test| !test.is_running())
    });

    assert_eq!(tester.calls(), 1, "the proxy was asked once");
    assert!(
        runtime.commands.lock().expect("commands").is_empty(),
        "the browser was not launched"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().row(id).expect("the row").state()),
        RuntimeState::Stopped,
        "and the profile is not left reading as starting"
    );
    let message = last_message(cx, &view);
    assert!(message.contains("Profile 1"), "{message}");
    assert!(message.contains("Office"), "{message}");
    assert!(message.contains("no route to the upstream"), "{message}");
}

/// The same press, with a proxy that answers: the browser starts, and the
/// window says where the traffic it will use leaves from.
#[gpui_kit::test]
fn starting_a_proxied_profile_goes_ahead_once_its_proxy_answers(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let tester = Arc::new(FakeProxyTester::passing_from("198.51.100.9"));
    let (view, runtime, _opener, tester, _copier) = view_with_tester(
        cx,
        Arc::new(FakeVerifier::passing()),
        tester,
        Arc::new(FakeBrowserDataCopier::passing()),
        None,
        Arc::new(FakeOpener::working()),
        None,
    );
    let cx = window(cx, &view);
    let (id, _proxy_id) = seed_proxied_profile(cx, &view);

    cx.update(|window, cx| window.click(format!("start-{id}"), cx));
    wait_for_state(cx, &view, |state| {
        state
            .row(id)
            .is_some_and(|row| row.state() == RuntimeState::Running)
    });

    assert_eq!(tester.calls(), 1, "the proxy was asked once");
    assert_eq!(
        runtime.commands.lock().expect("commands").clone(),
        [format!("start:{id}")]
    );
    let message = last_message(cx, &view);
    assert!(
        message.contains("Started Profile 1 through Office"),
        "{message}"
    );
    assert!(message.contains("198.51.100.9"), "{message}");
}

/// A profile without a proxy is not slowed down or gated by any of this: the
/// request a check would send is the one the browser makes for its own first
/// page.
#[gpui_kit::test]
fn starting_a_profile_without_a_proxy_asks_nothing(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let tester = Arc::new(FakeProxyTester::passing());
    let (view, runtime, _opener, tester, _copier) = view_with_tester(
        cx,
        Arc::new(FakeVerifier::passing()),
        tester,
        Arc::new(FakeBrowserDataCopier::passing()),
        None,
        Arc::new(FakeOpener::working()),
        None,
    );
    let cx = window(cx, &view);
    let id = seed_profile(cx, &view);

    cx.update(|window, cx| window.click(format!("start-{id}"), cx));
    settle(cx);

    assert_eq!(tester.calls(), 0, "no proxy, nothing to ask");
    assert_eq!(
        runtime.commands.lock().expect("commands").clone(),
        [format!("start:{id}")]
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().row(id).expect("the row").state()),
        RuntimeState::Running
    );
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

/// A browser-data copy is hundreds of megabytes, so it is handed to a
/// worker; what the card has to get right is the job it is handed - the
/// direction, the directory and the profiles - and the sentence it reports.
#[gpui_kit::test]
fn a_browser_data_copy_from_the_window_asks_the_worker_and_says_what_it_did(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_kit::init);
    let copier = Arc::new(FakeBrowserDataCopier::passing());
    let (view, _runtime, copier) = view_with_browser_data(cx, copier);
    let cx = window(cx, &view);
    seed_profile(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    let field = view
        .read_with(cx, |view, _| view.browser_data_input())
        .expect("the Settings page builds the browser-data field");
    cx.update(|window, cx| {
        field.update(cx, |state, cx| {
            state.set_value("/backups/browser-data", window, cx)
        });
    });
    settle(cx);

    scroll_settings_to(cx, "browser-data-out");
    cx.update(|window, cx| window.click("browser-data-out", cx));
    wait_for_state(cx, &view, |state| {
        state
            .toasts()
            .iter()
            .any(|toast| toast.message.contains("Copied"))
    });

    // The worker was handed the direction and the directory the field held,
    // and every profile on the list.
    let jobs = copier.jobs();
    assert_eq!(copier.calls(), 1);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].direction, Direction::ToBackup);
    assert_eq!(jobs[0].directory, PathBuf::from("/backups/browser-data"));
    assert_eq!(jobs[0].profiles.len(), 1);
    assert!(jobs[0].running.is_empty());

    let message = last_message(cx, &view);
    assert!(message.contains("Copied the browser data"), "{message}");
    assert!(message.contains("/backups/browser-data"), "{message}");
}

/// A copy in flight is a state the window can be asked about. The lease the
/// job carries is held by the worker, so while the first copy is still running
/// a second press and a start are both refused - and both work again once it
/// has finished. The worker is a fake this test holds open, which is what
/// makes "still running" something to assert against rather than to race.
#[gpui_kit::test]
fn a_copy_in_flight_refuses_a_second_copy_and_a_start(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (copier, pause) = FakeBrowserDataCopier::paused();
    let (view, runtime, copier) = view_with_browser_data(cx, Arc::new(copier));
    let cx = window(cx, &view);
    let id = seed_profile(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    let field = view
        .read_with(cx, |view, _| view.browser_data_input())
        .expect("the Settings page builds the browser-data field");
    cx.update(|window, cx| {
        field.update(cx, |state, cx| {
            state.set_value("/backups/browser-data", window, cx)
        });
    });
    settle(cx);

    // The first copy starts and stops inside the worker.
    scroll_settings_to(cx, "browser-data-out");
    cx.update(|window, cx| window.click("browser-data-out", cx));
    pause.wait_until_copying();

    // A second press is refused, and starts no worker of its own.
    cx.update(|window, cx| window.click("browser-data-out", cx));
    settle(cx);
    let message = last_message(cx, &view);
    assert!(message.contains("try again"), "{message}");
    assert_eq!(copier.calls(), 1, "the refused press started no worker");

    // So is starting one of the profiles the copy is holding.
    cx.update(|window, cx| window.click("nav-Profiles", cx));
    settle(cx);
    cx.update(|window, cx| window.click(format!("start-{id}"), cx));
    settle(cx);
    let message = last_message(cx, &view);
    assert!(message.contains("try again"), "{message}");
    assert!(
        runtime.commands.lock().expect("commands").is_empty(),
        "the refused start never reached the runtime"
    );

    // The copy finishes. Everything it held is free, so the next one runs.
    pause.release();
    wait_for_state(cx, &view, |state| {
        state
            .toasts()
            .iter()
            .any(|toast| toast.message.contains("Copied"))
    });
    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    scroll_settings_to(cx, "browser-data-out");
    cx.update(|window, cx| window.click("browser-data-out", cx));
    pause.wait_until_copying();
    assert_eq!(copier.calls(), 2, "the copy after it finished is allowed");
    pause.release();
}

#[gpui_kit::test]
fn a_browser_data_copy_without_a_directory_is_refused_in_the_banner(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let copier = Arc::new(FakeBrowserDataCopier::passing());
    let (view, _runtime, copier) = view_with_browser_data(cx, copier);
    let cx = window(cx, &view);
    seed_profile(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    scroll_settings_to(cx, "browser-data-out");
    cx.update(|window, cx| window.click("browser-data-out", cx));
    settle(cx);

    let notice = view.read_with(cx, |view, _| view.state().notice().cloned());
    let notice = notice.expect("the refusal is shown");
    assert!(notice.error, "{:?}", notice.message);
    assert!(
        notice.message.contains("Type the directory"),
        "{:?}",
        notice.message
    );
    assert_eq!(
        copier.calls(),
        0,
        "no worker is started without a directory"
    );
}
