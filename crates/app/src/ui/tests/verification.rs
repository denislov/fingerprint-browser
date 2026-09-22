use super::*;

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
