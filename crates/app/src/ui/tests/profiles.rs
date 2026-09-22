use super::*;

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
