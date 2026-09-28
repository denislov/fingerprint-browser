//! The engine card and the Download Xray dialog.
//!
//! The catalog and the downloader are fakes, so nothing here dials anything.
//! What these tests are about is the part of the feature the reader sees, and
//! the one part of it that is not like the core dialog above it: the build
//! offered is the one for this machine, named by architecture, because an engine
//! archive carries the architecture in its file name and a wrong one does not
//! run at all.

use super::*;
use crate::core_releases::Platform;
use crate::xray_releases::testing::{asset, release};
use crate::xray_releases::{Architecture, XrayRelease};

/// A published release whose only assets are for another architecture.
fn published_elsewhere(tag: &str) -> XrayRelease {
    let foreign = match Architecture::host().expect("a host architecture") {
        Architecture::X86_64 => Architecture::Mips64,
        _ => Architecture::X86_64,
    };
    XrayRelease {
        tag: tag.to_string(),
        published: "2026-02-01".to_string(),
        prerelease: false,
        assets: vec![asset(tag, Platform::host(), foreign)],
    }
}

/// A configuration file of this test's own, emptied first.
///
/// The shared harness file outlives a run, and these tests write the engine path
/// into it: a test that asks whether a fresh installation has an engine has to
/// have one.
fn own_config(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("fp-ui-xray-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a temporary directory");
    dir.join("config.json")
}

/// The card's sentence names the build and says whether it is there, which is
/// the whole of what the card is for: "downloaded" and "gone" are not the same
/// news, and an engine that was deleted is the case a reader cannot see for
/// themselves.
#[test]
fn the_card_says_which_engine_is_at_the_path_and_whether_it_is_there() {
    let en = crate::text::en();
    let downloaded = en.xray_engine_state(Some("v26.3.27"), true);
    let deleted = en.xray_engine_state(Some("v26.3.27"), false);
    let unknown = en.xray_engine_state(None, false);

    assert!(downloaded.contains("v26.3.27"), "{downloaded}");
    assert!(
        deleted.contains("v26.3.27"),
        "the build is still named once its file is gone: {deleted}"
    );
    assert_ne!(
        downloaded, deleted,
        "an engine that is there and one that was deleted are not the same news"
    );
    assert_ne!(
        downloaded, unknown,
        "and neither is an engine nobody recorded"
    );
    assert!(
        crate::text::text(crate::text::Lang::Zh)
            .xray_engine_state(Some("v26.3.27"), true)
            .contains("v26.3.27"),
        "the build is named in either language"
    );
}

/// Opens the Download Xray dialog from the Settings page's Runtime group.
fn open_engine_downloads(cx: &mut gpui_kit::VisualTestContext) {
    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    scroll_settings_to(cx, "download-xray");
    cx.update(|window, cx| window.click("download-xray", cx));
    settle(cx);
}

/// The card sits with the setting it writes: it says what is at the path in
/// force, and it is where an engine is fetched from.
#[gpui_kit::test]
fn the_engine_card_is_on_the_settings_page_beside_its_setting(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let config = own_config("card");
    let (view, _runtime) = view_with_engines(
        cx,
        Some(&config),
        Arc::new(FakeXrayCatalog::listing(Vec::new())),
        Arc::new(FakeXrayDownloader::unpacking("")),
    );
    let cx = window(cx, &view);

    cx.update(|window, cx| window.click("nav-Settings", cx));
    settle(cx);
    scroll_settings_to(cx, "download-xray");

    assert!(
        cx.update(|window, _| window.try_find("xray-engine").is_some()),
        "the card is on the Runtime group"
    );
    assert!(
        !view.read_with(cx, |view, _| view.state().xray_engine_present()),
        "a fresh installation has no engine at the default path"
    );
    let rows = view.read_with(cx, |view, _| view.state().setting_rows());
    let row = rows
        .iter()
        .find(|row| row.key == crate::settings::SettingKey::XrayExecutable)
        .expect("the engine path is a setting");
    assert!(
        row.value.ends_with("xray") || row.value.ends_with("xray.exe"),
        "the row and the card are about the same path: {}",
        row.value
    );
}

/// The dialog lists what the repository published: one row per version.
#[gpui_kit::test]
fn the_engine_dialog_lists_the_published_builds(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let catalog = Arc::new(FakeXrayCatalog::listing(vec![
        release("v26.3.27"),
        release("v26.2.6"),
    ]));
    let (view, _runtime) = view_with_engines(
        cx,
        None,
        catalog.clone(),
        Arc::new(FakeXrayDownloader::unpacking("")),
    );
    let cx = window(cx, &view);

    open_engine_downloads(cx);
    wait_for_xray_releases(cx, &view);
    settle(cx);

    assert_eq!(
        catalog.calls(),
        1,
        "opening the dialog asks for the list once"
    );
    for tag in ["v26.3.27", "v26.2.6"] {
        assert!(
            cx.update(|window, _| window.try_find(format!("xray-release-{tag}")).is_some()),
            "the row for {tag} is listed"
        );
    }
}

/// The one place this dialog is not the core dialog's: it offers the build for
/// this machine, named by platform and architecture, and a release that
/// published nothing for this machine says so instead of offering a wrong file.
#[gpui_kit::test]
fn the_build_offered_is_this_machines_and_a_release_without_one_says_so(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let catalog = Arc::new(FakeXrayCatalog::listing(vec![
        release("v26.3.27"),
        published_elsewhere("v26.2.6"),
    ]));
    let (view, _runtime) = view_with_engines(
        cx,
        None,
        catalog,
        Arc::new(FakeXrayDownloader::unpacking("")),
    );
    let cx = window(cx, &view);

    open_engine_downloads(cx);
    wait_for_xray_releases(cx, &view);
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("xray-download-v26.3.27").is_some()),
        "the release with a build for this machine has its button"
    );
    assert!(
        cx.update(|window, _| window.try_find("xray-download-v26.2.6").is_none()),
        "a release with nothing for this machine offers no button to get it wrong with"
    );
    assert!(
        cx.update(|window, _| window.try_find("xray-release-none-v26.2.6").is_some()),
        "and it says so where the button would be"
    );
}

/// Clicking downloads that asset, unpacks the engine, and writes the path into
/// the setting a proxied profile starts from.
#[gpui_kit::test]
fn downloading_the_build_for_this_machine_records_it(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let catalog = Arc::new(FakeXrayCatalog::listing(vec![release("v26.3.27")]));
    let downloader = Arc::new(FakeXrayDownloader::unpacking("an engine"));
    let config = own_config("download");
    let (view, _runtime) = view_with_engines(cx, Some(&config), catalog, downloader.clone());
    let cx = window(cx, &view);

    open_engine_downloads(cx);
    wait_for_xray_releases(cx, &view);
    settle(cx);

    cx.update(|window, cx| window.click("xray-download-v26.3.27", cx));
    wait_for_xray_download(cx, &view);
    settle(cx);

    let jobs = downloader.jobs();
    assert_eq!(jobs.len(), 1, "one click asks for one file");
    assert_eq!(jobs[0].0, "v26.3.27");
    let architecture = Architecture::host().expect("a host architecture");
    assert!(
        jobs[0].1.contains(architecture.token()),
        "the asset asked for is this machine's architecture: {}",
        jobs[0].1
    );
    assert!(
        downloader
            .progress()
            .iter()
            .any(|(received, _)| *received > 0),
        "the download reported how far it had got"
    );

    let (path, present, said) = view.read_with(cx, |view, _| {
        (
            view.state().xray_executable().to_path_buf(),
            view.state().xray_engine_present(),
            view.state()
                .toasts()
                .iter()
                .map(|toast| toast.message.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    });
    let expected = view.read_with(cx, |view, _| view.state().xray_release_dir("v26.3.27"));
    assert_eq!(
        path.parent(),
        Some(expected.as_path()),
        "the engine in force is the one just downloaded"
    );
    assert!(present, "and it is on disk");
    assert!(
        view.read_with(cx, |view, _| view.state().notice().is_none()),
        "a download that worked leaves no banner behind"
    );
    assert!(
        said.contains("next start"),
        "when it takes effect is the part that has to be said: {said}"
    );
}

/// A download that failed is reported, and the working engine the reader already
/// had is still the one in force.
#[gpui_kit::test]
fn a_failed_download_is_reported_and_leaves_the_engine_alone(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let catalog = Arc::new(FakeXrayCatalog::listing(vec![release("v26.3.27")]));
    let downloader = Arc::new(FakeXrayDownloader::refusing("the archive did not unpack"));
    let config = own_config("failed");
    let (view, _runtime) = view_with_engines(cx, Some(&config), catalog, downloader);
    let cx = window(cx, &view);
    let before = view.read_with(cx, |view, _| view.state().xray_executable().to_path_buf());

    open_engine_downloads(cx);
    wait_for_xray_releases(cx, &view);
    settle(cx);
    cx.update(|window, cx| window.click("xray-download-v26.3.27", cx));
    wait_for_xray_download(cx, &view);
    settle(cx);

    let (path, notice) = view.read_with(cx, |view, _| {
        (
            view.state().xray_executable().to_path_buf(),
            view.state().notice().cloned(),
        )
    });
    assert_eq!(path, before, "a failed download takes nothing away");
    let notice = notice.expect("the banner says why");
    assert!(notice.error);
    assert!(
        notice.message.contains("did not unpack"),
        "{}",
        notice.message
    );
}

/// Refresh asks the repository again, and the dialog is rebuilt from the new
/// answer.
#[gpui_kit::test]
fn refreshing_the_engine_list_asks_the_repository_again(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let catalog = Arc::new(FakeXrayCatalog::listing(vec![release("v26.3.27")]));
    let (view, _runtime) = view_with_engines(
        cx,
        None,
        catalog.clone(),
        Arc::new(FakeXrayDownloader::unpacking("")),
    );
    let cx = window(cx, &view);

    open_engine_downloads(cx);
    wait_for_xray_releases(cx, &view);
    settle(cx);
    assert_eq!(catalog.calls(), 1);

    cx.update(|window, cx| window.click("xray-download-refresh", cx));
    wait_for_xray_releases(cx, &view);
    settle(cx);

    assert_eq!(catalog.calls(), 2, "the button asks for the list again");
}

/// A list that could not be read is said in the dialog and in the banner, and
/// nothing pretends to be a row.
#[gpui_kit::test]
fn a_fetch_that_failed_is_shown_in_the_engine_dialog(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view_with_engines(
        cx,
        None,
        Arc::new(FakeXrayCatalog::failing("API rate limit exceeded")),
        Arc::new(FakeXrayDownloader::unpacking("")),
    );
    let cx = window(cx, &view);

    open_engine_downloads(cx);
    wait_for_xray_releases(cx, &view);
    settle(cx);

    assert!(
        cx.update(|window, _| window.try_find("xray-download-failed").is_some()),
        "the dialog says why"
    );
    assert!(
        cx.update(|window, _| window.try_find("xray-download-scroll").is_none()),
        "a failed fetch is not an empty list of rows"
    );
    let notice = view
        .read_with(cx, |view, _| view.state().notice().cloned())
        .expect("the banner says so too");
    assert!(notice.error);
    assert!(
        notice.message.contains("API rate limit"),
        "{}",
        notice.message
    );
}
