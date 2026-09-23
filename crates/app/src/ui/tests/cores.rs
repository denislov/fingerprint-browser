use super::*;

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
    let selected = view
        .read_with(cx, |view, _| view.editor())
        .expect("a form")
        .read_with(cx, |editor, cx| editor.core(cx));
    assert_eq!(
        selected, seeded_core,
        "the form opens on the core the profile will run on"
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
    // Deleting lives in the row's overflow menu now, so the test opens it the
    // way a user does and picks the item by what it says: edit, the location,
    // a separator, then delete.
    core_menu(cx, 0, CoreMenu::Delete);
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
            crate::tray::Tray::start,
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

/// What a core can be asked to spoof is a reading, not a phrase.
///
/// The compatibility line is derived from the capability table. The regression
/// this guards is the one the page used to have: it matched the English word
/// "honoured" inside a display string, so the colour survived exactly as long as
/// nobody translated it.
#[gpui_kit::test]
fn a_core_row_reads_its_capability_rather_than_an_english_phrase(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    // The fixture's own core, of the generation that honours the switch.
    let modern_id = view.update(cx, |view, _| {
        let id = view
            .state()
            .core_rows()
            .expect("rows")
            .into_iter()
            .find(|row| row.core.version == "144.0.0.0")
            .expect("the fixture's core")
            .core
            .id;
        // A core whose version was never read. The service refuses to register
        // one - that is the refusal the editor shows - so the record is edited
        // into that shape, which is the only way the page can meet it.
        let mut blank = view.state().core(id).expect("the core");
        blank.version = String::new();
        blank.major = 0;
        view.state_mut()
            .update_core(blank)
            .expect("the row is saved");
        id
    });
    let silent_id = modern_id;
    let binary =
        |name: &str, banner: &str| crate::state::testing::CoreBinary::new(name, Some(banner));
    // Two more binaries, one either side of the pivot.
    let legacy = binary("ui-legacy-compat", "Chromium 128.0.0.0");
    let modern = binary("ui-modern-compat", "Chromium 148.0.7778.215");
    view.update(cx, |view, _| {
        view.state_mut()
            .add_core(None, legacy.path_buf())
            .expect("the older core is added");
        view.state_mut()
            .add_core(None, modern.path_buf())
            .expect("the newer core is added");
    });

    cx.update(|window, cx| window.click("nav-Cores", cx));
    settle(cx);

    let id_of = |cx: &mut gpui_kit::VisualTestContext, path: &std::path::Path| {
        view.read_with(cx, |view, _| {
            view.state()
                .core_rows()
                .expect("rows")
                .into_iter()
                .find(|row| row.core.executable == path)
                .expect("the core is listed")
                .core
                .id
        })
    };
    let reading = |cx: &mut gpui_kit::VisualTestContext, id: CoreId| {
        cx.update(|window, _| {
            window
                .find(format!("core-compatibility-{id}"))
                .label()
                .map(|label| label.to_string())
        })
        .expect("the row carries a compatibility column")
    };

    // Major 128 ignores it, and the row says so in words rather than in colours.
    let legacy_id = id_of(cx, &legacy.path_buf());
    let legacy_reading = reading(cx, legacy_id);
    assert!(
        legacy_reading.contains("Chrome 143 and older")
            && legacy_reading.contains("ignores exclusion switches"),
        "{legacy_reading}"
    );

    // A core that answered nothing has no switch set to describe, and that is a
    // state of its own rather than a quieter version of a working core.
    let silent_reading = reading(cx, silent_id);
    assert!(
        silent_reading.contains("no detected version"),
        "{silent_reading}"
    );
    assert_eq!(
        cx.update(|window, _| window
            .find(format!("core-version-{silent_id}"))
            .label()
            .map(|label| label.to_string()))
            .as_deref(),
        Some("version unknown"),
        "the version column says it has none rather than showing a blank"
    );

    // Major 148 is the generation that honours the exclusion switch.
    let modern_id = id_of(cx, &modern.path_buf());
    let modern_reading = reading(cx, modern_id);
    assert!(
        modern_reading.contains("Chrome 144+")
            && modern_reading.contains("excludes spoofing switches"),
        "{modern_reading}"
    );

    // The English the state model spells internally never reaches the window.
    for reading in [&modern_reading, &legacy_reading, &silent_reading] {
        assert!(
            !reading.contains("honoured") && !reading.contains("not honoured"),
            "the page still shows the state's own wording: {reading}"
        );
    }
}

/// A core's path is a value somebody has to paste somewhere, so the row copies
/// it and the menu opens the directory it came from.
#[gpui_kit::test]
fn a_cores_path_can_be_copied_and_its_location_opened(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let opener = Arc::new(FakeOpener::working());
    let (view, _runtime, opener) =
        view_with_opener(cx, Arc::new(FakeVerifier::passing()), None, opener);
    let cx = window(cx, &view);

    let binary =
        crate::state::testing::CoreBinary::new("ui-open-location", Some("Chromium 148.0.7778.215"));
    view.update(cx, |view, _| {
        view.state_mut()
            .add_core(None, binary.path_buf())
            .expect("the core is added");
    });
    cx.update(|window, cx| window.click("nav-Cores", cx));
    settle(cx);
    let (index, path) = view.read_with(cx, |view, _| {
        view.state()
            .core_rows()
            .expect("rows")
            .into_iter()
            .enumerate()
            .find(|(_, row)| row.core.executable == binary.path_buf())
            .map(|(index, row)| (index, row.core.executable))
            .expect("the added core is listed")
    });

    cx.update(|window, cx| window.click(format!("copy-core-path-{index}"), cx));
    settle(cx);
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("the path is on the clipboard");
    assert_eq!(copied, path.to_string_lossy());
    let toast = view
        .read_with(cx, |view, _| view.state().toasts().last().cloned())
        .expect("the copy is acknowledged");
    assert!(
        toast.message.contains(&path.display().to_string()),
        "the acknowledgement names what was copied: {}",
        toast.message
    );

    // Opening the location goes to the directory the binary lives in: there is
    // no portable "select this file" verb, and the folder is what a reader wants
    // when the path is wrong.
    core_menu(cx, index, CoreMenu::OpenLocation);
    wait_for_state(cx, &view, |state| {
        state
            .log_rows()
            .iter()
            .any(|row| row.message.contains("Opened"))
    });
    let directory = path.parent().expect("a parent directory").to_path_buf();
    assert_eq!(opener.opened(), vec![directory]);
}

/// The core row's cells line up under the headings that name them.
#[gpui_kit::test]
fn the_core_rows_line_up_under_the_column_headings(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Cores", cx));
    settle(cx);
    let id = view.read_with(cx, |view, _| {
        view.state().core_rows().expect("rows")[0].core.id
    });
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_aligned(
            window.find("column-version").bounds().left(),
            window.find(format!("core-version-{id}")).bounds().left(),
            "the version column",
        );
        assert_aligned(
            window.find("column-compatibility").bounds().left(),
            window
                .find(format!("core-compatibility-{id}"))
                .bounds()
                .left(),
            "the compatibility column",
        );
        assert_aligned(
            window.find("column-core-usage").bounds().left(),
            window.find(format!("core-usage-{id}")).bounds().left(),
            "the usage column",
        );
        assert_aligned(
            window.find("column-core-actions").bounds().right(),
            window.find("more-core-0").bounds().right(),
            "the actions column",
        );
    });
}
