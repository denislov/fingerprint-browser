use super::*;

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
