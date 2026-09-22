use super::*;

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
