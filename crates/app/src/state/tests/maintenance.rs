use super::*;

#[test]
fn restore_holds_the_installation_until_its_worker_finishes() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().unwrap();
    let id = fixture.state.create_profile("Original").unwrap();
    fixture
        .state
        .set_restore_path("/not-read-until-worker/config.json");
    fixture.state.set_browser_data_path("/backups/fp");
    let job = fixture.state.begin_restore(RestoreMode::Replace).unwrap();
    assert!(fixture.state.start(id).is_err());
    assert!(fixture.state.create_profile("During restore").is_err());
    assert!(fixture.state.delete_profile(id).is_err());
    let mut edited = fixture.state.profile(id).unwrap();
    edited.name = "Changed".into();
    // The service itself is guarded, independently of the UI entry point.
    assert!(fixture.state.profiles.update(edited).is_err());
    assert!(fixture.state.browser_data_job(Direction::ToBackup).is_err());
    assert!(fixture.runtime.commands.lock().unwrap().is_empty());
    drop(job);
    fixture
        .state
        .finish_restore(std::path::Path::new("unused"), &Err("cancelled".into()));
    assert!(fixture.state.create_profile("After restore").is_ok());
}

#[test]
fn an_export_leaves_the_passwords_out_unless_asked_otherwise() {
    let scratch = Scratch::new("default-choice");
    let mut fixture = fixture();
    let core = seed_core(&fixture);
    let proxy = seed_proxy_holding_a_password(&mut fixture);
    fixture.state.load().expect("load");
    let profile = fixture.state.create_profile("Work laptop").expect("create");
    let mut draft = fixture.state.profile(profile).expect("the profile");
    draft.proxy_id = Some(proxy);
    draft.core_id = core;
    fixture
        .state
        .update_profile(draft)
        .expect("assign the proxy");

    let path = scratch.join("config.json");
    fixture
        .state
        .set_export_path(path.to_string_lossy().to_string());

    // The default is the safe one: a file carrying plain-text passwords is
    // the thing to reach for on purpose, not the thing to get by not reading
    // a checkbox.
    assert!(!fixture.state.export_includes_credentials());
    let report = fixture.state.export_configuration().expect("export");

    assert_eq!(report.profiles, 1);
    assert_eq!(report.credentials, Credentials::Excluded);
    assert_eq!(report.credentials_removed, 1);

    let text = std::fs::read_to_string(&path).expect("read back");
    assert!(!text.contains(EXPORT_SECRET), "{text}");
    // And the profile's references still point at what travelled with it.
    let document = application::ConfigBackup::from_json(&text).expect("parse");
    assert_eq!(document.profiles[0].core_id, document.cores[0].id);
    assert_eq!(document.profiles[0].proxy_id, Some(document.proxies[0].id));
}

#[test]
fn an_export_with_nothing_to_leave_out_says_that_instead() {
    // The opposite sentence, and the reason the count is reported: "left
    // out" over a configuration that had nothing to leave out would describe
    // a file that is missing something it is not.
    let scratch = Scratch::new("nothing-to-leave-out");
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");

    let path = scratch.join("config.json");
    fixture
        .state
        .set_export_path(path.to_string_lossy().to_string());
    let report = fixture.state.export_configuration().expect("export");

    assert_eq!(report.credentials, Credentials::Excluded);
    assert_eq!(report.credentials_removed, 0);
    assert_eq!(report.proxies, 0);

    let message = last_message(&fixture);
    assert!(
        message.contains("no proxy credentials to leave out"),
        "{message}"
    );
}

#[test]
fn a_failed_export_is_reported_as_an_error_and_writes_nothing() {
    let scratch = Scratch::new("failed");
    let blocker = scratch.join("not-a-directory");
    std::fs::write(&blocker, b"").expect("write the blocker");
    let mut fixture = fixture();
    fixture.state.load().expect("load");

    fixture
        .state
        .set_export_path(blocker.join("config.json").to_string_lossy().to_string());
    let error = fixture
        .state
        .export_configuration()
        .expect_err("cannot write");

    assert!(error.contains("could not be written"), "{error}");
    assert!(error.contains("not-a-directory"), "{error}");
    let notice = fixture.state.notice().expect("a banner");
    assert!(notice.error, "{}", notice.message);
}

#[test]
fn an_empty_export_field_means_the_default_and_a_typed_one_means_itself() {
    let mut fixture = fixture();

    // Empty, and whitespace, both mean "the default": a field someone has
    // cleared is not a request to write a file called nothing.
    assert_eq!(
        fixture.state.export_destination(),
        fixture.state.export_default_path()
    );
    fixture.state.set_export_path("   ");
    assert_eq!(
        fixture.state.export_destination(),
        fixture.state.export_default_path()
    );

    fixture.state.set_export_path("  /tmp/my-backup.json  ");
    assert_eq!(
        fixture.state.export_destination(),
        PathBuf::from("/tmp/my-backup.json")
    );
}

#[test]
fn the_default_export_lands_under_the_data_directory_and_names_a_new_file() {
    // Nothing is written here - the default is somewhere the machine keeps
    // real data, and this test only reads what it would be.
    let fixture = fixture();
    let data_dir = fixture.state.export_default_path();
    let parent = data_dir.parent().expect("a parent").to_path_buf();

    assert_eq!(
        parent.file_name().unwrap(),
        crate::paths::EXPORT_DIR,
        "{}",
        parent.display()
    );
    assert!(
        data_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("fp-browser-config-"),
        "{}",
        data_dir.display()
    );
    assert_eq!(
        data_dir.extension().unwrap(),
        "json",
        "{}",
        data_dir.display()
    );
}

#[test]
fn the_export_path_field_survives_a_trip_to_another_page() {
    // The field itself lives in the view, which is not what is tested here;
    // what is testable without a window is that the state behind it keeps
    // what was typed rather than forgetting it between renders.
    let mut fixture = fixture();
    fixture.state.set_export_path("/tmp/kept.json");
    fixture.state.set_page(crate::state::Page::Proxies);
    assert_eq!(fixture.state.export_path(), "/tmp/kept.json");
    fixture.state.set_page(crate::state::Page::Settings);
    assert_eq!(fixture.state.export_path(), "/tmp/kept.json");
}

#[test]
fn an_export_imports_back_into_an_empty_installation() {
    let scratch = Scratch::new("import-round-trip");
    let mut source = fixture();
    let core = seed_core(&source);
    let proxy = seed_proxy_holding_a_password(&mut source);
    let profile = source.state.create_profile("Work laptop").expect("create");
    let mut draft = source.state.profile(profile).expect("the profile");
    draft.proxy_id = Some(proxy);
    draft.core_id = core;
    source
        .state
        .update_profile(draft)
        .expect("assign the proxy");

    let backup = scratch.join("config.json");
    write_backup_from(&mut source, &backup);

    let data_dir = scratch.join("data");
    let mut arriving = fixture_with_data_dir(&data_dir);
    arriving
        .state
        .set_import_path(backup.to_string_lossy().to_string());
    let report = arriving.state.import_configuration().expect("import");

    assert_eq!(report.added.cores, 1);
    assert_eq!(report.added.proxies, 1);
    assert_eq!(report.added.profiles, 1);
    assert!(!report.needs_attention(), "{:?}", report.notes);

    // The message names the file and says what arrived.
    let message = last_message(&arriving);
    assert!(message.contains("config.json"), "{message}");
    assert!(message.contains("were added"), "{message}");

    // The rows are reloaded, so the page shows what just arrived.
    assert_eq!(arriving.state.rows().len(), 1);

    // The profile's directory moved to this machine's data directory:
    // the one the file recorded is not here, and a directory that is not
    // on this machine was never going to be right.
    let stored = arriving
        .profiles
        .get(profile)
        .expect("stored")
        .expect("the profile arrived");
    assert_eq!(
        stored.user_data_dir,
        data_dir.join("profiles").join(profile.to_string())
    );

    // And the proxy arrived without the password the export left out.
    let stored_proxy = arriving
        .proxies
        .get(proxy)
        .expect("stored")
        .expect("the proxy arrived");
    match &stored_proxy.outbound {
        ProxyOutbound::Socks5(socks5) => {
            assert_eq!(socks5.password, None, "{:?}", stored_proxy.outbound);
        }
        other => panic!("expected the socks5 proxy back, got {other:?}"),
    }
}

#[test]
fn importing_the_same_file_twice_adds_nothing_the_second_time() {
    // The ordinary result of importing a file twice, and the reason the
    // report distinguishes "nothing was added" from a refusal: the second
    // import succeeded, it just had nothing to do.
    let scratch = Scratch::new("import-twice");
    let mut source = fixture();
    seed_core(&source);
    source.state.load().expect("load");
    source.state.create_profile("Work laptop").expect("create");

    let backup = scratch.join("config.json");
    write_backup_from(&mut source, &backup);

    let mut arriving = fixture();
    arriving
        .state
        .set_import_path(backup.to_string_lossy().to_string());
    let first = arriving.state.import_configuration().expect("first import");
    assert_eq!(first.added.total(), 2);

    let second = arriving
        .state
        .import_configuration()
        .expect("second import");
    assert_eq!(second.added.total(), 0);
    assert!(!second.needs_attention(), "{:?}", second.notes);
    let message = last_message(&arriving);
    assert!(message.contains("nothing was added"), "{message}");
    assert_eq!(arriving.state.rows().len(), 1);
}

#[test]
fn an_import_whose_core_is_not_in_the_file_skips_its_profiles() {
    // A hand-edited file: the export always pairs a profile with its core,
    // but a file is a file a person can edit, and the rule has to hold
    // when the pairing is broken.
    let scratch = Scratch::new("import-missing-core");
    let mut source = fixture();
    seed_core(&source);
    source.state.load().expect("load");
    source.state.create_profile("Work laptop").expect("create");

    let backup = scratch.join("config.json");
    write_backup_from(&mut source, &backup);

    let mut document = application::ConfigBackup::from_json(
        &std::fs::read_to_string(&backup).expect("read the backup"),
    )
    .expect("parse");
    document.cores.clear();
    let edited = scratch.join("edited.json");
    std::fs::write(&edited, document.to_json().expect("serialise")).expect("write");

    let mut arriving = fixture();
    arriving
        .state
        .set_import_path(edited.to_string_lossy().to_string());
    let report = arriving.state.import_configuration().expect("import");

    assert_eq!(report.added.profiles, 0);
    assert_eq!(report.notes.missing_core, vec!["Work laptop".to_string()]);
    assert!(report.needs_attention());
    // A partial import is a problem the reader has to dismiss, not a
    // toast that drifts away: what was skipped is the thing they came to
    // find out.
    assert!(
        arriving.state.notice().is_some_and(|notice| notice.error),
        "the shortfall is in the banner"
    );
    assert_eq!(arriving.state.rows().len(), 0);
}

#[test]
fn an_import_without_a_path_is_refused_where_the_field_is() {
    let mut arriving = fixture();
    let result = arriving.state.import_configuration();

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Type the path"));
    assert!(arriving.state.notice().is_some_and(|notice| notice.error));
}

#[test]
fn a_file_that_is_not_a_backup_is_said_so() {
    let scratch = Scratch::new("import-foreign");
    let elsewhere = scratch.join("notes.txt");
    std::fs::write(&elsewhere, "not a configuration backup").expect("write");

    let mut arriving = fixture();
    arriving
        .state
        .set_import_path(elsewhere.to_string_lossy().to_string());
    let result = arriving.state.import_configuration();

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("could not be read"));
    assert_eq!(arriving.state.rows().len(), 0);
}

#[test]
fn a_restore_onto_an_empty_installation_reads_the_file() {
    let scratch = Scratch::new("restore-empty");
    let backup = backup_with_one_profile(&scratch);

    let data_dir = scratch.join("data");
    let mut arriving = fixture_with_data_dir(&data_dir);
    arriving
        .state
        .set_restore_path(backup.to_string_lossy().to_string());
    let report = arriving
        .state
        .restore_configuration(RestoreMode::OnlyWhenEmpty)
        .expect("restore");

    assert_eq!(report.added.cores, 1);
    assert_eq!(report.added.profiles, 1);
    assert_eq!(report.removed.total(), 0, "there was nothing to replace");
    assert!(
        arriving.state.notice().is_none(),
        "a clean restore is a toast, not a banner"
    );
    let message = last_message(&arriving);
    assert!(message.contains("were added"), "{message}");
    assert_eq!(arriving.state.rows().len(), 1);
}

/// The precondition, and the whole difference from an import: a populated
/// installation is never replaced without being asked.
#[test]
fn a_restore_without_confirmation_refuses_a_populated_installation() {
    let scratch = Scratch::new("restore-refuses");
    let backup = backup_with_one_profile(&scratch);

    let mut arriving = fixture();
    seed_core(&arriving);
    arriving.state.load().expect("load");
    let kept = arriving.state.create_profile("Keep me").expect("create");
    arriving
        .state
        .set_restore_path(backup.to_string_lossy().to_string());

    let error = arriving
        .state
        .restore_configuration(RestoreMode::OnlyWhenEmpty)
        .expect_err("a populated installation cannot be quietly replaced");

    assert!(error.contains("already holds"), "{error}");
    assert!(
        arriving.state.notice().is_some_and(|notice| notice.error),
        "the refusal owns the banner"
    );
    assert_eq!(arriving.state.rows().len(), 1);
    assert_eq!(arriving.state.rows()[0].profile.name, "Keep me");
    assert!(
        arriving.state.profile(kept).is_some(),
        "nothing was removed"
    );
}

#[test]
fn a_confirmed_restore_replaces_what_is_here() {
    let scratch = Scratch::new("restore-replaces");
    let backup = backup_with_one_profile(&scratch);

    let mut arriving = fixture();
    seed_core(&arriving);
    arriving.state.load().expect("load");
    arriving.state.create_profile("Replace me").expect("create");
    arriving
        .state
        .set_restore_path(backup.to_string_lossy().to_string());

    let report = arriving
        .state
        .restore_configuration(RestoreMode::Replace)
        .expect("restore");

    assert_eq!(
        report.removed.total(),
        2,
        "the core and profile that were here"
    );
    assert_eq!(report.added.total(), 2);
    assert_eq!(arriving.state.rows().len(), 1);
    assert_eq!(arriving.state.rows()[0].profile.name, "Work laptop");
}

#[test]
fn a_restore_without_a_path_is_refused_where_the_field_is() {
    let mut arriving = fixture();
    let result = arriving
        .state
        .restore_configuration(RestoreMode::OnlyWhenEmpty);

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Type the path"));
    assert!(arriving.state.notice().is_some_and(|notice| notice.error));
}

/// Restoring would delete the row of a live browser, leaving a process the
/// window can no longer stop; the refusal names what to stop.
#[test]
fn a_restore_is_blocked_while_a_profile_is_running() {
    let scratch = Scratch::new("restore-running");
    let backup = backup_with_one_profile(&scratch);

    let mut arriving = fixture();
    running_profile(&mut arriving);
    arriving
        .state
        .set_restore_path(backup.to_string_lossy().to_string());

    let error = arriving
        .state
        .restore_configuration(RestoreMode::Replace)
        .expect_err("a running profile blocks the restore");

    assert!(error.contains("Stop these profiles"), "{error}");
    assert!(error.contains("verify me"), "{error}");
    assert_eq!(arriving.state.rows().len(), 1, "nothing was replaced");
}

/// The restore and import fields are separate: a path left over from one verb
/// must not become a path the other acts on.
#[test]
fn the_restore_and_import_paths_are_separate_fields() {
    let mut fixture = fixture();
    fixture.state.set_import_path("/tmp/import.json");
    fixture.state.set_restore_path("/tmp/restore.json");

    assert_eq!(
        fixture.state.import_source(),
        Some(PathBuf::from("/tmp/import.json"))
    );
    assert_eq!(
        fixture.state.restore_source(),
        Some(PathBuf::from("/tmp/restore.json"))
    );
}

#[test]
fn a_browser_data_job_needs_a_directory_and_gathers_every_profile() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    fixture.state.create_profile("Work laptop").expect("create");

    assert!(
        fixture.state.browser_data_job(Direction::ToBackup).is_err(),
        "a copy needs somewhere to go"
    );

    fixture.state.set_browser_data_path("  /backups/fp  ");
    let (job, lease) = fixture
        .state
        .browser_data_job(Direction::ToBackup)
        .expect("a job");
    assert_eq!(job.directory, PathBuf::from("/backups/fp"), "trimmed");
    assert_eq!(job.profiles.len(), 1);
    assert!(job.running.is_empty());
    assert_eq!(job.direction, Direction::ToBackup);
    drop(lease);
}

#[test]
fn a_browser_data_job_is_refused_while_a_profile_is_running() {
    let mut fixture = fixture();
    running_profile(&mut fixture);
    fixture.state.set_browser_data_path("/backups/fp");

    let error = fixture
        .state
        .browser_data_job(Direction::ToBackup)
        .expect_err("a running profile blocks the copy");

    assert!(error.contains("Stop these profiles"), "{error}");
    assert!(error.contains("verify me"), "{error}");
}

#[test]
fn a_browser_data_job_with_no_profiles_is_refused() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    fixture.state.set_browser_data_path("/backups/fp");

    let error = fixture
        .state
        .browser_data_job(Direction::FromBackup)
        .expect_err("nothing to copy");

    assert!(error.contains("no profiles"), "{error}");
}

#[test]
fn a_copy_holds_its_profiles_until_the_worker_gives_them_back() {
    let mut fixture = fixture();
    seed_core(&fixture);
    fixture.state.load().expect("load");
    let id = fixture.state.create_profile("Work laptop").expect("create");
    fixture.state.set_browser_data_path("/backups/fp");

    let (job, lease) = fixture
        .state
        .browser_data_job(Direction::ToBackup)
        .expect("the first copy");
    assert_eq!(job.profiles.len(), 1);

    let error = fixture
        .state
        .browser_data_job(Direction::FromBackup)
        .expect_err("a second copy is refused while the first runs");
    assert!(error.contains("Work laptop"), "{error}");
    assert!(error.contains("copied"), "{error}");

    let error = fixture
        .state
        .start(id)
        .expect_err("starting one of the profiles is refused");
    assert!(error.to_string().contains("Work laptop"), "{error}");

    // A replacement reads its file first, so the path has to be there for the
    // refusal to be about the busy profile rather than about the field.
    fixture.state.set_restore_path("/backups/fp/config.json");
    let error = fixture
        .state
        .restore_configuration(RestoreMode::Replace)
        .expect_err("replacing the configuration is refused");
    assert!(error.contains("Work laptop"), "{error}");

    // The worker finishes: everything it held is free again.
    drop(lease);
    let (job, lease) = fixture
        .state
        .browser_data_job(Direction::FromBackup)
        .expect("the copy after it finishes");
    assert_eq!(job.profiles.len(), 1);
    drop(lease);
}

#[test]
fn finishing_a_browser_data_copy_says_what_happened() {
    let mut fixture = fixture();
    fixture.state.finish_browser_data(
        Direction::ToBackup,
        Ok(BrowserDataReport {
            directory: PathBuf::from("/backups/fp"),
            copied: vec!["Work laptop".to_string()],
            skipped: vec!["Fresh".to_string()],
            bytes: 2 * 1024 * 1024,
        }),
    );

    let message = last_message(&fixture);
    assert!(
        message.contains("Copied the browser data of 1 profile"),
        "{message}"
    );
    assert!(message.contains("/backups/fp"), "{message}");
    assert!(message.contains("2.0 MiB"), "{message}");
    assert!(
        message.contains("Fresh"),
        "a skipped profile is named: {message}"
    );
}

#[test]
fn a_failed_browser_data_copy_is_a_banner() {
    let mut fixture = fixture();
    fixture.state.finish_browser_data(
        Direction::FromBackup,
        Err("Stop these profiles first.".to_string()),
    );

    let notice = fixture.state.notice().expect("a banner");
    assert!(notice.error, "{}", notice.message);
    assert!(
        notice.message.contains("Stop these profiles"),
        "{}",
        notice.message
    );
}

/// The two directions are empty for opposite reasons, and the sentence has to
/// say which one happened: blaming the backup directory for profiles that
/// have never been started would send the reader to the wrong place.
#[test]
fn an_empty_browser_data_copy_blames_the_end_that_was_empty() {
    let empty = BrowserDataReport {
        directory: PathBuf::from("/backups/fp"),
        copied: Vec::new(),
        skipped: vec!["Fresh".to_string()],
        bytes: 0,
    };

    let out = browser_data_summary(Direction::ToBackup, &empty, en());
    assert!(out.contains("no profile has browser data yet"), "{out}");
    assert!(
        !out.contains("/backups/fp"),
        "the source is at fault: {out}"
    );

    let back = browser_data_summary(Direction::FromBackup, &empty, en());
    assert!(back.contains("/backups/fp"), "{back}");
    assert!(back.contains("holds no browser data"), "{back}");
}

/// A file written without credentials restores proxies that no longer carry
/// them. An import already says so; a restore that stayed silent would leave
/// the reader with a proxy that fails authentication and no explanation.
#[test]
fn a_restore_from_a_credential_free_file_says_a_proxy_may_need_them_again() {
    let mut report = RestoreReport::default();
    report.added.proxies = 1;
    report.notes.credentials_excluded = true;

    let sentence = restore_summary(&report, std::path::Path::new("/tmp/config.json"), en());

    assert!(sentence.contains("were added"), "{sentence}");
    assert!(sentence.contains("without proxy credentials"), "{sentence}");
    assert!(
        sentence.contains("may need them typed in again"),
        "{sentence}"
    );
}
