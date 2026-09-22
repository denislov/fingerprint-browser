use super::*;

/// The address and the path are the record: a row is replaced by the next
/// test, and the log is not.
#[test]
fn the_activity_log_keeps_the_address_the_path_and_the_class() {
    let dir = std::env::temp_dir().join(format!("fp-proxy-test-log-{}", CoreId::new()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut fixture = fixture_with_log(&dir);
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let ok = fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");
    let bad = fixture
        .state
        .create_proxy("Home", socks5("10.0.0.2", 1080))
        .expect("create proxy");

    fixture.state.begin_proxy_test(ok).expect("begin");
    fixture.state.finish_proxy_test(
        ok,
        false,
        Ok(Diagnosis {
            exit_ip: "198.51.100.9".to_string(),
            elapsed: Duration::from_millis(90),
        }),
    );
    fixture.state.begin_proxy_test(bad).expect("begin");
    fixture.state.finish_proxy_test(
        bad,
        false,
        Err(Fault::new(
            FaultClass::Unreachable,
            "no route to the upstream",
        )),
    );

    let lines: Vec<String> = fixture
        .state
        .log_entries()
        .iter()
        .map(|entry| entry.message.clone())
        .collect();
    // `Created proxy Office` is also a line about Office, so the search is
    // for the test's own record rather than for the name.
    let mention = |name: &str| {
        lines
            .iter()
            .find(|line| line.starts_with("proxy test:") && line.contains(name))
            .unwrap_or_else(|| panic!("no record of testing {name}: {lines:?}"))
    };
    let passed = mention("Office");
    assert!(passed.contains("198.51.100.9"), "{passed}");
    assert!(
        passed.contains("temporary engine"),
        "the path is the half of the answer the row cannot keep: {passed}"
    );
    let failed = mention("Home");
    assert!(failed.contains("unreachable"), "{failed}");
    assert!(failed.contains("no route"), "{failed}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The class is the whole point of keeping them apart, so it is what the
/// immediate feedback leads with.
#[test]
fn a_failure_toasts_the_class_and_a_success_the_address() {
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
            FaultClass::Timeout,
            "the request ran out of time",
        )),
    );
    let toast = fixture.state.toasts().last().expect("a toast").clone();
    assert_eq!(toast.kind, ToastKind::Error);
    assert!(toast.message.contains("timeout"), "{}", toast.message);

    fixture.state.begin_proxy_test(id).expect("begin again");
    fixture.state.finish_proxy_test(
        id,
        false,
        Ok(Diagnosis {
            exit_ip: "198.51.100.9".to_string(),
            elapsed: Duration::from_millis(90),
        }),
    );
    let toast = fixture.state.toasts().last().expect("a toast").clone();
    assert_eq!(toast.kind, ToastKind::Success);
    assert!(toast.message.contains("198.51.100.9"), "{}", toast.message);
}

#[test]
fn starting_an_unknown_profile_records_an_error_notice() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");

    let unknown = ProfileId::new();
    let error = fixture.state.start(unknown).expect_err("must fail");

    assert!(matches!(error, AppError::NotFound(_)));
    assert!(fixture.state.notice().expect("notice").error);
}

#[test]
fn a_success_is_a_toast_and_leaves_the_banner_clear() {
    let mut fixture = fixture();

    fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create proxy");

    let toast = fixture.state.toasts().last().expect("a toast");
    assert_eq!(toast.kind, ToastKind::Success);
    assert!(toast.message.contains("Office"), "{}", toast.message);
    assert!(
        fixture.state.notice().is_none(),
        "a success does not need an acknowledgement"
    );
}

#[test]
fn a_problem_owns_the_banner_until_it_is_dismissed() {
    let mut fixture = fixture();

    fixture
        .state
        .create_proxy("Broken", socks5("", 1080))
        .expect_err("refused");

    let notice = fixture.state.notice().expect("the banner");
    assert!(notice.error);
    assert_eq!(
        fixture.state.toasts().last().map(|toast| toast.kind),
        Some(ToastKind::Error),
        "the problem is a toast as well, so it is seen while it happens"
    );

    // A later success clears the problem: the banner is the current
    // problem, not every problem ever seen.
    fixture.state.dismiss_notice();
    assert!(fixture.state.notice().is_none());
    fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create");
    assert!(fixture.state.notice().is_none());
}

#[test]
fn draining_toasts_leaves_nothing_to_show_twice() {
    let mut fixture = fixture();
    fixture
        .state
        .create_proxy("Office", socks5("10.0.0.1", 1080))
        .expect("create");
    assert!(!fixture.state.toasts().is_empty());

    let drained = fixture.state.drain_toasts();

    assert_eq!(drained.len(), 1);
    assert!(drained[0].message.contains("Office"));
    assert!(fixture.state.toasts().is_empty());
    assert!(fixture.state.drain_toasts().is_empty());
}

#[test]
fn every_notice_is_written_to_the_log() {
    let mut fixture = fixture();
    assert!(fixture.state.log_entries().is_empty());

    fixture.state.push_notice("Saved.", false);
    fixture.state.push_notice("it broke", true);

    let entries = fixture.state.log_entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].level, LogLevel::Info);
    assert_eq!(entries[0].message, "Saved.");
    assert_eq!(entries[0].profile_id, None, "a window-level line");
    assert_eq!(entries[1].level, LogLevel::Error);
    assert_eq!(entries[1].message, "it broke");
}

#[test]
fn runtime_events_are_written_to_the_log_once_each() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    let profile_id = id;
    // The profile creation is its own line; this test is about events.
    fixture.state.clear_log();

    for event in [
        RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Starting,
        },
        RuntimeEvent::EffectiveLaunchArgs {
            profile_id,
            args: vec!["a".into(), "b".into(), "c".into()],
        },
        RuntimeEvent::Started {
            profile_id,
            browser_pid: 4242,
            xray_pid: Some(4343),
            cdp_port: 9222,
            socks_port: Some(1080),
        },
        RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Running,
        },
        RuntimeEvent::Warning {
            profile_id,
            message: "legacy core".into(),
        },
        RuntimeEvent::Stopped { profile_id },
        RuntimeEvent::StateChanged {
            profile_id,
            state: RuntimeState::Stopped,
        },
    ] {
        fixture.state.record_event(&event);
    }

    let messages: Vec<(&str, String)> = fixture
        .state
        .log_entries()
        .iter()
        .map(|entry| (entry.level.label(en()), entry.message.clone()))
        .collect();
    assert_eq!(
        messages,
        [
            ("info", "starting".to_string()),
            ("info", "launching with 3 arguments".to_string()),
            (
                "info",
                "browser started (pid 4242, cdp port 9222, socks port 1080, xray pid 4343)"
                    .to_string()
            ),
            ("warning", "legacy core".to_string()),
            ("info", "browser stopped".to_string()),
        ],
        "running and stopped state changes are the events' own lines, not extra ones"
    );
    assert!(
        fixture
            .state
            .log_entries()
            .iter()
            .all(|entry| entry.profile_id == Some(profile_id))
    );
}

#[test]
fn a_crash_and_a_refused_start_are_logged_as_errors() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Primary").expect("create");
    // Creating the profile is its own (informational) line.
    fixture.state.clear_log();

    fixture.state.record_event(&RuntimeEvent::Crashed {
        profile_id: id,
        component: RuntimeComponent::Xray,
        message: "process exited unexpectedly: signal: 11".into(),
    });
    fixture.state.record_event(&RuntimeEvent::StateChanged {
        profile_id: id,
        state: RuntimeState::Failed {
            message: "no browser core".into(),
        },
    });

    let entries = fixture.state.log_entries();
    assert_eq!(entries[0].level, LogLevel::Error);
    assert_eq!(
        entries[0].message,
        "xray crashed: process exited unexpectedly: signal: 11"
    );
    assert_eq!(entries[1].level, LogLevel::Error);
    assert_eq!(entries[1].message, "failed: no browser core");
}

#[test]
fn the_log_is_capped_and_the_newest_line_survives() {
    let mut fixture = fixture();

    for index in 0..LOG_CAPACITY + 100 {
        fixture.state.push_notice(format!("line {index}"), false);
    }

    let entries = fixture.state.log_entries();
    assert_eq!(entries.len(), LOG_CAPACITY);
    assert_eq!(entries[0].message, "line 100", "the oldest lines fell off");
    assert_eq!(
        entries[LOG_CAPACITY - 1].message,
        format!("line {}", LOG_CAPACITY + 99)
    );
}

#[test]
fn log_rows_name_the_profile_and_put_the_newest_line_first() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.clear_log();

    fixture
        .state
        .record_event(&RuntimeEvent::Stopped { profile_id: id });
    fixture.state.push_notice("Saved.", false);

    let rows = fixture.state.log_rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].who, "app", "the newest line is a window-level one");
    assert_eq!(rows[0].message, "Saved.");
    assert_eq!(rows[1].who, "verify me", "the profile is named, not its id");
    assert_eq!(rows[1].message, "browser stopped");

    fixture.state.clear_log();
    assert!(fixture.state.log_rows().is_empty());
    assert!(
        fixture.state.notice().is_none(),
        "clearing the history does not silence a current problem"
    );
}

#[test]
fn the_panel_log_tail_is_one_profile_and_newest_first() {
    let mut fixture = fixture();
    let id = running_profile(&mut fixture);
    fixture.state.clear_log();
    let other = ProfileId::new();

    fixture.state.push_notice("app level", false);
    fixture
        .state
        .record_event(&RuntimeEvent::Stopped { profile_id: id });
    fixture.state.record_event(&RuntimeEvent::Warning {
        profile_id: other,
        message: "another profile".into(),
    });
    fixture.state.record_event(&RuntimeEvent::Warning {
        profile_id: id,
        message: "this profile".into(),
    });

    let tail = fixture.state.log_tail(id);
    assert_eq!(tail.len(), 2, "{tail:?}");
    assert_eq!(tail[0].message, "this profile", "newest first");
    assert_eq!(tail[0].who, "verify me", "and named");
    assert_eq!(tail[1].message, "browser stopped");
    assert!(
        tail.iter().all(|row| row.message != "app level"),
        "the panel shows this profile only; the Log page has everything"
    );
}

#[test]
fn the_log_page_filter_hides_lines_without_losing_them() {
    let mut fixture = fixture();
    fixture.state.push_notice("Saved.", false);
    fixture.state.push_notice("careful", true);
    let id = running_profile(&mut fixture);
    fixture.state.record_event(&RuntimeEvent::Warning {
        profile_id: id,
        message: "legacy core".into(),
    });

    assert_eq!(fixture.state.log_filter(), LogFilter::All);
    assert_eq!(fixture.state.log_rows().len(), fixture.state.log_len());

    fixture.state.set_log_filter(LogFilter::Warnings);
    let warnings: Vec<&str> = fixture
        .state
        .log_rows()
        .iter()
        .map(|row| row.level.label(en()))
        .collect();
    assert!(!warnings.contains(&"info"), "{warnings:?}");
    assert!(
        fixture.state.log_len() > fixture.state.log_rows().len(),
        "the filter hides lines, it does not drop them"
    );

    fixture.state.set_log_filter(LogFilter::Errors);
    assert!(
        fixture
            .state
            .log_rows()
            .iter()
            .all(|row| row.level == LogLevel::Error),
        "the narrowest filter keeps only errors"
    );
    assert!(fixture.state.log_len() >= 4, "nothing was cleared");
}

#[test]
fn the_activity_log_is_written_to_the_file_as_well() {
    let dir = std::env::temp_dir().join(format!("fp-app-log-{}", CoreId::new()));
    let path = dir.join(crate::log_file::LOG_FILE);
    let mut fixture = fixture_with_log(&dir);
    let id = running_profile(&mut fixture);
    fixture.state.clear_log();

    fixture.state.record_event(&RuntimeEvent::Started {
        profile_id: id,
        browser_pid: 4242,
        xray_pid: None,
        cdp_port: 9222,
        socks_port: None,
    });
    fixture.state.push_notice("Saved.", false);
    fixture.state.push_notice("it broke", true);

    let text = std::fs::read_to_string(&path).expect("the log file was written");
    let lines: Vec<&str> = text.lines().collect();
    // The profile creation line is already in the file: `clear_log` empties
    // the window's history, not what was written down.
    assert_eq!(lines.len(), 4, "{text}");
    assert!(lines[0].contains("app: Created verify me"), "{}", lines[0]);
    assert!(
        lines[1].contains("info")
            && lines[1].contains("verify me")
            && lines[1].contains("browser started (pid 4242, cdp port 9222)"),
        "a line names the profile and what happened: {}",
        lines[1]
    );
    assert!(
        lines[2].contains("info") && lines[2].ends_with("app: Saved."),
        "{}",
        lines[2]
    );
    assert!(
        lines[3].contains("error") && lines[3].ends_with("app: it broke"),
        "{}",
        lines[3]
    );
    assert!(
        lines
            .iter()
            .all(|line| line.starts_with("20") && line.contains('T')),
        "every line carries an absolute UTC time: {text}"
    );
    assert_eq!(
        fixture.state.log_file_status().ok(),
        Some(path.as_path()),
        "the page can say where the file is"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The window must keep working when the log cannot be written, and the
/// failure must be said once rather than per line.
#[test]
fn a_log_file_that_cannot_be_written_is_reported_once() {
    let dir = std::env::temp_dir().join(format!("fp-app-log-bad-{}", CoreId::new()));
    let mut fixture = fixture_full(
        &dir.join("config.json"),
        None,
        Some(crate::log_file::LogFile::at(
            dir.join("missing").join("activity.log"),
        )),
        None,
    );

    fixture.state.push_notice("first", false);
    fixture.state.push_notice("second", false);
    fixture.state.push_notice("third", true);

    let error = fixture
        .state
        .log_file_status()
        .expect_err("the sink failed");
    assert!(error.contains("could not open"), "{error}");
    assert_eq!(
        fixture
            .state
            .toasts()
            .iter()
            .filter(|toast| toast.message.contains("activity log could not be written"))
            .count(),
        1,
        "the failure is reported once, not once per line"
    );
    assert_eq!(
        fixture.state.log_len(),
        3,
        "the in-memory log is unaffected"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
