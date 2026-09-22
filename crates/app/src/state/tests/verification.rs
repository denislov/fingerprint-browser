use super::*;

#[test]
fn deleting_a_profile_removes_it_and_forgets_its_reading() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture
        .state
        .finish_verification(id, Ok(VerificationReport::default()));

    fixture.state.delete_profile(id).expect("delete");

    assert!(fixture.state.rows().is_empty(), "the row is gone");
    assert!(fixture.state.profile(id).is_none());
    assert!(
        fixture.state.verification(id).is_none(),
        "a deleted profile keeps no verification result"
    );
    assert_eq!(fixture.state.selected_id(), None);
}

/// A core whose version was never read has no capability table. Asking for
/// one used to reach `CoreCapabilities::for_major(0)`, which is a debug
/// assertion: in a debug build the window would have panicked.
#[test]
fn verifying_through_a_core_without_a_version_is_refused_not_asserted() {
    let mut fixture = fixture();
    let unknown = domain::BrowserCore {
        id: CoreId::new(),
        name: "unknown 0".to_string(),
        executable: PathBuf::from("/tmp/whatever/chrome"),
        version: "unknown".to_string(),
        major: 0,
    };
    fixture.cores.save(&unknown).expect("save core");
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("unreadable").expect("create");
    fixture.runtime.set_state(id, RuntimeState::Running);
    fixture.runtime.set_cdp_port(id, 9333);
    fixture.state.refresh_runtime();

    let error = fixture.state.begin_verification(id).unwrap_err();
    assert!(error.to_string().contains("no detected version"), "{error}");
    assert_eq!(
        fixture.state.core_rows().expect("rows")[0].generation_label(),
        None,
        "there is no generation to show"
    );
}

/// The half of the answer the row cannot show: the row says the fingerprint
/// was confirmed, and this is where the traffic went.
#[test]
fn a_confirmed_reading_is_logged_with_the_address_it_left_from() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.finish_verification(
        id,
        Ok(VerificationReport {
            exit_ip: Some("203.0.113.7".to_string()),
            ..VerificationReport::default()
        }),
    );

    let line = confirmed_line_in(&fixture);

    assert!(line.contains("203.0.113.7"), "{line}");
}

/// "Confirmed" on its own would read as though the address had been checked
/// too, so a reading that was not taken says so in the same line.
#[test]
fn a_confirmed_reading_that_could_not_read_an_address_says_so() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.finish_verification(
        id,
        Ok(VerificationReport {
            exit_unreadable: Some("the browser's page never committed".to_string()),
            ..VerificationReport::default()
        }),
    );

    let line = confirmed_line_in(&fixture);

    assert!(line.contains("exit address was not read"), "{line}");
    assert!(line.contains("never committed"), "{line}");
}

#[test]
fn a_verification_needs_a_running_browser_with_a_debug_port() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture
        .state
        .create_profile("stopped")
        .expect("create profile");

    assert!(fixture.state.begin_verification(id).is_err());
    assert!(fixture.state.verification(id).is_none());
}

#[test]
fn a_verification_job_carries_the_profile_and_its_capabilities() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);

    let job = fixture.state.begin_verification(id).expect("begin");

    assert_eq!(job.port, 9333);
    assert_eq!(job.profile_id, id);
    assert_eq!(
        job.profile.seed,
        fixture.state.rows()[0].profile.fingerprint.seed
    );
    assert_eq!(
        job.capabilities.major, 144,
        "the capabilities come from the profile's own core"
    );
    assert!(
        fixture
            .state
            .verification(id)
            .is_some_and(Verification::is_running)
    );
}

#[test]
fn an_old_verification_does_not_replace_the_current_task() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    let old = fixture.state.begin_verification(id).unwrap();
    fixture.state.forget_verification(id);
    let current = fixture.state.begin_verification(id).unwrap();
    fixture
        .state
        .complete_verification(&old, Ok(VerificationReport::default()));
    assert!(fixture.state.verification(id).unwrap().is_running());
    fixture
        .state
        .complete_verification(&current, Err("current failure".into()));
    assert!(
        matches!(fixture.state.verification(id), Some(Verification::Unreadable(message)) if message == "current failure")
    );
}

#[test]
fn verification_from_a_browser_that_exited_is_discarded() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    let job = fixture.state.begin_verification(id).unwrap();
    fixture.runtime.set_state(id, RuntimeState::Stopped);
    fixture
        .state
        .complete_verification(&job, Ok(VerificationReport::default()));
    assert!(fixture.state.verification(id).is_none());
}

#[test]
fn a_second_verification_of_the_same_profile_is_refused() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.begin_verification(id).expect("first");

    assert!(fixture.state.begin_verification(id).is_err());
}

#[test]
fn an_outcome_records_confirmation_disagreement_or_failure() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);

    fixture
        .state
        .finish_verification(id, Ok(VerificationReport::default()));
    assert!(matches!(
        fixture.state.verification(id),
        Some(Verification::Confirmed(_))
    ));

    let disagreement = Discrepancy {
        claim: "platform",
        expected: "Win32".to_string(),
        observed: "Linux x86_64".to_string(),
    };
    fixture.state.finish_verification(
        id,
        Ok(VerificationReport::from_discrepancies(vec![
            disagreement.clone(),
        ])),
    );
    let recorded = fixture.state.verification(id).expect("recorded");
    assert_eq!(
        recorded.disagreements(),
        std::slice::from_ref(&disagreement)
    );
    assert_eq!(recorded.label(en()), "1 claim not confirmed");

    fixture
        .state
        .finish_verification(id, Err("no debug port".to_string()));
    let failed = fixture.state.verification(id).expect("recorded");
    assert_eq!(failed.failure(), Some("no debug port"));
    assert!(
        !failed.is_running(),
        "a failed reading is not a reading in flight"
    );
}

#[test]
fn stopping_or_restarting_a_profile_drops_a_stale_reading() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture
        .state
        .finish_verification(id, Ok(VerificationReport::default()));
    assert!(fixture.state.verification(id).is_some());

    fixture.state.restart(id).expect("restart");
    assert!(
        fixture.state.verification(id).is_none(),
        "a restarted browser has a new fingerprint"
    );

    fixture
        .state
        .finish_verification(id, Ok(VerificationReport::default()));
    fixture.state.stop(id).expect("stop");
    assert!(fixture.state.verification(id).is_none());
}

#[test]
fn a_failed_reading_is_logged_as_an_error() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);

    fixture
        .state
        .finish_verification(id, Err("no debug port".to_string()));

    let entry = fixture.state.log_entries().last().expect("a line");
    assert_eq!(entry.level, LogLevel::Error);
    assert!(entry.message.contains("no debug port"), "{}", entry.message);
}
