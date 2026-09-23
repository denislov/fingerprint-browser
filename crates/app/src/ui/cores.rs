//! The Browser Cores page: which cores are registered and what each binary is.
//!
//! Deliberately spare: a core has a name, a path, a version and the profiles that
//! launch with it, and the page is those four things rather than a dashboard.

use super::components::{EmptyState, PageHeader};
use super::*;

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

pub(super) fn cores_header(cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    PageHeader::new(t.nav_cores)
        .summary("cores-summary", t.cores_intro, t.cores_intro)
        .action(
            Button::new("new-core")
                .icon(icons::action(icons::glyph::NEW))
                .label(t.add_core)
                .primary()
                .on_click(cx.listener(|this, _, window, cx| this.on_edit_core(None, window, cx))),
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
