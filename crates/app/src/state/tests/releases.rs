//! The published cores: the fetch, and the one download in flight.
//!
//! Nothing here dials. The catalog and the downloader are injected into the
//! window, and what these tests drive is the half of the feature that decides
//! what the reader is told and what ends up registered - which is where the
//! mistakes that matter would be: a version taken from a tag instead of a
//! binary, or a file left on disk with nobody told where it is.

use super::*;
use crate::core_releases::testing::release;
use crate::core_releases::{AssetKind, CoreAsset, Downloaded, Platform};

/// An asset to ask for, of the kind a release actually offers.
fn asset() -> CoreAsset {
    CoreAsset {
        name: "ungoogled-chromium_148.0.7778.215-1.1_windows_x64.zip".to_string(),
        url: "https://example.invalid/148/windows_x64.zip".to_string(),
        size: 189_767_686,
        platform: Platform::Windows,
        kind: AssetKind::Portable,
    }
}

#[test]
fn a_fetched_list_is_what_the_dialog_shows() {
    let mut fixture = fixture();

    fixture.state.begin_release_fetch();
    assert!(
        matches!(fixture.state.releases(), ReleaseStatus::Loading),
        "a fetch in flight is not an empty list"
    );

    fixture
        .state
        .finish_release_fetch(Ok(vec![release("148.0.7778.215")]));

    match fixture.state.releases() {
        ReleaseStatus::Listed(releases) => {
            assert_eq!(releases.len(), 1);
            assert_eq!(releases[0].tag, "148.0.7778.215");
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
        .finish_release_fetch(Err("API rate limit exceeded".to_string()));

    match fixture.state.releases() {
        ReleaseStatus::Failed(message) => {
            assert!(message.contains("API rate limit"), "{message}");
            assert!(
                message.contains("published browser cores"),
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
    let asset = asset();

    let started = fixture
        .state
        .begin_core_download("148.0.7778.215", &asset)
        .expect("the first is accepted");
    assert_eq!(started.received, 0);
    assert_eq!(started.total, asset.size, "the row knows the size at once");
    assert_eq!(started.key, format!("148.0.7778.215/{}", asset.name));

    let error = fixture
        .state
        .begin_core_download("144.0.7559.132", &asset)
        .expect_err("the second is refused");
    assert!(error.contains(&asset.name), "{error}");
    assert!(
        fixture
            .state
            .download()
            .is_some_and(|running| { running.release == "148.0.7778.215" }),
        "the refusal did not disturb the download that is running"
    );
}

/// A progress report belongs to one asset, and one for something else must not
/// move the bar that is on screen.
#[test]
fn progress_for_another_asset_is_ignored() {
    let mut fixture = fixture();
    let asset = asset();
    let running = fixture
        .state
        .begin_core_download("148.0.7778.215", &asset)
        .expect("accepted");

    fixture.state.update_download(&running.key, 100, 200);
    assert_eq!(
        fixture.state.download().expect("running").fraction(),
        Some(0.5)
    );

    fixture
        .state
        .update_download("144.0.7559.132/something-else.zip", 200, 200);
    assert_eq!(
        fixture.state.download().expect("running").received,
        100,
        "a report for another asset left this one alone"
    );
}

/// Without a total there is no fraction to draw, and the window does not invent
/// one: the two sizes it does know are still shown.
#[test]
fn a_download_with_no_total_has_no_fraction() {
    let mut fixture = fixture();
    let mut asset = asset();
    asset.size = 0;
    let running = fixture
        .state
        .begin_core_download("148.0.7778.215", &asset)
        .expect("accepted");

    fixture.state.update_download(&running.key, 4096, 0);

    let running = fixture.state.download().expect("running");
    assert_eq!(running.received, 4096);
    assert_eq!(running.total, 0);
    assert_eq!(running.fraction(), None);
}

/// The version a downloaded core is registered with comes from the binary, not
/// from the release tag - which is the whole reason the registration goes
/// through the same probe the Add Core form uses.
#[test]
fn a_downloaded_browser_is_registered_with_the_version_it_reports() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("downloaded", Some("Chromium 148.0.7778.215"));
    let asset = asset();
    fixture
        .state
        .begin_core_download("148.0.7778.215", &asset)
        .expect("accepted");
    fixture.state.update_download(
        &crate::core_releases::asset_key("148.0.7778.215", &asset.name),
        1024,
        2048,
    );

    fixture.state.finish_core_download(
        "148.0.7778.215",
        Ok(Downloaded {
            file: binary.path_buf().with_extension("zip"),
            unpacked: Some(binary.path_buf().parent().expect("a parent").to_path_buf()),
            executable: Some(binary.path_buf()),
        }),
    );

    assert!(fixture.state.download().is_none(), "the row is free again");
    let rows = fixture.state.core_rows().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].core.major, 148);
    assert_eq!(rows[0].core.version, "Chromium 148.0.7778.215");
    assert_eq!(rows[0].core.executable, binary.path_buf());
}

/// A downloaded browser that answers nothing is still on disk, and the reader
/// is told which file it is: the probe's own refusal alone would not say.
#[test]
fn a_downloaded_browser_that_reports_nothing_names_the_file() {
    let mut fixture = fixture();
    let binary = CoreBinary::new("downloaded-silent", None);
    let asset = asset();
    fixture
        .state
        .begin_core_download("148.0.7778.215", &asset)
        .expect("accepted");

    fixture.state.finish_core_download(
        "148.0.7778.215",
        Ok(Downloaded {
            file: binary.path_buf().with_extension("zip"),
            unpacked: None,
            executable: Some(binary.path_buf()),
        }),
    );

    let notice = fixture.state.notice().expect("a refusal is shown");
    assert!(notice.error);
    assert!(
        notice.message.contains("148.0.7778.215"),
        "{}",
        notice.message
    );
    assert!(
        notice
            .message
            .contains(&binary.path_buf().display().to_string()),
        "the reader needs to know which file: {}",
        notice.message
    );
    assert!(
        fixture.state.core_rows().expect("rows").is_empty(),
        "nothing was registered"
    );
}

/// An installer or a disk image is not a core, and the message says what to do
/// with it rather than pretending it failed.
#[test]
fn a_file_that_is_not_a_core_says_where_it_is() {
    let mut fixture = fixture();
    let asset = CoreAsset {
        kind: AssetKind::Installer,
        name: "ungoogled-chromium_148.0.7778.215-1.1_installer_x64.exe".to_string(),
        ..asset()
    };
    fixture
        .state
        .begin_core_download("148.0.7778.215", &asset)
        .expect("accepted");
    let file = fixture
        .state
        .release_dir("148.0.7778.215")
        .join(&asset.name);

    fixture.state.finish_core_download(
        "148.0.7778.215",
        Ok(Downloaded {
            file: file.clone(),
            unpacked: None,
            executable: None,
        }),
    );

    let notice = fixture.state.notice().expect("the reader is told");
    assert!(
        notice.message.contains(&file.display().to_string()),
        "{}",
        notice.message
    );
    assert!(fixture.state.core_rows().expect("rows").is_empty());
}

/// An archive with no browser this program recognises is unpacked anyway, and
/// the directory is named so the reader can go and look.
#[test]
fn an_archive_with_no_browser_says_where_it_was_unpacked() {
    let mut fixture = fixture();
    let asset = asset();
    fixture
        .state
        .begin_core_download("148.0.7778.215", &asset)
        .expect("accepted");
    let directory = fixture
        .state
        .release_dir("148.0.7778.215")
        .join(crate::core_releases::UNPACK_DIR);

    fixture.state.finish_core_download(
        "148.0.7778.215",
        Ok(Downloaded {
            file: directory.join(&asset.name),
            unpacked: Some(directory.clone()),
            executable: None,
        }),
    );

    let notice = fixture.state.notice().expect("the reader is told");
    assert!(
        notice.message.contains(&directory.display().to_string()),
        "{}",
        notice.message
    );
    assert!(
        notice.message.contains("no browser binary"),
        "{}",
        notice.message
    );
}

/// A download that failed reports the worker's own sentence and registers
/// nothing.
#[test]
fn a_failed_download_is_reported_and_registers_nothing() {
    let mut fixture = fixture();
    let asset = asset();
    fixture
        .state
        .begin_core_download("148.0.7778.215", &asset)
        .expect("accepted");

    fixture.state.finish_core_download(
        "148.0.7778.215",
        Err("could not download it: the connection was reset".to_string()),
    );

    assert!(fixture.state.download().is_none());
    let notice = fixture.state.notice().expect("the failure is shown");
    assert!(notice.error);
    assert!(
        notice.message.contains("connection was reset"),
        "{}",
        notice.message
    );
    assert!(fixture.state.core_rows().expect("rows").is_empty());
}

/// A release tag names a directory, and it arrives in a JSON document: the same
/// rule the asset names go through applies to it.
#[test]
fn a_release_directory_cannot_escape_the_cores_directory() {
    let fixture = fixture();
    let directory = fixture.state.release_dir("../../etc");

    assert!(
        directory.starts_with(fixture.state.cores_dir()),
        "{} escaped {}",
        directory.display(),
        fixture.state.cores_dir().display()
    );
    assert_eq!(
        directory,
        fixture.state.cores_dir().join(".._.._etc"),
        "the separators became part of one name"
    );
}
