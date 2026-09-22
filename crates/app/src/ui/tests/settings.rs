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
