//! The Xray card on the Settings page, and the dialog that downloads an engine.
//!
//! The card is where the engine's own row lives: what is at the configured path,
//! whether this window put it there, and the one button that fetches one. It sits
//! in the Runtime group beside the settings it writes rather than on a page of
//! its own, because an engine is a fact about this installation and not a
//! collection the reader curates.
//!
//! The dialog behind the button offers one build per release - the one for this
//! machine - rather than the three system buttons the core dialog has. That is
//! the whole of the difference and it is deliberate: Xray's archives carry the
//! architecture in their file names, so offering "Linux" without knowing which
//! Linux at the far end would be offering a coin toss. What the reader gets
//! instead is the build this program can run, named by platform and architecture.
//!
//! The caption, the progress bar and the footer's two buttons are the core
//! dialog's too, drawn by [`super::downloads`] for both; what is this dialog's
//! own is the row, which offers the build for this machine rather than three
//! system buttons.

use super::*;
use crate::xray_releases::{DownloadJob, REPOSITORY, XrayAsset, XrayRelease};
use gpui_kit::component::dialog::Dialog;

/// The dialog's width: the two reading columns and the one build column beside
/// them, with the page's own padding.
const XRAY_DIALOG_WIDTH: f32 = 760.0;
/// The version column, sized for `v26.3.27` and the pre-release pill under it.
const COLUMN_RELEASE: f32 = 170.0;
/// The publication date, sized for `2026-03-27`.
const COLUMN_PUBLISHED: f32 = 110.0;
/// The build button, sized for `Windows · 32-bit x86`.
const XRAY_BUTTON: f32 = 200.0;
/// How tall the release list may grow before it scrolls.
const XRAY_LIST_HEIGHT: f32 = 380.0;

/// What the engine card needs that is not already in [`AppState`].
///
/// A value rather than three positional parameters, for the reason
/// [`SettingsExport`](super::settings::SettingsExport) is one: the set grows, and
/// every call site should not have to change when it does.
pub(super) struct XrayCard {
    /// Whether there is an executable at the configured path.
    pub(super) present: bool,
    /// The release this window downloaded there, when it was this window.
    pub(super) tag: Option<String>,
    /// The path in force, as the row above shows it.
    pub(super) path: String,
}

/// The engine card: what is at the path in force, and the way to fetch one.
pub(super) fn xray_card(
    card: XrayCard,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> impl IntoElement {
    let XrayCard { present, tag, path } = card;
    components::card(p)
        .id("xray-engine")
        .test_support()
        .child(components::card_heading(
            icons::glyph::XRAY,
            t.xray_card_title,
            t.xray_card_body,
            p,
        ))
        .child(components::card_note(
            t.xray_engine_state(tag.as_deref(), present),
            p,
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_3()
                .child(
                    Button::new("download-xray")
                        .icon(icons::action(icons::glyph::DOWNLOAD_CORE))
                        .label(t.download_xray)
                        .outline()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_open_xray_downloads(window, cx)
                        })),
                )
                .child(
                    div()
                        .id("xray-engine-path")
                        .test_support()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(p.dim))
                        .child(path),
                ),
        )
        .child(components::card_note(
            t.xray_download_restart_note.to_string(),
            p,
        ))
}

/// The Download Xray dialog's body, as a view of its own.
///
/// A second entity rather than reads of [`AppView`] inside the dialog's closure,
/// for the reason [`super::cores::CoreDownloads`] is one: the dialog is drawn
/// while the window it belongs to is being read, and reading that window from
/// there is the re-entrancy the entity map refuses. So the window pushes each
/// answer into this one, and the dialog reads this one instead.
pub struct XrayDownloads {
    text: &'static Text,
    view: WeakEntity<AppView>,
    releases: XrayReleaseStatus,
    download: Option<XrayDownload>,
}

impl XrayDownloads {
    fn new(
        view: WeakEntity<AppView>,
        text: &'static Text,
        releases: XrayReleaseStatus,
        download: Option<XrayDownload>,
    ) -> Self {
        Self {
            text,
            view,
            releases,
            download,
        }
    }

    /// Takes the copy of the window's state this draws.
    fn show(&mut self, releases: XrayReleaseStatus, download: Option<XrayDownload>) {
        self.releases = releases;
        self.download = download;
    }

    /// Whether a list is on its way, for the footer's Refresh button.
    fn is_loading(&self) -> bool {
        matches!(self.releases, XrayReleaseStatus::Loading)
    }
}

impl Render for XrayDownloads {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let t = self.text;
        let mut body = div()
            .id("xray-download-body")
            .test_support()
            .flex()
            .flex_col()
            .gap_2()
            .min_h_0()
            .child(downloads::caption(
                "xray-download-source",
                t.xray_download_source(REPOSITORY),
                t.xray_download_intro,
                p,
            ));

        match &self.releases {
            XrayReleaseStatus::Unasked | XrayReleaseStatus::Loading => {
                body = body.child(downloads::note(
                    "xray-download-loading",
                    t.xray_download_loading,
                    p.muted,
                ));
            }
            XrayReleaseStatus::Failed(message) => {
                body = body.child(downloads::failure("xray-download-failed", message, p));
            }
            XrayReleaseStatus::Listed(releases) if releases.is_empty() => {
                body = body.child(downloads::note(
                    "xray-download-empty",
                    t.xray_download_empty,
                    p.muted,
                ));
            }
            XrayReleaseStatus::Listed(releases) => {
                let download = self.download.as_ref();
                body =
                    body.child(xray_download_columns(p, t)).child(
                        div()
                            .id("xray-download-scroll")
                            .test_support()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .max_h(px(XRAY_LIST_HEIGHT))
                            .overflow_y_scroll()
                            .children(releases.iter().map(|release| {
                                xray_release_row(&self.view, release, download, p, t)
                            })),
                    );
            }
        }

        body
    }
}

/// The dialog's chrome around the body [`XrayDownloads`] draws.
fn xray_download_dialog(
    downloads: Entity<XrayDownloads>,
    view: WeakEntity<AppView>,
    text: &'static Text,
    dialog: Dialog,
    cx: &mut App,
) -> Dialog {
    let loading = downloads.read(cx).is_loading();
    dialog
        .title(text.xray_download_title)
        .w(px(XRAY_DIALOG_WIDTH))
        .child(downloads)
        .footer(
            DialogFooter::new()
                .child(downloads::open_dir_button(
                    "xray-download-open-folder",
                    text.xray_download_open_folder,
                    &view,
                    AppView::on_open_xray_dir,
                ))
                .child(downloads::refresh_button(
                    "xray-download-refresh",
                    text.xray_download_refresh,
                    &view,
                    loading,
                    AppView::on_fetch_xray_releases,
                ))
                .child(
                    DialogClose::new()
                        .trigger(|button| button.label(text.xray_download_close).outline()),
                ),
        )
        .on_close(|_, _, _| ())
}

/// The Download Xray dialog, and the two things it asks workers to do.
impl AppView {
    /// Opens the dialog, asking for the release list the first time.
    pub(super) fn on_open_xray_downloads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state.xray_releases(), XrayReleaseStatus::Unasked) {
            self.on_fetch_xray_releases(cx);
        }
        let view = cx.entity().downgrade();
        let text = self.state.text();
        let releases = self.state.xray_releases().clone();
        let download = self.state.xray_download().cloned();
        let downloads = cx.new(|_| XrayDownloads::new(view.clone(), text, releases, download));
        self.xray_downloads = Some(downloads.clone());

        window.open_dialog(cx, move |dialog, _window, cx| {
            xray_download_dialog(downloads.clone(), view.clone(), text, dialog, cx)
        });
        cx.notify();
    }

    /// Asks for the release list again.
    pub(super) fn on_fetch_xray_releases(&mut self, cx: &mut Context<Self>) {
        self.state.begin_xray_release_fetch();
        let catalog = Arc::clone(&self.xray_catalog);
        let sender = self.xray_tx.clone();
        std::thread::spawn(move || {
            let result = catalog.list();
            let _ = sender.send(XrayReleaseEvent::Listed(result));
        });
        self.sync_xray_downloads(cx);
        cx.notify();
    }

    /// Downloads one asset of one release and records the engine it contains.
    ///
    /// Recording happens when the worker's answer arrives, not here: it writes
    /// the configuration this window holds, and it is where a setting that an
    /// environment variable outranks is noticed - see
    /// [`AppState::finish_xray_download`].
    pub(super) fn on_download_xray(
        &mut self,
        release: String,
        asset: XrayAsset,
        cx: &mut Context<Self>,
    ) {
        let t = self.state.text();
        if let Err(message) = self.state.begin_xray_download(&release, &asset) {
            self.state.push_notice(message, true);
            cx.notify();
            return;
        }
        let job = DownloadJob {
            text: t,
            release: release.clone(),
            asset: asset.clone(),
            directory: self.state.xray_release_dir(&release),
        };
        // Said as a toast as well as shown on the row: the dialog can be closed
        // while twenty megabytes are arriving, and the log line is then the only
        // record of what this window is doing.
        self.state.push_notice(
            t.xray_release_downloading(&release, asset.architecture.label()),
            false,
        );

        let downloader = Arc::clone(&self.xray_downloader);
        let sender = self.xray_tx.clone();
        let key = crate::releases::asset_key(&job.release, &job.asset.name);
        std::thread::spawn(move || {
            let mut report = |received: u64, total: u64| {
                let _ = sender.send(XrayReleaseEvent::Progress {
                    key: key.clone(),
                    received,
                    total,
                });
            };
            let result = downloader.download(&job, &mut report);
            let _ = sender.send(XrayReleaseEvent::Downloaded {
                release: job.release.clone(),
                result,
            });
        });
        self.sync_xray_downloads(cx);
        cx.notify();
    }

    /// Opens the directory downloaded engines are kept in.
    ///
    /// Created first when it is not there: nothing downloaded yet is not an
    /// error, and a reader who asked to see where engines go should be shown the
    /// place rather than told it does not exist.
    pub(super) fn on_open_xray_dir(&mut self, cx: &mut Context<Self>) {
        let directory = self.state.xray_dir();
        let t = self.state.text();
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.state.push_notice(
                t.release_disk_failed(&directory.display().to_string(), &error.to_string()),
                true,
            );
            cx.notify();
            return;
        }
        let opener = Arc::clone(&self.opener);
        let sender = self.open_tx.clone();
        std::thread::spawn(move || {
            let result = opener.open(&directory, t);
            let _ = sender.send((directory, result));
        });
        cx.notify();
    }

    /// Copies the engine state into the dialog's own view, when one is open.
    pub(super) fn sync_xray_downloads(&mut self, cx: &mut Context<Self>) {
        let Some(downloads) = self.xray_downloads.clone() else {
            return;
        };
        let releases = self.state.xray_releases().clone();
        let download = self.state.xray_download().cloned();
        downloads.update(cx, |downloads, cx| {
            downloads.show(releases, download);
            cx.notify();
        });
    }
}

/// The list's headings, laid out with the rows' own widths.
fn xray_download_columns(p: Palette, t: &'static Text) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_4()
        .px_3()
        // Invisible, and there for the same reason the core list's header has
        // one: a row is drawn with a 1px border and the heading is not, so
        // without this the columns would sit one pixel to the left of the values
        // they name.
        .border_1()
        .border_color(hsla(0.0, 0.0, 0.0, 0.0))
        .text_xs()
        .text_color(rgb(p.dim))
        .child(
            div()
                .id("xray-column-release")
                .test_support()
                .w(px(COLUMN_RELEASE))
                .flex_shrink_0()
                .child(t.xray_download_column_version),
        )
        .child(
            div()
                .id("xray-column-published")
                .test_support()
                .w(px(COLUMN_PUBLISHED))
                .flex_shrink_0()
                .child(t.xray_download_column_published),
        )
        .child(
            div()
                .id("xray-column-build")
                .test_support()
                .flex_1()
                .min_w_0()
                .text_right()
                .child(t.xray_download_column_build),
        )
}

/// One published version: what it is, when it came out, and the build for this
/// machine.
///
/// A release that published nothing for this machine keeps its row and loses its
/// button, saying so where the button would be. It is a fact about upstream - the
/// newest release may lag a platform - and a row that vanished would leave the
/// reader wondering whether the list was read at all.
fn xray_release_row(
    view: &WeakEntity<AppView>,
    release: &XrayRelease,
    download: Option<&XrayDownload>,
    p: Palette,
    t: &'static Text,
) -> impl IntoElement {
    let running = download.filter(|running| running.release == release.tag);
    let busy = download.is_some();

    let mut tag = div().flex().items_center().gap_2().min_w_0().child(
        div()
            .min_w_0()
            .truncate()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .child(release.tag.clone()),
    );
    if release.prerelease {
        tag = tag.child(components::status_badge(
            format!("xray-release-prerelease-{}", release.tag),
            components::Tone::Warning,
            t.xray_download_prerelease,
            t.xray_download_prerelease,
            p,
        ));
    }

    div()
        .id(format!("xray-release-{}", release.tag))
        .test_support()
        .flex()
        .items_center()
        .gap_4()
        .px_3()
        .py_3()
        .rounded(px(components::RADIUS_SURFACE))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(if running.is_some() {
            p.selected
        } else {
            p.panel
        }))
        .child(
            div()
                .id(format!("xray-release-version-{}", release.tag))
                .test_support()
                .w(px(COLUMN_RELEASE))
                .flex_shrink_0()
                .child(tag),
        )
        .child(
            div()
                .id(format!("xray-release-published-{}", release.tag))
                .test_support()
                .w(px(COLUMN_PUBLISHED))
                .flex_shrink_0()
                .truncate()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(release.published.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_end()
                .items_center()
                .gap_2()
                .when_some(running, |this, running| {
                    this.child(downloads::progress(
                        "xray-download-progress",
                        "xray-download-bar",
                        running,
                        t.xray_download_working,
                        p,
                        t,
                    ))
                })
                .when(running.is_none(), |this| {
                    this.child(match release.for_host() {
                        Some(asset) => {
                            xray_download_button(view, release, asset, busy).into_any_element()
                        }
                        None => div()
                            .id(format!("xray-release-none-{}", release.tag))
                            .test_support()
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child(t.xray_download_no_build)
                            .into_any_element(),
                    })
                }),
        )
}

/// The button for this machine's build.
///
/// Named by platform *and* architecture, which is the part the core dialog has no
/// equivalent of: the file name is the only place the architecture is written, and
/// a reader who has downloaded the wrong one before will look for it here.
fn xray_download_button(
    view: &WeakEntity<AppView>,
    release: &XrayRelease,
    asset: &XrayAsset,
    busy: bool,
) -> Button {
    let label = asset.label();
    let tooltip = format!(
        "{} · {}",
        asset.name,
        crate::releases::size_label(asset.size)
    );

    let button = Button::new(format!("xray-download-{}", release.tag))
        .label(label.clone())
        .tooltip(tooltip)
        .accessibility_label(label)
        .disabled(busy)
        .w(px(XRAY_BUTTON))
        .primary();

    let tag = release.tag.clone();
    let asset = asset.clone();
    let view = view.clone();
    button.on_click(move |_, _, cx| {
        let tag = tag.clone();
        let asset = asset.clone();
        if let Some(view) = view.upgrade() {
            view.update(cx, |view, cx| view.on_download_xray(tag, asset, cx));
        }
    })
}
