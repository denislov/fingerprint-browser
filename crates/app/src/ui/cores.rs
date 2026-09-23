//! The Browser Cores page: which cores are registered and what each binary is.
//!
//! Deliberately spare: a core has a name, a path, a version and the profiles that
//! launch with it, and the page is those four things rather than a dashboard.

use super::components::{EmptyState, PageHeader};
use super::*;

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
    cx: &mut Context<AppView>,
    t: &'static Text,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id("cores-scroll")
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
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_edit_core(None, window, cx)),
                        ),
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
                .justify_between()
                .gap_4()
                .px_4()
                .py_3()
                .rounded(px(components::RADIUS_SURFACE))
                .border_1()
                .border_color(rgb(p.border))
                .bg(rgb(p.panel))
                .hover(|this| this.bg(rgb(p.hover)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(row.core.name.clone()),
                                )
                                .when(!row.present, |this| {
                                    this.child(
                                        div()
                                            .text_xs()
                                            .text_color(rgb(p.danger))
                                            .child(t.core_executable_missing),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(t.core_label(&row.core.version, row.core.major)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(match row.generation_label() {
                                    Some(label) if label.contains("honoured") => p.success,
                                    Some(_) => p.warning,
                                    None => p.danger,
                                }))
                                .child(
                                    row.generation_label()
                                        .unwrap_or_else(|| t.core_no_version.to_string()),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(row.core.executable.to_string_lossy().to_string()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(if row.is_used() { p.success } else { p.muted }))
                                .child(row.usage_label(t)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new(format!("redetect-core-{index}"))
                                .icon(icons::action(icons::glyph::REDETECT))
                                .label(t.redetect)
                                .outline()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.on_redetect_core(id, cx)
                                    }),
                                ),
                        )
                        .child(core_menu(row, index, cx, t)),
                )
        }))
}

/// A core row's overflow menu: the two things done once per core.
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
