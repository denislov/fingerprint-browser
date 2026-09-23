use super::*;

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
    profile_menu(cx, id, ProfileMenu::Edit);

    pick_proxy(cx, &view, Some(proxy_id));
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
    let proxy_id = view.read_with(cx, |view, _| {
        view.state().proxy_rows().expect("rows")[0].proxy.id
    });
    profile_menu(cx, id, ProfileMenu::Edit);
    pick_proxy(cx, &view, Some(proxy_id));
    settle(cx);
    cx.update(|window, cx| window.click("ok", cx));
    settle(cx);

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    // Deleting lives in the row's overflow menu now: edit, a separator, delete.
    cx.update(|window, cx| window.click("more-proxy-0", cx));
    settle(cx);
    cx.update(|window, cx| window.within("popup-menu").click(2usize, cx));
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

/// The four states a proxy's last test can be in, as the row reads them.
///
/// Three of them are results and one is the absence of one, and a blank cell
/// would make "nobody asked" look like "the answer was nothing". Driven through
/// the state rather than by pressing Test, because the point here is what the
/// row draws in each state; the press itself is covered above.
#[gpui_kit::test]
fn a_proxy_row_reads_differently_in_each_test_state(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    let proxy_id = seed_proxy(cx, &view, "Office");

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    let cell = |cx: &mut gpui_kit::VisualTestContext| {
        cx.update(|window, _| {
            window
                .find(format!("proxy-test-{proxy_id}"))
                .label()
                .map(|label| label.to_string())
        })
        .expect("the row carries a test column")
    };

    // Never asked: a state of its own, and not an empty cell.
    assert!(
        cell(cx).contains("not tested"),
        "an untested proxy says so: {}",
        cell(cx)
    );

    // In flight: the reading is named as pending, and the button that would ask
    // again is disabled while it is.
    view.update(cx, |view, cx| {
        view.state_mut()
            .begin_proxy_test(proxy_id)
            .expect("no test is in flight");
        cx.notify();
    });
    settle(cx);
    assert!(
        cell(cx).contains("testing"),
        "a test in flight says so: {}",
        cell(cx)
    );
    // The button that would ask again is put into its loading state for the same
    // block; what stops a second request is the state's own lease, which
    // `a_proxy_already_being_tested_is_not_tested_twice` asserts.

    // It answered: the address the traffic left from is the whole point, and the
    // time the engine took is named as the test's own, not as a latency.
    view.update(cx, |view, cx| {
        view.state_mut().finish_proxy_test(
            proxy_id,
            false,
            Ok(runtime::Diagnosis {
                exit_ip: "203.0.113.7".to_string(),
                elapsed: std::time::Duration::from_millis(250),
            }),
        );
        cx.notify();
    });
    settle(cx);
    let passed = cell(cx);
    assert!(passed.contains("203.0.113.7"), "the exit address: {passed}");
    assert!(passed.contains("250 ms"), "how long it took: {passed}");
    assert!(
        passed.contains("through a temporary engine"),
        "and which engine was probed, because a rehearsal and a live path are \
         different claims: {passed}"
    );

    // It did not answer. The class is on the row and the engine's own words are
    // one press away, because a row is one line and a fault is a paragraph.
    view.update(cx, |view, cx| {
        view.state_mut().finish_proxy_test(
            proxy_id,
            false,
            Err(Fault::new(FaultClass::Timeout, "no answer in 30s")),
        );
        cx.notify();
    });
    settle(cx);
    let failed = cell(cx);
    assert!(failed.contains("no traffic (timeout)"), "{failed}");
    // The class is what the row draws; the whole reading - the engine's own
    // words included - is the cell's accessible name, so a reader who cannot see
    // the row is not left with a class and no reason.
    assert!(
        failed.contains("no answer in 30s"),
        "the accessible name carries the fault in full: {failed}"
    );
    // And none of it is wrapped into the row: a fault is a paragraph, and a
    // paragraph inside a list row pushes every row below it down.
    let height = cx.update(|window, _| window.find("proxy-0").bounds().size.height.as_f32());
    assert!(
        height <= 64.5,
        "the fault stayed out of the row: {height}px"
    );
    assert!(
        cx.update(|window, _| window
            .try_find(format!("proxy-diagnostics-{proxy_id}"))
            .is_some()),
        "a failed row offers the way to the evidence"
    );
}

/// The evidence behind a failed reading is the line the engine wrote, and the
/// row's own control is what takes the reader to it.
#[gpui_kit::test]
fn a_failed_test_takes_its_reader_to_the_line_the_engine_wrote(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime, _opener, _tester, _copier) = view_with_tester(
        cx,
        Arc::new(FakeVerifier::passing()),
        Arc::new(FakeProxyTester::with_outcome(Err(Fault::new(
            FaultClass::Timeout,
            "no answer in 30s",
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

    cx.update(|window, cx| window.click(format!("proxy-diagnostics-{proxy_id}"), cx));
    settle(cx);

    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        crate::state::Page::Log,
        "the control opens the page the fault was written to"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().log_filter()),
        crate::state::LogFilter::Errors,
        "and narrows it to the lines that are problems"
    );
    let line = cx
        .update(|window, _| window.find("log-0").label().map(|label| label.to_string()))
        .expect("the failure is the newest line");
    assert!(line.contains("no answer in 30s"), "{line}");
}

/// The proxy row's cells line up under the headings that name them.
#[gpui_kit::test]
fn the_proxy_rows_line_up_under_the_column_headings(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let cx = window(cx, &view);
    cx.simulate_resize(size(px(1440.), px(900.)));
    let proxy_id = seed_proxy(cx, &view, "Office");

    cx.update(|window, cx| window.click("nav-Proxies", cx));
    settle(cx);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_aligned(
            window.find("column-endpoint").bounds().left(),
            window
                .find(format!("proxy-endpoint-{proxy_id}"))
                .bounds()
                .left(),
            "the endpoint column",
        );
        assert_aligned(
            window.find("column-usage").bounds().left(),
            window
                .find(format!("proxy-usage-{proxy_id}"))
                .bounds()
                .left(),
            "the usage column",
        );
        assert_aligned(
            window.find("column-test").bounds().left(),
            window
                .find(format!("proxy-test-{proxy_id}"))
                .bounds()
                .left(),
            "the last-test column",
        );
        assert_aligned(
            window.find("column-actions").bounds().right(),
            window.find("more-proxy-0").bounds().right(),
            "the actions column",
        );
    });
}
