//! The published engines: the fetch, the one download, and what becomes of it.
//!
//! Nothing here dials. The catalog and the downloader are injected into the
//! window, and what these tests drive is the half of the feature that decides
//! what the reader is told and what the configuration ends up saying - which is
//! where the mistakes that matter would be: an engine reported as installed
//! while the environment is overriding it, or a setting rewritten by a download
//! that failed.

use super::*;
use crate::core_releases::Platform;
use crate::settings::XRAY_BIN_ENV;
use crate::xray_releases::testing::{asset, release};
use crate::xray_releases::{Architecture, Downloaded};
use std::path::Path;

/// What a finished download leaves behind, with the binary on disk.
///
/// The file is really created: the settings page reads whether there is
/// something at the path, and a fake that answered with a path nothing is at
/// would test the fake.
/// A configuration file of this test's own, emptied first.
///
/// The shared fixture's file outlives a run and these tests write the engine path
/// into it: a test that asks what a fresh installation has at that path needs one.
fn own_config(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fp-xray-state-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir.join("config.json")
}

fn downloaded_in(directory: &Path) -> Downloaded {
    std::fs::create_dir_all(directory).expect("the release directory");
    let executable = directory.join(if cfg!(windows) { "xray.exe" } else { "xray" });
    std::fs::write(&executable, b"an engine").expect("the engine");
    Downloaded {
        executable,
        archive_sha256: "b".repeat(64),
        digest_url: "https://example.invalid/Xray-linux-64.zip.dgst".to_string(),
        skipped: 29 * 1024 * 1024,
    }
}

#[test]
fn a_fetched_list_is_what_the_dialog_shows() {
    let mut fixture = fixture();

    fixture.state.begin_xray_release_fetch();
    assert!(
        matches!(fixture.state.xray_releases(), XrayReleaseStatus::Loading),
        "a fetch in flight is not an empty list"
    );

    fixture
        .state
        .finish_xray_release_fetch(Ok(vec![release("v26.3.27")]));

    match fixture.state.xray_releases() {
        XrayReleaseStatus::Listed(releases) => {
            assert_eq!(releases.len(), 1);
            assert_eq!(releases[0].tag, "v26.3.27");
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// A fetch that failed says so twice: in the dialog, where the reader is
/// looking, and in the banner, which is what they see if the dialog was closed
/// while the request was out.
#[test]
fn a_failed_fetch_is_a_reason_in_the_dialog_and_in_the_banner() {
    let mut fixture = fixture();

    fixture
        .state
        .finish_xray_release_fetch(Err("API rate limit exceeded".to_string()));

    match fixture.state.xray_releases() {
        XrayReleaseStatus::Failed(message) => {
            assert!(message.contains("API rate limit"), "{message}");
            assert!(
                message.contains("Xray builds"),
                "the frame is the window's: {message}"
            );
        }
        other => panic!("expected a failure, got {other:?}"),
    }
    let notice = fixture.state.notice().expect("the banner says so too");
    assert!(notice.error);
    assert!(
        notice.message.contains("API rate limit"),
        "{}",
        notice.message
    );
}

#[test]
fn one_download_at_a_time_and_the_refusal_names_the_one_running() {
    let mut fixture = fixture();
    let asset = asset("v26.3.27", Platform::host(), Architecture::host().unwrap());

    let started = fixture
        .state
        .begin_xray_download("v26.3.27", &asset)
        .expect("the first is accepted");
    assert_eq!(started.received, 0);
    assert_eq!(started.total, asset.size, "the row knows the size at once");
    assert_eq!(started.key, format!("v26.3.27/{}", asset.name));

    let error = fixture
        .state
        .begin_xray_download("v26.2.6", &asset)
        .expect_err("the second is refused");
    assert!(error.contains(&asset.name), "{error}");
    assert!(
        fixture
            .state
            .xray_download()
            .is_some_and(|running| running.release == "v26.3.27"),
        "the refusal did not disturb the download that is running"
    );
}

/// A progress report belongs to one asset, and one for something else must not
/// move the bar that is on screen.
#[test]
fn progress_for_another_asset_is_ignored() {
    let mut fixture = fixture();
    let asset = asset("v26.3.27", Platform::host(), Architecture::host().unwrap());
    let running = fixture
        .state
        .begin_xray_download("v26.3.27", &asset)
        .expect("accepted");

    fixture.state.update_xray_download(&running.key, 100, 200);
    assert_eq!(
        fixture.state.xray_download().expect("running").fraction(),
        Some(0.5)
    );

    fixture
        .state
        .update_xray_download("v26.2.6/something-else.zip", 200, 200);
    assert_eq!(
        fixture.state.xray_download().expect("running").received,
        100,
        "a report for another asset left this one alone"
    );
}

/// The whole point of the feature: what comes down becomes the path the program
/// starts engines from, and the reader is told when that takes effect rather
/// than being left to guess.
#[test]
fn a_finished_download_becomes_the_setting_and_says_when_it_applies() {
    let mut fixture = fixture();
    let asset = asset("v26.3.27", Platform::host(), Architecture::host().unwrap());
    let directory = fixture.state.xray_release_dir("v26.3.27");
    let downloaded = downloaded_in(&directory);

    fixture
        .state
        .begin_xray_download("v26.3.27", &asset)
        .expect("accepted");
    fixture
        .state
        .finish_xray_download("v26.3.27", Ok(downloaded));

    assert!(
        fixture.state.xray_download().is_none(),
        "the row is free again"
    );
    assert_eq!(
        fixture.state.xray_executable(),
        directory.join(if cfg!(windows) { "xray.exe" } else { "xray" }),
        "the engine that was downloaded is the one in force"
    );
    assert!(fixture.state.xray_engine_present());

    // A success is a toast rather than the banner: the banner is for problems,
    // and this one has none.
    assert!(
        fixture.state.notice().is_none(),
        "a download that worked leaves no banner behind"
    );
    let said = fixture
        .state
        .toasts()
        .iter()
        .map(|toast| toast.message.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(said.contains("v26.3.27"), "{said}");
    assert!(
        said.contains("next start"),
        "when it takes effect is the part a reader has to know: {said}"
    );
}

/// The one arrangement that must not be reported as success: the configuration
/// was written and an environment variable outranks it, so the engine that runs
/// will be something else.
#[test]
fn a_download_an_environment_variable_outranks_is_not_reported_as_installed() {
    let mut fixture = fixture_with_env_xray(Path::new("/opt/their-own/xray"));
    let asset = asset("v26.3.27", Platform::host(), Architecture::host().unwrap());
    let directory = fixture.state.xray_release_dir("v26.3.27");
    let downloaded = downloaded_in(&directory);

    fixture
        .state
        .begin_xray_download("v26.3.27", &asset)
        .expect("accepted");
    fixture
        .state
        .finish_xray_download("v26.3.27", Ok(downloaded));

    assert_eq!(
        fixture.state.xray_executable(),
        Path::new("/opt/their-own/xray"),
        "the environment is still the one in force"
    );
    let notice = fixture.state.notice().expect("the banner says so");
    assert!(notice.error, "this is not a success: {}", notice.message);
    assert!(
        notice.message.contains(XRAY_BIN_ENV),
        "the sentence has to name the variable to unset: {}",
        notice.message
    );
    assert!(
        notice.message.contains("v26.3.27"),
        "and the build that is on disk: {}",
        notice.message
    );
}

/// A download that failed leaves the configuration exactly as it was: an engine
/// path is what a proxied profile launches, and a failed download must not take
/// the working one away.
#[test]
fn a_failed_download_leaves_the_setting_alone() {
    let mut fixture = fixture();
    let before = fixture.state.xray_executable().to_path_buf();
    let asset = asset("v26.3.27", Platform::host(), Architecture::host().unwrap());

    fixture
        .state
        .begin_xray_download("v26.3.27", &asset)
        .expect("accepted");
    fixture
        .state
        .finish_xray_download("v26.3.27", Err("the archive did not unpack".to_string()));

    assert_eq!(fixture.state.xray_executable(), before);
    let notice = fixture.state.notice().expect("the banner says why");
    assert!(notice.error);
    assert!(
        notice.message.contains("did not unpack"),
        "{}",
        notice.message
    );
}

/// Which release is on disk is read back out of the path, so a reader who points
/// the row at their own binary stops being told about a downloaded one.
#[test]
fn the_tag_is_read_back_out_of_the_path_this_window_wrote() {
    let mut fixture = fixture_with_config(&own_config("tag"));
    assert_eq!(
        fixture.state.downloaded_xray_tag(),
        None,
        "nothing has been downloaded yet"
    );

    let directory = fixture.state.xray_release_dir("v26.3.27");
    let downloaded = downloaded_in(&directory);
    fixture
        .state
        .update_setting(
            SettingKey::XrayExecutable,
            &downloaded.executable.display().to_string(),
        )
        .expect("the setting takes");

    assert_eq!(
        fixture.state.downloaded_xray_tag().as_deref(),
        Some("v26.3.27")
    );
}

/// Where the binary came from is written to the log, and the log outlives the
/// banner: what a report is asked for weeks later is the hash and the file to
/// check it against.
#[test]
fn the_provenance_is_written_to_the_log() {
    let mut fixture = fixture();
    let asset = asset("v26.3.27", Platform::host(), Architecture::host().unwrap());
    let directory = fixture.state.xray_release_dir("v26.3.27");
    let downloaded = downloaded_in(&directory);
    let hash = downloaded.archive_sha256.clone();

    fixture
        .state
        .begin_xray_download("v26.3.27", &asset)
        .expect("accepted");
    fixture
        .state
        .finish_xray_download("v26.3.27", Ok(downloaded));

    let logged = fixture
        .state
        .log_rows()
        .iter()
        .map(|row| row.message.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(logged.contains("v26.3.27"), "{logged}");
    assert!(logged.contains(&hash), "the hash is recorded: {logged}");
    assert!(logged.contains(".dgst"), "and where to check it: {logged}");
}
