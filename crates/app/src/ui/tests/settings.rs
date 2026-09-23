use super::*;

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

    // The chrome is the sidebar now. The exit button that used to sit in a
    // header above every page is gone: the window's own close control and the
    // tray answer a close, and the rare variants are behind the brand's menu -
    // asserted below, so a shell with no way out cannot pass this test.
    assert!(
        cx.update(|window, _| window.try_find("brand-more").is_some()),
        "the sidebar carries the low-frequency menu"
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

/// The sidebar folds to a rail of marks, and open again, with the choice
/// written down.
///
/// The rail is the same five rows with their names left out: the marks are the
/// navigation, and the name each mark stands for is still on the row as its
/// accessible name - which is what a screen reader reads, and what a pointer
/// gets on hover. The switch writes before the window moves, so a config file
/// that cannot be written leaves the sidebar where it was rather than drawing
/// one that would be back to its old width by the next start.
#[gpui_kit::test]
fn the_sidebar_folds_to_a_rail_and_opens_again(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-sidebar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let config = dir.join("config.json");
    let (view, _runtime) = view_with_config(cx, &config);
    let cx = window(cx, &view);

    let width = |cx: &mut gpui_kit::VisualTestContext| {
        cx.update(|window, _| window.find("sidebar").bounds().size.width)
    };
    let name = |cx: &mut gpui_kit::VisualTestContext, page: crate::state::Page| {
        cx.update(|window, _| {
            window
                .find(format!("nav-{}", page.id()))
                .label()
                .map(str::to_string)
        })
    };

    assert_eq!(
        width(cx),
        px(crate::ui::components::SIDEBAR),
        "the sidebar opens as designed"
    );
    assert!(
        cx.update(|window, _| window.try_find("nav-label-Profiles").is_some()),
        "and it names its pages"
    );

    cx.update(|window, cx| window.click("sidebar-toggle", cx));
    settle(cx);

    assert_eq!(
        width(cx),
        px(crate::ui::components::SIDEBAR_RAIL),
        "the rail is as wide as its marks"
    );
    assert!(
        cx.update(|window, _| window.try_find("nav-label-Profiles").is_none()),
        "the names are put away"
    );
    assert!(
        cx.update(|window, _| window.try_find("nav-Profiles").is_some()),
        "the marks are not"
    );
    assert_eq!(
        name(cx, crate::state::Page::Profiles).as_deref(),
        Some("Profiles"),
        "the row still says which page it is, for a screen reader and a hover"
    );
    assert_eq!(
        name(cx, crate::state::Page::Settings).as_deref(),
        Some("Settings"),
        "on every row, not only the first"
    );
    assert!(
        std::fs::read_to_string(&config)
            .expect("the switch wrote the config file")
            .contains("collapsed"),
        "the choice is written before the window moves"
    );

    // And back: one control for both directions.
    cx.update(|window, cx| window.click("sidebar-toggle", cx));
    settle(cx);
    assert_eq!(
        width(cx),
        px(crate::ui::components::SIDEBAR),
        "the labels come back with the width"
    );
    assert!(
        cx.update(|window, _| window.try_find("nav-label-Profiles").is_some()),
        "and the names with them"
    );
    assert!(
        !std::fs::read_to_string(&config)
            .expect("the switch wrote the config file")
            .contains("collapsed"),
        "expanding leaves the file with one spelling of expanded"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The rail's switch is the mark until the pointer arrives.
///
/// A rail has room for one control, so the mark is the control: hovering the
/// head draws the switch where the mark was. The button exists either way - one
/// that appeared only under a pointer would be one the keyboard could never
/// reach - so what is pinned here is what is *seen*, which is the whole of the
/// difference between the two states.
#[gpui_kit::test]
fn the_rail_shows_the_switch_only_under_the_pointer(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-rail-hover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::write(dir.join("config.json"), r#"{"sidebar": "collapsed"}"#).expect("temp config");
    let (view, _runtime) = view_with_config(cx, &dir.join("config.json"));
    let cx = window(cx, &view);

    let drawn = |cx: &mut gpui_kit::VisualTestContext, id: &'static str| {
        cx.update(|window, _| window.find(id).visible())
    };

    assert!(drawn(cx, "sidebar-mark"), "the mark is what the rail shows");

    // The head is the window's top-left corner, which is where a test's pointer
    // starts: it has to be taken away before there is anything to see.
    cx.update(|window, cx| window.hover("nav-row-Profiles", cx));
    cx.update(|window, cx| window.render_frame(cx));
    assert!(
        !drawn(cx, "sidebar-switch"),
        "and the switch is not drawn while the pointer is away"
    );

    cx.update(|window, cx| window.hover("sidebar-head", cx));
    cx.update(|window, cx| window.render_frame(cx));
    assert!(
        drawn(cx, "sidebar-switch"),
        "the pointer over the head brings the switch out"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Every row of the navigation starts at the same left edge.
///
/// The icon and the name are drawn by the row rather than handed to the button's
/// own `icon` and `label` slots, because the library centres those: a centred row
/// puts the long name further left than the short one, and a reader looking down
/// the sidebar finds every page's mark at a different place. This pins the two
/// columns a list read downwards is built from.
#[gpui_kit::test]
fn every_sidebar_row_starts_at_the_same_left_edge(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let bounds = |cx: &mut gpui_kit::VisualTestContext, id: String| {
        cx.update(move |window, _| window.find(id).bounds())
    };
    let pages = [
        crate::state::Page::Profiles,
        crate::state::Page::Proxies,
        crate::state::Page::Cores,
        crate::state::Page::Log,
        crate::state::Page::Settings,
    ];
    let rows: Vec<_> = pages
        .iter()
        .map(|page| bounds(cx, format!("nav-row-{}", page.id())).left())
        .collect();
    let labels: Vec<_> = pages
        .iter()
        .map(|page| bounds(cx, format!("nav-label-{}", page.id())).left())
        .collect();

    assert_eq!(rows.len(), 5, "the five destinations are one list");
    assert!(
        rows.iter().all(|left| *left == rows[0]),
        "every row starts at the same left edge: {rows:?}"
    );
    assert!(
        labels.iter().all(|left| *left == labels[0]),
        "every name sits in one column under the icons: {labels:?}"
    );
    // And the column is at the left of the row, not in the middle of it: the
    // name follows the icon, and starts well before the row's own midpoint.
    let row = bounds(cx, "nav-row-Profiles".to_string());
    assert!(
        labels[0] > row.left() && labels[0] - row.left() < row.size.width / 2.0,
        "the mark and the name are at the row's left edge: {row:?} against {labels:?}"
    );
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
        redetected.capability_parts(),
        Some(("Chrome 143 and older".to_string(), false)),
        "the re-detected core moved to the generation that ignores exclusions"
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
        cx.update(|window, _| window.try_find("appearance").is_some()),
        "the settings page opens on its first group"
    );
    assert!(
        cx.update(|window, _| window.try_find("profiles-scroll").is_none()),
        "the profiles body is not rendered on the settings page"
    );
    assert!(
        cx.update(|window, _| window.try_find("setting-data-dir").is_none()),
        "and only that group: a row of another one is not on screen"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        crate::state::Page::Settings
    );

    // Every group is a chip, and choosing one shows the rows that belong to it.
    for group in crate::settings::SettingGroup::ALL {
        open_settings_group(cx, group);
        assert_eq!(
            view.read_with(cx, |view, _| view.state().settings_group()),
            group,
            "the chip selects its own group"
        );
        assert!(
            cx.update(|window, _| window.try_find("settings-group-note").is_some()),
            "the group says what it holds"
        );
    }
    open_settings_group(cx, crate::settings::SettingGroup::Data);
    assert!(
        cx.update(|window, _| window.try_find("setting-data-dir").is_some()),
        "the data group holds the installation's own paths"
    );
    open_settings_group(cx, crate::settings::SettingGroup::Runtime);
    assert!(
        cx.update(|window, _| window.try_find("setting-chromium-bin").is_some()),
        "the runtime group holds the read-only rows too"
    );
}

#[gpui_kit::test]
fn only_the_editable_settings_offer_a_button(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);

    // Every row, in whichever group it lives: the group has to be shown before
    // the row can be asked about, and the page's own partition is what says
    // which - a row that belonged to no group would never be reached here.
    for key in crate::settings::SettingKey::ALL {
        open_settings_group(cx, crate::settings::SettingGroup::of(key));
        let found = cx.update(|window, _| {
            window
                .try_find(format!("edit-setting-{}", key.id()))
                .is_some()
        });
        assert_eq!(
            found,
            key.editable(),
            "{} is {}",
            key.id(),
            if key.editable() {
                "editable and offers a button"
            } else {
                "read-only and offers none"
            }
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
    open_settings_group(cx, crate::settings::SettingGroup::Runtime);
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
    open_settings_group(cx, crate::settings::SettingGroup::Data);
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

#[gpui_kit::test]
fn a_profile_can_be_edited_from_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    let id = seed_profile(cx, &view);

    profile_menu(cx, id, ProfileMenu::Edit);
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

/// The shell's exit affordance, where the removed header's button went.
///
/// Nothing here is a new exit *path*: the menu asks the same question the
/// window's close control does, which is what stops the two from drifting into
/// two different answers.
#[gpui_kit::test]
fn the_brand_menu_carries_the_ways_out(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    // A config file of this test's own: a close remembered by another test
    // would answer the question this one asks.
    let dir = std::env::temp_dir().join(format!("fp-ui-brand-menu-{}", std::process::id()));
    let (view, _) = view_with_config(cx, &dir.join("config.json"));
    let cx = window(cx, &view);

    assert!(
        cx.update(|window, _| window.try_find("quit").is_none()),
        "the content area no longer repeats the window's close control"
    );

    cx.update(|window, cx| window.click("brand-more", cx));
    assert!(
        cx.update(|window, _| window.try_find("popup-menu").is_some()),
        "the menu opens from the brand"
    );

    // Items are picked by their place in the menu: about, a separator, closing
    // the window, then stopping everything.
    cx.update(|window, cx| window.within("popup-menu").click(2usize, cx));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("exit-choice-exit-all").is_some()),
        "closing from the menu asks the question the window's control asks"
    );
}

/// A desktop that will not take a tray icon keeps the window on screen.
///
/// Without the icon there is no way back to a hidden window, so "keep running"
/// would leave a program the user can neither see nor reach. The close is still
/// refused - the program does keep running - and the banner, which is readable
/// precisely because the window stayed, says why.
#[gpui_kit::test]
fn a_desktop_without_a_tray_keeps_the_window(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-no-tray-{}", std::process::id()));
    let (view, _) = view_with_config(cx, &dir.join("config.json"));
    let cx = window(cx, &view);
    view.update(cx, |view, _| {
        view.state_mut()
            .set_exit_mode(ExitMode::Background)
            .expect("store the answer");
        view.set_tray_starter(|_| Err("no StatusNotifierItem host".to_string()));
    });

    let allowed =
        cx.update(|window, cx| view.update(cx, |view, cx| view.on_close_requested(window, cx)));
    assert!(
        !allowed,
        "the program keeps running, so the close is still refused"
    );
    view.update(cx, |view, _| {
        assert!(
            view.tray.is_none(),
            "there is no icon, which is the whole problem"
        );
        let notice = view
            .state()
            .notice()
            .expect("the refusal to hide is reported where it can be read");
        assert!(notice.error, "it is a problem, not a note: {notice:?}");
        assert!(
            notice.message.contains("StatusNotifierItem"),
            "the banner carries the desktop's own reason: {}",
            notice.message
        );
    });
}

/// The close question says what each answer costs with the profiles that are
/// running right now.
///
/// The three answers are one word apart and opposite in effect, and a reader
/// looking at a browser already up has to be able to tell which one stops it.
/// The count is not a decoration: "leave them running" is the same sentence
/// whether nothing or six are up, and only one of those is worth reading twice.
#[gpui_kit::test]
fn the_close_question_counts_what_is_running(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let dir = std::env::temp_dir().join(format!("fp-ui-exit-count-{}", std::process::id()));
    let (view, _runtime) = view_with_config(cx, &dir.join("config.json"));
    let cx = window(cx, &view);

    let id = seed_profile(cx, &view);
    cx.update(|window, cx| window.click(format!("start-{id}"), cx));
    wait_for_state(cx, &view, |state| {
        state
            .row(id)
            .is_some_and(|row| row.state() == RuntimeState::Running)
    });

    cx.update(|window, cx| {
        view.update(cx, |view, cx| view.on_close_requested(window, cx));
    });
    settle(cx);
    let asked = cx.update(|window, _| {
        ["background", "keep-running", "exit-all"]
            .into_iter()
            .map(|code| {
                window
                    .find(format!("exit-choice-{code}"))
                    .label()
                    .map(|label| label.to_string())
                    .unwrap_or_else(|| panic!("{code} is one of the answers"))
            })
            .collect::<Vec<_>>()
    });
    for answer in &asked {
        assert!(
            answer.contains('1') && answer.contains("running profile"),
            "each answer says what it does to the profile that is up: {answer}"
        );
    }
    assert!(
        asked[2].contains("stops the browsers"),
        "and the one that stops them says so: {}",
        asked[2]
    );
    assert!(
        !asked[0].contains("stops"),
        "while the one that leaves them alone does not: {}",
        asked[0]
    );
}
