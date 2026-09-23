use super::*;

#[test]
fn editing_a_profile_writes_it_and_keeps_the_row_in_step() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Before").expect("create");

    let mut edited = fixture.state.profile(id).expect("the profile is loaded");
    edited.name = "After".to_string();
    edited.fingerprint.seed = 999;
    edited.window.width = 1600;
    fixture.state.update_profile(edited).expect("save");

    let row = fixture
        .state
        .rows()
        .iter()
        .find(|row| row.profile.id == id)
        .expect("the row is still listed");
    assert_eq!(row.profile.name, "After");
    assert_eq!(row.profile.fingerprint.seed, 999);
    assert_eq!(row.profile.window.width, 1600);
    assert_eq!(
        fixture.profiles.get(id).expect("stored").unwrap().name,
        "After",
        "the edit reached storage, not just the view"
    );
}

#[test]
fn a_refused_edit_leaves_the_stored_profile_alone() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Kept").expect("create");

    let mut broken = fixture.state.profile(id).expect("loaded");
    broken.name = "   ".to_string();

    assert!(fixture.state.update_profile(broken).is_err());
    assert!(
        fixture.state.notice().is_some_and(|notice| notice.error),
        "the refusal is shown in the banner"
    );
    assert_eq!(
        fixture.profiles.get(id).expect("stored").unwrap().name,
        "Kept"
    );
}

#[test]
fn a_duplicate_gets_its_own_identity_and_is_selected() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Source").expect("create");
    fixture.state.select(id);

    let copy = fixture.state.duplicate_profile(id).expect("duplicate");

    assert_ne!(copy, id);
    assert_eq!(fixture.state.selected_id(), Some(copy));
    let source = fixture.state.profile(id).expect("source");
    let duplicated = fixture.state.profile(copy).expect("copy");
    assert_ne!(duplicated.fingerprint.seed, source.fingerprint.seed);
    assert_ne!(duplicated.user_data_dir, source.user_data_dir);
    assert!(duplicated.name.contains("Source") || duplicated.name.contains("copy"));
}

#[test]
fn an_empty_filter_lists_every_profile() {
    let (mut fixture, _, _, _) = filtered_fixture();

    assert_eq!(fixture.state.visible_rows().len(), 3);

    // Whitespace is not a filter: it is a field someone tabbed through.
    fixture.state.set_profile_filter("   ");
    assert_eq!(fixture.state.visible_rows().len(), 3);
}

#[test]
fn a_filter_narrows_the_list_to_the_profiles_that_match() {
    let (mut fixture, work, _, _) = filtered_fixture();

    fixture.state.set_profile_filter("work");

    let visible = fixture.state.visible_rows();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].profile.id, work);
    assert_eq!(
        fixture.state.profile_filter(),
        "work",
        "the field holds what was typed, filter or not"
    );
}

#[test]
fn a_filter_ignores_case_and_the_space_around_it() {
    let (mut fixture, work, _, _) = filtered_fixture();

    fixture.state.set_profile_filter("  WORK  ");

    let visible = fixture.state.visible_rows();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].profile.id, work);
}

#[test]
fn a_filter_matches_the_seed_as_well_as_the_name() {
    let (mut fixture, _, _, staging) = filtered_fixture();

    fixture.state.set_profile_filter("1234");

    let visible = fixture.state.visible_rows();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].profile.id, staging);
}

#[test]
fn a_filter_that_matches_nothing_hides_the_list_without_touching_a_profile() {
    let (mut fixture, _, _, _) = filtered_fixture();

    fixture.state.set_profile_filter("nothing is called this");

    assert!(fixture.state.visible_rows().is_empty());
    assert_eq!(
        fixture.state.rows().len(),
        3,
        "a filter is a view of the list, not a way to lose profiles"
    );
}

#[test]
fn a_filtered_out_profile_keeps_its_focus_and_keeps_running() {
    let (mut fixture, work, _, _) = filtered_fixture();
    fixture.state.select(work);
    fixture.state.start(work).expect("start");
    let started = fixture
        .state
        .rows()
        .iter()
        .find(|row| row.profile.id == work)
        .expect("the row is listed")
        .state();
    assert!(
        matches!(started, RuntimeState::Running),
        "the fixture really starts it"
    );

    fixture.state.set_profile_filter("shopping");

    assert_eq!(
        fixture.state.selected_id(),
        Some(work),
        "the panel answers for what was chosen, not for what is listed"
    );
    let visible = fixture.state.visible_rows();
    assert_eq!(visible.len(), 1);
    assert_ne!(visible[0].profile.id, work);
    let after = fixture
        .state
        .rows()
        .iter()
        .find(|row| row.profile.id == work)
        .expect("still listed")
        .state();
    assert!(
        matches!(after, RuntimeState::Running),
        "hiding a row cannot stop the browser behind it"
    );
}

/// A wrong address is a finding, not a footnote: the profile claims to
/// leave by a proxy that was measured somewhere else.
#[test]
fn a_profile_that_leaves_from_elsewhere_is_not_confirmed() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.finish_verification(
        id,
        Ok(VerificationReport {
            discrepancies: vec![Discrepancy {
                claim: "exit address",
                expected: "203.0.113.7 (where the proxy was tested)".to_string(),
                observed: "198.51.100.9".to_string(),
            }],
            exit_ip: Some("198.51.100.9".to_string()),
            exit_unreadable: None,
        }),
    );

    let verification = fixture.state.verification(id).expect("recorded");

    assert_eq!(verification.label(en()), "1 claim not confirmed");
    assert_eq!(
        verification
            .report()
            .and_then(|report| report.exit_label(en())),
        Some("traffic left from 198.51.100.9".to_string()),
        "the address is still worth showing: it is where the traffic went"
    );
}

/// When a profile is already up, its engine is the one that would have to
/// forward, so the test asks that engine instead of starting a second one.
#[test]
fn a_running_profile_makes_the_test_probe_its_engine() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let profile_id = fixture.state.create_profile("Proxied").expect("create");
    let mut profile = fixture.state.profile(profile_id).expect("loaded");
    profile.proxy_id = Some(proxy_id);
    fixture.state.update_profile(profile).expect("assign");
    fixture.runtime.set_state(profile_id, RuntimeState::Running);
    fixture.runtime.set_socks_port(profile_id, 51234);
    fixture.state.refresh_runtime();

    let job = fixture.state.begin_proxy_test(proxy_id).expect("begin");

    assert_eq!(
        job.live_port,
        Some(51234),
        "the engine a profile is using is the one that has to carry the request"
    );
    assert!(job.is_live());
}

/// The port is cleared when a profile stops, so a test cannot probe a port
/// that nothing is listening on and call the answer a proxy failure.
#[test]
fn a_stopped_profile_leaves_no_port_to_probe() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let proxy_id = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let profile_id = fixture.state.create_profile("Proxied").expect("create");
    let mut profile = fixture.state.profile(profile_id).expect("loaded");
    profile.proxy_id = Some(proxy_id);
    fixture.state.update_profile(profile).expect("assign");
    fixture.runtime.set_state(profile_id, RuntimeState::Running);
    fixture.runtime.set_socks_port(profile_id, 51234);
    fixture.state.refresh_runtime();

    fixture.runtime.set_state(profile_id, RuntimeState::Stopped);
    fixture.state.refresh_runtime();

    // The snapshot still holds the port, which is exactly the trap: it is
    // the profile's own state that decides, so a stale port is never
    // dialled and a dead socket is never reported as a broken proxy.
    assert_eq!(
        fixture
            .runtime
            .snapshot_of(profile_id)
            .expect("snapshot")
            .socks_port,
        Some(51234),
        "the port outlives the profile in the snapshot"
    );

    let job = fixture.state.begin_proxy_test(proxy_id).expect("begin");
    assert_eq!(job.live_port, None);
}

#[test]
fn an_outcome_records_the_address_and_the_path_it_left_by() {
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
        true,
        Ok(Diagnosis {
            exit_ip: "198.51.100.9".to_string(),
            elapsed: Duration::from_millis(431),
        }),
    );

    let test = fixture.state.proxy_test(id).expect("a result");
    let reading = test.reading().expect("a reading, not a fault");
    assert_eq!(reading.exit_ip, "198.51.100.9");
    assert_eq!(reading.elapsed, Duration::from_millis(431));
    assert!(reading.live, "the request went through the running engine");
    assert_eq!(test.label(en()), "exit 198.51.100.9");
    assert!(test.fault().is_none());
}

/// A fault is not a reading with an empty address: it says where the
/// request stopped, because that is what decides the fix.
#[test]
fn a_failed_test_keeps_the_class_and_the_evidence() {
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
        Err(Fault::new(
            FaultClass::Auth,
            "upstream refused the credentials it was offered",
        )),
    );

    let test = fixture.state.proxy_test(id).expect("a result");
    assert!(
        test.reading().is_none(),
        "nothing left, so nothing was read"
    );
    let fault = test.fault().expect("the fault");
    assert_eq!(fault.class, FaultClass::Auth);
    assert!(fault.detail.contains("credentials"), "{fault}");
    assert_eq!(test.label(en()), "no traffic (authentication)");
    assert!(
        test.detail(en())
            .expect("a detail line")
            .contains("no traffic reached the endpoint"),
        "the row and the log both have to say nothing arrived"
    );
}

#[test]
fn create_lists_the_profile_and_selects_it() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("initial load");

    let id = fixture.state.create_profile("Primary").expect("create");

    assert_eq!(fixture.state.rows().len(), 1);
    let row = &fixture.state.rows()[0];
    assert_eq!(row.profile.name, "Primary");
    assert_eq!(row.profile.id, id);
    assert_eq!(row.state(), RuntimeState::Stopped);
    assert_eq!(row.state_label(en()), "Stopped");
    assert_eq!(row.core_name, "Test Core 144");
    assert_eq!(fixture.state.selected_id(), Some(id));
    assert!(row.can_start());
    assert!(!row.can_stop());
}

#[test]
fn start_and_stop_update_the_row_from_the_snapshot() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Primary").expect("create");

    fixture.state.start(id).expect("start");
    let row = fixture.state.selected().expect("selected row");
    assert_eq!(row.state(), RuntimeState::Running);
    assert!(row.can_stop());
    assert!(!row.can_start());

    fixture.state.stop(id).expect("stop");
    assert_eq!(
        fixture.state.selected().expect("selected row").state(),
        RuntimeState::Stopped
    );

    let commands = fixture.runtime.commands.lock().expect("commands");
    assert_eq!(
        commands.as_slice(),
        [format!("start:{id}"), format!("stop:{id}")]
    );
}

#[test]
fn refresh_runtime_picks_up_external_state_changes() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Primary").expect("create");

    // Simulate a supervisor-side crash that the UI never received as an event.
    fixture.runtime.set_state(
        id,
        RuntimeState::Crashed {
            message: "browser exited".to_string(),
        },
    );
    fixture.state.refresh_runtime();

    let row = fixture.state.selected().expect("selected row");
    assert_eq!(row.state_label(en()), "Crashed");
    assert_eq!(row.state_message(), Some("browser exited"));
    assert!(row.can_start());
}

#[test]
fn load_drops_a_selection_whose_profile_was_deleted() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Primary").expect("create");

    fixture.profiles.delete(id).expect("delete");
    fixture.state.load().expect("reload");

    assert!(fixture.state.rows().is_empty());
    assert_eq!(fixture.state.selected_id(), None);
}

#[test]
fn rows_are_sorted_by_name_and_renumbered_for_new_profiles() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");

    assert_eq!(fixture.state.next_profile_name(), "Profile 1");
    fixture.state.create_profile("Bravo").expect("create");
    assert_eq!(fixture.state.next_profile_name(), "Profile 2");
    fixture.state.create_profile("Alpha").expect("create");

    let names: Vec<&str> = fixture
        .state
        .rows()
        .iter()
        .map(|row| row.profile.name.as_str())
        .collect();
    assert_eq!(names, ["Alpha", "Bravo"]);
}

#[test]
fn the_details_panel_starts_on_details() {
    let mut fixture = fixture();
    assert_eq!(fixture.state.details_tab(), DetailsTab::Details);
    fixture.state.set_details_tab(DetailsTab::Args);
    assert_eq!(fixture.state.details_tab(), DetailsTab::Args);
}

/// One configuration task at a time, and the marker is what makes it so.
///
/// The four of them read and write the same rows and the same file, and two
/// clicks arriving before the first answer would otherwise interleave an
/// import with the restore that is replacing everything it is importing into.
#[test]
fn a_second_configuration_task_is_refused_while_one_is_running() {
    let scratch = Scratch::new("one-at-a-time");
    let mut fixture = fixture();
    fixture.state.set_export_path(
        scratch
            .dir
            .join("config.json")
            .to_string_lossy()
            .to_string(),
    );

    // A path, so the refusal below is about the task in flight rather than
    // about an empty field.
    fixture.state.set_import_path(
        scratch
            .dir
            .join("elsewhere.json")
            .to_string_lossy()
            .to_string(),
    );
    let job = fixture.state.begin_export().expect("the first task");
    // No answer has come back yet, which is exactly when a second click
    // happens.
    let refused = fixture
        .state
        .begin_import()
        .err()
        .expect("one configuration task at a time");
    assert!(refused.contains("still running"), "{refused}");
    let notice = fixture.state.notice().expect("a notice");
    assert!(notice.error, "{notice:?}");
    assert_eq!(notice.message, refused);

    // The answer lets the next one in - a failure as much as a success, or the
    // window would refuse every task after the first that went wrong.
    let destination = job.destination.clone();
    let result = crate::maintenance::run_export(job);
    fixture
        .state
        .finish_maintenance(crate::maintenance::Outcome::Exported {
            destination,
            result,
        });
    fixture.state.begin_export().expect("the marker was let go");
}

#[test]
fn asking_for_the_credentials_writes_them_and_says_so() {
    let scratch = Scratch::new("asked-for");
    let mut fixture = fixture();
    seed_core(&fixture);
    seed_proxy_holding_a_password(&mut fixture);
    fixture.state.load().expect("load");

    let path = scratch.join("config.json");
    fixture
        .state
        .set_export_path(path.to_string_lossy().to_string());
    fixture.state.set_export_includes_credentials(true);
    let report = fixture.state.export_configuration().expect("export");

    assert_eq!(report.credentials, Credentials::Included);
    assert_eq!(report.credentials_removed, 0);
    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(text.contains(EXPORT_SECRET));

    // The warning is part of the answer, not decoration: a file holding
    // passwords in plain text has to say so where the user is looking.
    let message = last_message(&fixture);
    assert!(message.contains("plain text"), "{message}");
    assert!(message.contains("config.json"), "{message}");
}

#[test]
fn is_configuration_empty_says_what_is_here() {
    let mut fixture = fixture();
    assert!(fixture.state.is_configuration_empty().expect("read"));

    seed_core(&fixture);
    fixture.state.load().expect("load");
    fixture.state.create_profile("Work laptop").expect("create");

    assert!(!fixture.state.is_configuration_empty().expect("read"));
}

/// And the fourth: a worker that never reports at all. The wait is bounded
/// rather than kept until the window closes, because a lease that outlives
/// the thing that took it is a profile that can never be started again.
#[test]
fn a_check_that_never_answers_gives_its_profile_back() {
    let mut fixture = fixture();
    let (id, proxy_id) = proxied(&mut fixture, "Work laptop");
    fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("the gate");
    // The check is as old as the bound allows, which is what it looks like
    // from here when the worker died.
    fixture.state.pending_starts.insert(
        id,
        PendingStart {
            proxy: proxy_id,
            how: Opening::Start,
            asked: Instant::now()
                .checked_sub(CHECK_LEASE + Duration::from_secs(1))
                .expect("the clock goes back"),
        },
    );

    fixture.state.refresh_runtime();

    assert_eq!(
        fixture.state.row(id).expect("the row").state(),
        RuntimeState::Stopped
    );
    assert_eq!(fixture.state.operations.held(id), None);
    assert_eq!(
        fixture.state.proxy_test(proxy_id),
        Some(&ProxyTest::Running),
        "the reading is still the row's business, not the start's"
    );
    let message = last_message(&fixture);
    assert!(message.contains("did not answer in time"), "{message}");

    // And the profile can be started again, which is the whole point. The
    // reading is still in flight, so the second attempt waits for it rather
    // than asking a second time - what matters is that it is not refused.
    let again = fixture
        .state
        .begin_opening(id, Opening::Start)
        .expect("a second attempt");
    assert!(matches!(again, StartGate::Awaiting));
}

/// A start holds its profile from the command until the runtime has answered
/// for it. A stopped snapshot is the window between the two, and a copy is
/// refused for its whole width - which the snapshot alone could not do, since
/// it is stopped for the whole of it.
#[test]
fn a_queued_start_holds_its_profile_until_the_snapshot_answers() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Work laptop").expect("create");
    fixture.state.set_browser_data_path("/backups/fp");

    // The command is queued and nothing has answered: the profile reads as
    // stopped, which is exactly the window the lease is for.
    fixture.runtime.answer_nothing();
    fixture.state.start(id).expect("the command is queued");
    assert_eq!(
        fixture.state.operations.held(id),
        Some(Operation::Starting),
        "a start that has not been answered holds its profile"
    );

    let error = fixture
        .state
        .browser_data_job(Direction::ToBackup)
        .expect_err("the queued start holds the profile");
    assert!(error.contains("Work laptop"), "{error}");

    // The runtime answers. The snapshot refuses a copy from here on, and the
    // lease is gone - which is what lets the profile be restarted at all.
    fixture.runtime.set_state(id, RuntimeState::Running);
    fixture.state.refresh_runtime();
    assert_eq!(
        fixture.state.operations.held(id),
        None,
        "an answered start gives its profile back"
    );
    fixture
        .state
        .restart(id)
        .expect("a restart is not refused by the start before it");
}

#[test]
fn retrying_a_failed_start_keeps_its_lease_until_its_own_answer() {
    for old in [
        RuntimeState::Failed {
            message: "old failure".into(),
        },
        RuntimeState::Crashed {
            message: "old crash".into(),
        },
    ] {
        let mut fixture = fixture();
        seed_core(&fixture);
        fixture.state.load().unwrap();
        let id = fixture.state.create_profile("Retry").unwrap();
        fixture.runtime.set_state(id, old);
        fixture.runtime.answer_nothing();
        fixture.state.start(id).unwrap();
        fixture.state.refresh_runtime();
        assert!(fixture.state.row(id).unwrap().can_stop());
        assert!(fixture.state.delete_profile(id).is_err());
        fixture.state.set_browser_data_path("/backups/fp");
        assert_eq!(fixture.state.operations.held(id), Some(Operation::Starting));
        assert!(fixture.state.browser_data_job(Direction::ToBackup).is_err());
        // Even a failed retry ends its lease, once it is this request's answer.
        fixture.runtime.set_state(
            id,
            RuntimeState::Failed {
                message: "new failure".into(),
            },
        );
        fixture.state.refresh_runtime();
        assert_eq!(fixture.state.operations.held(id), None);
        assert!(fixture.state.browser_data_job(Direction::ToBackup).is_ok());
    }
}

#[test]
fn a_queued_start_can_be_stopped_before_the_runtime_starts_it() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().unwrap();
    let id = fixture.state.create_profile("Queued").unwrap();
    fixture.runtime.answer_nothing();
    fixture.state.start(id).unwrap();
    assert!(fixture.state.row(id).unwrap().can_stop());
    fixture.state.stop(id).unwrap();
    assert_eq!(fixture.state.operations.held(id), None);
    assert_eq!(
        fixture.state.row(id).unwrap().state(),
        RuntimeState::Stopped
    );
    assert!(fixture.state.delete_profile(id).is_ok());
}
