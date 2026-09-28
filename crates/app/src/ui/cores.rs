//! The Browser Cores page: which cores are registered and what each binary is.
//!
//! Deliberately spare: a core has a name, a path, a version and the profiles that
//! launch with it, and the page is those four things rather than a dashboard.
//!
//! The one thing on the page that is not about a registered core is the Download
//! Core dialog: the builds published for fingerprint-chromium, one row per
//! version and one button per system it was built for. It lives here rather than
//! on a page of its own because that is where the cores it produces end up.

use super::components::{EmptyState, PageHeader};
use super::*;
use crate::core_releases::{AssetKind, CoreAsset, CoreRelease, DownloadJob, Platform, REPOSITORY};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::progress::Progress;

/// The core row's fixed columns.
///
/// The name holds the path under it and takes what is left; the three beside it
/// are sized for the longest value in either language - a Chromium version, a
/// compatibility reading with its exclusion note, and the Re-detect button
/// beside its menu. Narrow windows put version and usage below the name instead
/// of squeezing the name away or pushing the controls beyond the window.
const COLUMN_VERSION: f32 = 150.0;
const COLUMN_COMPATIBILITY: f32 = 260.0;
const COLUMN_USAGE: f32 = 140.0;
const COLUMN_ACTIONS: f32 = 160.0;

/// The Download Core dialog's width: the two reading columns and the three
/// platform buttons beside them, with the page's own padding.
const DOWNLOAD_DIALOG_WIDTH: f32 = 820.0;
/// The version column, sized for `148.0.7778.215` and the pre-release pill under
/// it.
const COLUMN_RELEASE: f32 = 180.0;
/// The publication date, sized for `2026-06-21`.
const COLUMN_PUBLISHED: f32 = 110.0;
/// One platform's button, sized for the longest of the three names.
const DOWNLOAD_BUTTON: f32 = 92.0;
/// How tall the release list may grow before it scrolls.
///
/// The dialog is read a version at a time and the reader is choosing one, so the
/// list is bounded and the buttons below it stay where they are - a footer that
/// moves with the number of releases is a footer that has to be found again.
const DOWNLOAD_LIST_HEIGHT: f32 = 380.0;

pub(super) fn cores_header(cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    PageHeader::new(t.nav_cores)
        .summary("cores-summary", t.cores_intro, t.cores_intro)
        .action(
            // Two actions in one slot: the page's own primary, and the one that
            // puts a core on disk to begin with. Downloading is not the primary
            // - most visits here are about a core that is already registered -
            // so it is the outline one beside it.
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("download-core")
                        .icon(icons::action(icons::glyph::DOWNLOAD_CORE))
                        .label(t.download_core)
                        .outline()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_open_core_downloads(window, cx)
                        })),
                )
                .child(
                    Button::new("new-core")
                        .icon(icons::action(icons::glyph::NEW))
                        .label(t.add_core)
                        .primary()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_edit_core(None, window, cx)),
                        ),
                ),
        )
        .render(p)
}

pub(super) fn cores_body(
    rows: &[CoreRow],
    compact: bool,
    cx: &mut Context<AppView>,
    t: &'static Text,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_2()
        .when(!rows.is_empty(), |this| {
            this.child(list_header(compact, p, t))
        })
        .child(
            div()
                .id("cores-scroll")
                .test_support()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .gap_2()
                .overflow_y_scroll()
                .when(rows.is_empty(), |this| {
                    this.child(
                        EmptyState::new(
                            "cores-empty",
                            icons::glyph::EMPTY_CORES,
                            t.empty_cores_title,
                            t.cores_empty,
                        )
                        .action(
                            Button::new("empty-new-core")
                                .icon(icons::action(icons::glyph::NEW))
                                .label(t.add_core)
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_edit_core(None, window, cx)
                                })),
                        )
                        .render(p),
                    )
                })
                // Built inline: a helper returning a borrowed type cannot escape the
                // closure that owns the context.
                .children(rows.iter().enumerate().map(|(index, row)| {
                    let id = row.core.id;
                    div()
                        .id(format!("core-{index}"))
                        .test_support()
                        .flex()
                        .items_center()
                        .gap_4()
                        .px_4()
                        .min_h(px(if compact {
                            112.0
                        } else {
                            components::ROW_HEIGHT
                        }))
                        .rounded(px(components::RADIUS_SURFACE))
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .hover(|this| this.bg(rgb(p.hover)))
                        .child(
                            name_cell(row, index, cx, p, t)
                                .id(format!("core-name-{id}"))
                                .test_support()
                                .when(compact, |this| {
                                    this.child(version_cell(row, p, t))
                                        .child(usage_cell(row, p, t))
                                }),
                        )
                        .when(!compact, |this| this.child(version_cell(row, p, t)))
                        .child(compatibility_cell(row, p, t))
                        .when(!compact, |this| this.child(usage_cell(row, p, t)))
                        .child(
                            div()
                                .w(px(COLUMN_ACTIONS))
                                .flex_shrink_0()
                                .flex()
                                .justify_end()
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new(format!("redetect-core-{index}"))
                                        .icon(icons::action(icons::glyph::REDETECT))
                                        .label(t.redetect)
                                        .outline()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.on_redetect_core(id, cx)
                                        })),
                                )
                                .child(core_menu(row, index, cx, t)),
                        )
                })),
        )
}

/// The Cores page's column headings, laid out with the rows' own widths.
pub(super) fn list_header(compact: bool, p: Palette, t: &Text) -> Div {
    div()
        .flex()
        .items_center()
        .gap_4()
        .px_4()
        .py_2()
        .border_1()
        .border_color(hsla(0.0, 0.0, 0.0, 0.0))
        .text_xs()
        .text_color(rgb(p.dim))
        .child(
            div()
                .id("column-core")
                .test_support()
                .flex_1()
                .min_w_0()
                .child(t.core_name),
        )
        .when(!compact, |this| {
            this.child(
                div()
                    .id("column-version")
                    .test_support()
                    .w(px(COLUMN_VERSION))
                    .flex_shrink_0()
                    .child(t.column_version),
            )
        })
        .child(
            div()
                .id("column-compatibility")
                .test_support()
                .w(px(COLUMN_COMPATIBILITY))
                .flex_shrink_0()
                .child(t.column_compatibility),
        )
        .when(!compact, |this| {
            this.child(
                div()
                    .id("column-core-usage")
                    .test_support()
                    .w(px(COLUMN_USAGE))
                    .flex_shrink_0()
                    .child(t.column_usage),
            )
        })
        .child(
            div()
                .id("column-core-actions")
                .test_support()
                .w(px(COLUMN_ACTIONS))
                .flex_shrink_0()
                .text_right()
                .child(t.column_actions),
        )
}

/// The core's name, whether its binary is still there, and the path it runs.
///
/// The path is secondary text rather than a column of its own - it is what the
/// name resolves to, not a fact read at a glance - and it carries its own copy
/// button, because a path that is truncated in the middle is a path somebody has
/// to retype.
fn name_cell(
    row: &CoreRow,
    index: usize,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &'static Text,
) -> Div {
    let id = row.core.id;
    let path = row.core.executable.to_string_lossy().to_string();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .min_w_0()
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap_2()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(row.core.name.clone()),
                )
                // A binary that is gone is a reason a launch is refused, so it is
                // its own label beside the name rather than a shade of grey.
                .when(!row.present, |this| {
                    this.child(components::status_badge(
                        format!("core-missing-{id}"),
                        components::Tone::Danger,
                        t.core_executable_missing,
                        t.core_executable_missing,
                        p,
                    ))
                }),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .min_w_0()
                .child(
                    div()
                        .id(format!("core-path-{index}"))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .tooltip(components::tooltip(t.core_path_help(&path)))
                        .child(path),
                )
                .child(
                    components::icon_button(
                        format!("copy-core-path-{index}"),
                        icons::glyph::COPY,
                        t.core_path_copy,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.on_copy_core_path(id, cx))),
                ),
        )
}

/// The version the binary answered, or an explicit label when it answered none.
fn version_cell(row: &CoreRow, p: Palette, t: &Text) -> impl IntoElement {
    let known = !row.core.version.trim().is_empty();
    let label = if known {
        row.core.version.clone()
    } else {
        t.core_version_unknown.to_string()
    };
    div()
        .id(format!("core-version-{}", row.core.id))
        .test_support()
        .aria_label(label.clone())
        .w(px(COLUMN_VERSION))
        .flex_shrink_0()
        .truncate()
        .text_xs()
        .text_color(rgb(if known { p.text_soft } else { p.warning }))
        .child(label)
}

/// Which switch generation the core belongs to, and what that means for a
/// profile on it.
///
/// The line is drawn from the capability table, not from the English words the
/// state model happens to use: a page that matched on "honoured" would silently
/// start colouring every core in a different language's spelling of it.
fn compatibility_cell(row: &CoreRow, p: Palette, t: &Text) -> impl IntoElement {
    let (label, colour, honoured) = match row.capability_parts() {
        Some((generation, honoured)) => (
            t.core_compatibility(&generation, honoured),
            if honoured { p.success } else { p.warning },
            Some(honoured),
        ),
        None => (t.core_no_version.to_string(), p.danger, None),
    };
    let help = honoured
        .map(|honoured| t.core_exclusions_help(row.core.major, honoured))
        .unwrap_or_else(|| t.core_no_version.to_string());
    div()
        .id(format!("core-compatibility-{}", row.core.id))
        .test_support()
        .aria_label(label.clone())
        .w(px(COLUMN_COMPATIBILITY))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(rgb(colour))
                .child(label),
        )
        .tooltip(components::tooltip(help))
}

/// Who is launching profiles with it, named rather than counted when there is
/// room for one name.
fn usage_cell(row: &CoreRow, p: Palette, t: &Text) -> impl IntoElement {
    div()
        .id(format!("core-usage-{}", row.core.id))
        .test_support()
        .w(px(COLUMN_USAGE))
        .flex_shrink_0()
        .truncate()
        .text_xs()
        .text_color(rgb(if row.is_used() { p.success } else { p.muted }))
        .tooltip(components::tooltip(row.used_by.join(", ")))
        .child(row.usage_label(t))
}

/// A core row's overflow menu: the things done once per core.
///
/// Opening the location is here rather than on the row because it is a repair
/// action - the binary was replaced, moved or installed somewhere odd - and it
/// keeps the row's one button for the reading a user actually takes repeatedly.
fn core_menu(
    row: &CoreRow,
    index: usize,
    cx: &mut Context<AppView>,
    t: &'static Text,
) -> impl IntoElement {
    let id = row.core.id;
    let name = row.core.name.clone();
    let view = cx.entity().downgrade();
    components::icon_button(
        format!("more-core-{index}"),
        icons::glyph::MORE,
        t.row_more(&name),
    )
    .dropdown_menu(move |menu, _window, _cx| {
        menu.item(components::menu_item(
            &view,
            t.edit,
            icons::glyph::EDIT,
            false,
            move |view, window, cx| view.on_edit_core(Some(id), window, cx),
        ))
        .item(components::menu_item(
            &view,
            t.core_open_location,
            icons::glyph::OPEN_DIR,
            false,
            move |view, _window, cx| view.on_open_core_dir(id, cx),
        ))
        .separator()
        .item(components::menu_item(
            &view,
            t.delete,
            icons::glyph::DELETE,
            false,
            move |view, window, cx| view.on_delete_core(id, window, cx),
        ))
    })
}

impl AppView {
    /// Puts a core's path on the clipboard.
    ///
    /// The path is a column that gets truncated and a value a user has to paste
    /// into a file manager or a problem report, so copying it is the row's own
    /// action rather than something to select by hand out of a text run that may
    /// be showing a middle ellipsis.
    pub(super) fn on_copy_core_path(&mut self, id: CoreId, cx: &mut Context<Self>) {
        let t = self.state.text();
        let Some(core) = self.state.core(id) else {
            self.state.push_notice(t.core_gone.to_string(), true);
            cx.notify();
            return;
        };
        let path = core.executable.to_string_lossy().to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
        self.state.push_notice(t.core_path_copied(&path), false);
        cx.notify();
    }

    /// Opens the directory a core's binary lives in.
    ///
    /// The directory rather than the file: there is no portable "select this
    /// file" verb, and the place a binary came from is what a reader wants when a
    /// core is missing or the wrong version. A binary that is already gone is
    /// still worth opening the folder for, so this is not gated on the binary
    /// being present - the opener reports what it found.
    pub(super) fn on_open_core_dir(&mut self, id: CoreId, cx: &mut Context<Self>) {
        let t = self.state.text();
        let Some(core) = self.state.core(id) else {
            self.state.push_notice(t.core_gone.to_string(), true);
            cx.notify();
            return;
        };
        let directory = match core.executable.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };
        let opener = Arc::clone(&self.opener);
        let sender = self.open_tx.clone();
        std::thread::spawn(move || {
            let result = opener.open(&directory, t);
            let _ = sender.send((directory, result));
        });
        cx.notify();
    }

    /// Removing a core is refused while a profile still launches with it.
    pub(super) fn on_delete_core(
        &mut self,
        id: CoreId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.state.text();
        let (name, version, used_by) = match self.state.core(id) {
            Some(core) => {
                let used_by = self
                    .state
                    .core_rows()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|row| row.core.id == id)
                    .map(|row| row.used_by)
                    .unwrap_or_default();
                (core.name, core.version, used_by)
            }
            None => (id.to_string(), String::new(), Vec::new()),
        };
        let description = if used_by.is_empty() {
            t.delete_core_confirm(&name, &version)
        } else {
            t.core_in_use(&name, &used_by.join(", "))
        };
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let description = description.clone();
            alert
                .title(t.delete_core_title)
                .description(description)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t.delete)
                        .cancel_text(t.keep)
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    let _ = view.state.delete_core(id);
                                    cx.notify();
                                });
                            }
                            true
                        })
                        .on_cancel(|_, _, _| true),
                )
        });
        cx.notify();
    }

    /// Re-reads a core's version - for a binary that was replaced in place.
    /// Reads a core's version again.
    ///
    /// The probe starts the binary and waits for it to answer, which is a program
    /// this window did not write: it runs on a worker, so a core that hangs or a
    /// binary on a slow disk does not stop the window redrawing.
    pub(super) fn on_redetect_core(&mut self, id: CoreId, cx: &mut Context<Self>) {
        let job = match self.state.begin_redetect(id) {
            Ok(job) => job,
            Err(message) => {
                self.state.push_notice(message, true);
                cx.notify();
                return;
            }
        };
        let sender = self.maintenance_tx.clone();
        std::thread::spawn(move || {
            let result = maintenance::run_redetect(job);
            let _ = sender.send(maintenance::Outcome::Redetected { id, result });
        });
        cx.notify();
    }

    /// Opens the form for a new core, or for one that is already registered.
    /// Opens the form for a new core, or for one that is already registered.
    ///
    /// Adding goes through the service, which probes the binary: the version a
    /// core claims decides what the engine may be asked to spoof, so it is read
    /// rather than typed.
    pub(super) fn on_edit_core(
        &mut self,
        id: Option<CoreId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.state.text();
        let editing = id.and_then(|id| self.state.core(id));
        if id.is_some() && editing.is_none() {
            self.state.push_notice(t.core_gone.to_string(), true);
            cx.notify();
            return;
        }
        let core_editor = cx.new(|cx| match &editing {
            Some(core) => CoreEditor::for_core(core, t, window, cx),
            None => CoreEditor::new(t, window, cx),
        });
        self.core_editor = Some(core_editor.clone());
        let view = cx.entity().downgrade();
        let accepted = core_editor.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let core_editor = core_editor.clone();
            let view = view.clone();
            let accepted = accepted.clone();
            let title = core_editor.read(cx).title();
            dialog
                .title(title)
                .w(px(680.0))
                .child(core_editor.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label(t.cancel).outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label(t.save))),
                )
                .on_ok(move |_, _, cx| {
                    let editing = core_editor.read(cx).is_edit();
                    let name = core_editor.read(cx).name(cx);
                    let path = core_editor.read(cx).executable(cx);
                    let built = if editing {
                        core_editor.read(cx).build_core(cx).map(Some)
                    } else if path.as_os_str().is_empty() {
                        Err(t.executable_cannot_be_empty.to_string())
                    } else {
                        Ok(None)
                    };

                    let saved: Result<(), String> = match built {
                        Ok(core) => view
                            .update(cx, |view, _| {
                                let result = match core {
                                    Some(core) => view.state.update_core(core),
                                    None => view.state.add_core(name, path).map(|_| ()),
                                };
                                match result {
                                    Ok(()) => Ok(()),
                                    Err(error) => {
                                        let message = error.to_string();
                                        view.state.push_notice(message.clone(), true);
                                        Err(message)
                                    }
                                }
                            })
                            .unwrap_or_else(|_| Err(t.window_gone.to_string())),
                        Err(error) => Err(error),
                    };
                    match saved {
                        Ok(()) => {
                            accepted.update(cx, |editor, cx| {
                                editor.set_error(None);
                                cx.notify();
                            });
                            true
                        }
                        Err(message) => {
                            accepted.update(cx, |editor, cx| {
                                editor.set_error(Some(message));
                                cx.notify();
                            });
                            false
                        }
                    }
                })
        });
    }
}

/// The Download Core dialog's body, as a view of its own.
///
/// A second entity rather than reads of [`AppView`] inside the dialog's
/// closure: the dialog is drawn while the window it belongs to is being read,
/// and reading that window from there is the re-entrancy the entity map
/// refuses. So the window pushes each answer into this one, and the dialog
/// reads this one instead - the same shape [`CoreEditor`] has, for the same
/// reason.
///
/// Its handle on the window is weak because the window owns it: a strong handle
/// would be a cycle neither of them would ever drop.
pub struct CoreDownloads {
    text: &'static Text,
    view: WeakEntity<AppView>,
    releases: ReleaseStatus,
    download: Option<ReleaseDownload>,
}

impl CoreDownloads {
    fn new(
        view: WeakEntity<AppView>,
        text: &'static Text,
        releases: ReleaseStatus,
        download: Option<ReleaseDownload>,
    ) -> Self {
        Self {
            text,
            view,
            releases,
            download,
        }
    }

    /// Takes the copy of the window's state this draws.
    fn show(&mut self, releases: ReleaseStatus, download: Option<ReleaseDownload>) {
        self.releases = releases;
        self.download = download;
    }

    /// Whether a list is on its way, for the footer's Refresh button.
    fn is_loading(&self) -> bool {
        matches!(self.releases, ReleaseStatus::Loading)
    }
}

impl Render for CoreDownloads {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = palette(cx);
        let t = self.text;
        let mut body = div()
            .id("core-download-body")
            .test_support()
            .flex()
            .flex_col()
            .gap_2()
            .min_h_0()
            .child(download_caption(p, t));

        match &self.releases {
            ReleaseStatus::Unasked | ReleaseStatus::Loading => {
                body = body.child(download_note(
                    "core-download-loading",
                    t.core_download_loading,
                    p.muted,
                ));
            }
            ReleaseStatus::Failed(message) => {
                body = body.child(download_failure(message, p));
            }
            ReleaseStatus::Listed(releases) if releases.is_empty() => {
                body = body.child(download_note(
                    "core-download-empty",
                    t.core_download_empty,
                    p.muted,
                ));
            }
            ReleaseStatus::Listed(releases) => {
                let download = self.download.as_ref();
                body = body.child(download_columns(p, t)).child(
                    div()
                        .id("core-download-scroll")
                        .test_support()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .max_h(px(DOWNLOAD_LIST_HEIGHT))
                        .overflow_y_scroll()
                        .children(
                            releases
                                .iter()
                                .map(|release| release_row(&self.view, release, download, p, t)),
                        ),
                );
            }
        }

        body
    }
}

/// The dialog's chrome around the body [`CoreDownloads`] draws.
///
/// Handed the two things it needs rather than reading them: the window's weak
/// handle for the buttons, and the dialog view for the list and the busy flag.
fn core_download_dialog(
    downloads: Entity<CoreDownloads>,
    view: WeakEntity<AppView>,
    text: &'static Text,
    dialog: Dialog,
    cx: &mut App,
) -> Dialog {
    let loading = downloads.read(cx).is_loading();
    dialog
        .title(text.core_download_title)
        .w(px(DOWNLOAD_DIALOG_WIDTH))
        .child(downloads)
        .footer(
            DialogFooter::new()
                .child(open_cores_dir_button(&view, text))
                .child(refresh_releases_button(&view, text, loading))
                .child(
                    DialogClose::new()
                        .trigger(|button| button.label(text.core_download_close).outline()),
                ),
        )
        .on_close(|_, _, _| ())
}

/// The Download Core dialog, and the two things it asks workers to do.
impl AppView {
    /// Opens the dialog, asking for the release list the first time.
    ///
    /// Asking here rather than when the Cores page is drawn: the list is a
    /// network request and most visits to the page are not about downloading
    /// anything, so the request is made by the reader who wants it.
    pub(super) fn on_open_core_downloads(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.state.releases(), ReleaseStatus::Unasked) {
            self.on_fetch_releases(cx);
        }
        let view = cx.entity().downgrade();
        let text = self.state.text();
        let releases = self.state.releases().clone();
        let download = self.state.download().cloned();
        let downloads = cx.new(|_| CoreDownloads::new(view.clone(), text, releases, download));
        self.downloads = Some(downloads.clone());

        window.open_dialog(cx, move |dialog, _window, cx| {
            core_download_dialog(downloads.clone(), view.clone(), text, dialog, cx)
        });
        cx.notify();
    }

    /// Asks for the release list again.
    ///
    /// Public to the view rather than private to the click, because the dialog's
    /// Refresh button and the first open are the same request.
    pub(super) fn on_fetch_releases(&mut self, cx: &mut Context<Self>) {
        self.state.begin_release_fetch();
        let catalog = Arc::clone(&self.catalog);
        let sender = self.releases_tx.clone();
        std::thread::spawn(move || {
            let result = catalog.list();
            let _ = sender.send(ReleaseEvent::Listed(result));
        });
        self.sync_downloads(cx);
        cx.notify();
    }

    /// Downloads one asset of one release, unpacks it, and registers what it
    /// contains.
    ///
    /// The registration happens when the worker's answer arrives, not here: it
    /// means the storage this window holds, and it is where the version is
    /// probed - see [`crate::state::AppState::finish_core_download`].
    pub(super) fn on_download_release(
        &mut self,
        release: String,
        asset: CoreAsset,
        cx: &mut Context<Self>,
    ) {
        let t = self.state.text();
        if let Err(message) = self.state.begin_core_download(&release, &asset) {
            self.state.push_notice(message, true);
            cx.notify();
            return;
        }
        let job = DownloadJob {
            text: t,
            release: release.clone(),
            asset: asset.clone(),
            directory: self.state.release_dir(&release),
        };
        // Said in a toast as well as shown on the row: the dialog can be closed
        // while a hundred megabytes are arriving, and the log line is then the
        // only record of what this window is doing.
        self.state.push_notice(
            t.core_release_downloading(&release, asset.platform.label()),
            false,
        );

        let downloader = Arc::clone(&self.downloader);
        let sender = self.releases_tx.clone();
        let key = crate::core_releases::asset_key(&job.release, &job.asset.name);
        std::thread::spawn(move || {
            let mut report = |received: u64, total: u64| {
                let _ = sender.send(ReleaseEvent::Progress {
                    key: key.clone(),
                    received,
                    total,
                });
            };
            let result = downloader.download(&job, &mut report);
            let _ = sender.send(ReleaseEvent::Downloaded {
                release: job.release.clone(),
                result,
            });
        });
        self.sync_downloads(cx);
        cx.notify();
    }

    /// Opens the directory downloaded cores are kept in.
    ///
    /// Created first when it is not there: nothing downloaded yet is not an
    /// error, and a reader who asked to see where cores go should be shown the
    /// place rather than told it does not exist.
    pub(super) fn on_open_cores_dir(&mut self, cx: &mut Context<Self>) {
        let directory = self.state.cores_dir();
        let t = self.state.text();
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.state.push_notice(
                t.core_release_disk_failed(&directory.display().to_string(), &error.to_string()),
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

    /// Copies the release state into the dialog's own view, when one is open.
    ///
    /// Called after every answer that changes it. The dialog cannot read the
    /// state through this window - see [`CoreDownloads`] - so this is the one
    /// place the two are kept in step, and the notify is what has the dialog
    /// draw the copy that just arrived: without it a progress report would sit
    /// in the entity until something else redrew the window.
    pub(super) fn sync_downloads(&mut self, cx: &mut Context<Self>) {
        let Some(downloads) = self.downloads.clone() else {
            return;
        };
        let releases = self.state.releases().clone();
        let download = self.state.download().cloned();
        downloads.update(cx, |downloads, cx| {
            downloads.show(releases, download);
            cx.notify();
        });
    }
}

/// The line above the list: where the builds come from, and what the buttons
/// beside them will do.
fn download_caption(p: Palette, t: &'static Text) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .id("core-download-source")
                .test_support()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(t.core_download_source(REPOSITORY)),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.dim))
                .child(t.core_download_intro),
        )
}

/// A centred line standing in for the list: loading, or nothing to list.
fn download_note(id: &'static str, text: &str, colour: u32) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .py_6()
        .flex()
        .justify_center()
        .text_xs()
        .text_color(rgb(colour))
        .child(text.to_string())
}

/// Why the list could not be read.
fn download_failure(message: &str, p: Palette) -> impl IntoElement {
    div()
        .id("core-download-failed")
        .test_support()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(p.danger_bg))
        .text_xs()
        .text_color(rgb(p.danger))
        .child(message.to_string())
}

/// The list's headings, laid out with the rows' own widths.
fn download_columns(p: Palette, t: &'static Text) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_4()
        .px_3()
        // Invisible, and there for the same reason the registered-cores list's
        // header has one: a row is drawn with a 1px border and the heading is
        // not, so without this the columns would sit one pixel to the left of
        // the values they name. `the_download_rows_line_up_under_the_column_headings`
        // is what would notice.
        .border_1()
        .border_color(hsla(0.0, 0.0, 0.0, 0.0))
        .text_xs()
        .text_color(rgb(p.dim))
        .child(
            div()
                .id("column-release")
                .test_support()
                .w(px(COLUMN_RELEASE))
                .flex_shrink_0()
                .child(t.core_download_column_version),
        )
        .child(
            div()
                .id("column-published")
                .test_support()
                .w(px(COLUMN_PUBLISHED))
                .flex_shrink_0()
                .child(t.core_download_column_published),
        )
        .child(
            div()
                .id("column-builds")
                .test_support()
                .flex_1()
                .min_w_0()
                .text_right()
                .child(t.core_download_column_builds),
        )
}

/// One published version: what it is, when it came out, and what can be
/// downloaded for it.
///
/// The row whose asset is in flight shows that download's progress in place of
/// its buttons. That is the one row a reader is looking at, and a progress line
/// anywhere else would be a second thing to find.
fn release_row(
    view: &WeakEntity<AppView>,
    release: &CoreRelease,
    download: Option<&ReleaseDownload>,
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
            format!("release-prerelease-{}", release.tag),
            components::Tone::Warning,
            t.core_download_prerelease,
            t.core_download_prerelease,
            p,
        ));
    }

    div()
        .id(format!("release-{}", release.tag))
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
                .id(format!("release-version-{}", release.tag))
                .test_support()
                .w(px(COLUMN_RELEASE))
                .flex_shrink_0()
                .child(tag),
        )
        .child(
            div()
                .id(format!("release-published-{}", release.tag))
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
                    this.child(download_progress(running, p, t))
                })
                .when(running.is_none(), |this| {
                    this.children(Platform::ALL.into_iter().filter_map(|platform| {
                        release
                            .for_platform(platform)
                            .map(|asset| download_button(view, release, asset, platform, busy, t))
                    }))
                }),
        )
}

/// The button for one platform's build.
///
/// The system this window is running on gets the filled button and says so in
/// its accessible name; the others are outlines beside it, because a reader who
/// is downloading for another machine still has to be able to reach them. The
/// tooltip carries what the label has no room for: which kind of file it is and
/// how large it is.
fn download_button(
    view: &WeakEntity<AppView>,
    release: &CoreRelease,
    asset: &CoreAsset,
    platform: Platform,
    busy: bool,
    t: &'static Text,
) -> Button {
    let host = platform == Platform::host();
    let detail = format!(
        "{} · {} · {}",
        platform.label(),
        asset_kind_label(asset.kind, t),
        crate::core_releases::size_label(asset.size)
    );
    let tooltip = if host {
        format!("{detail} ({})", t.core_download_this_system)
    } else {
        detail
    };
    let aria = if host {
        format!("{} {}", platform.label(), t.core_download_this_system)
    } else {
        platform.label().to_string()
    };

    let button = Button::new(format!("download-{}-{}", release.tag, platform.id()))
        .label(platform.label())
        .tooltip(tooltip)
        .accessibility_label(aria)
        .disabled(busy)
        .w(px(DOWNLOAD_BUTTON))
        .when(host, |this| this.primary())
        .when(!host, |this| this.outline());

    let tag = release.tag.clone();
    let asset = asset.clone();
    let view = view.clone();
    button.on_click(move |_, _, cx| {
        let tag = tag.clone();
        let asset = asset.clone();
        if let Some(view) = view.upgrade() {
            view.update(cx, |view, cx| view.on_download_release(tag, asset, cx));
        }
    })
}

/// How far the download on this row has got.
///
/// The bar is drawn from the fraction only when the server said how large the
/// file is; without a total the bytes that have arrived are still shown, because
/// "42 MB" with nothing to compare it to is at least true, and a bar that
/// guessed would not be.
fn download_progress(running: &ReleaseDownload, p: Palette, t: &'static Text) -> impl IntoElement {
    let received = crate::core_releases::size_label(running.received);
    let total = crate::core_releases::size_label(running.total);
    let line = if running.total > 0 {
        t.core_release_progress(&received, &total)
    } else {
        received.clone()
    };

    div()
        .id("core-download-progress")
        .test_support()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(div().min_w_0().truncate().child(running.asset.clone()))
                .child(line),
        )
        .child(
            Progress::new("core-download-bar")
                .value(running.fraction().unwrap_or(0.0) * 100.0)
                .accessibility_label(t.core_download_working),
        )
}

/// What kind of file an asset is, in the reader's language.
///
/// `AppImage` is the format's own name and is deliberately not translated, for
/// the same reason `SOCKS5` and `Windows` are not: it is what the file is called
/// in every language. What a reader choosing between the kinds needs translated
/// is the two that are descriptions rather than names.
fn asset_kind_label(kind: AssetKind, t: &'static Text) -> &'static str {
    match kind {
        AssetKind::Portable => t.core_download_portable,
        AssetKind::Installer => t.core_download_installer,
        AssetKind::AppImage => "AppImage",
        AssetKind::DiskImage => t.core_download_disk_image,
    }
}

/// The footer's button that opens the cores directory.
fn open_cores_dir_button(view: &WeakEntity<AppView>, t: &'static Text) -> Button {
    let view = view.clone();
    Button::new("core-download-open-folder")
        .icon(icons::action(icons::glyph::OPEN_DIR))
        .label(t.core_download_open_folder)
        .outline()
        .on_click(move |_, _, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| view.on_open_cores_dir(cx));
            }
        })
}

/// The footer's button that asks for the list again.
fn refresh_releases_button(view: &WeakEntity<AppView>, t: &'static Text, loading: bool) -> Button {
    let view = view.clone();
    Button::new("core-download-refresh")
        .icon(icons::action(icons::glyph::REFRESH))
        .label(t.core_download_refresh)
        .outline()
        // A list already on its way is not asked for again: two requests for one
        // dialog is one more than the API's rate limit was asked to carry.
        .disabled(loading)
        .on_click(move |_, _, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| view.on_fetch_releases(cx));
            }
        })
}
