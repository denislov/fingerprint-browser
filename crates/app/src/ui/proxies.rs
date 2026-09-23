//! The Proxies page: the list, one proxy's card, and the reading a test left.
//!
//! A proxy's row carries the last thing that came back through it, so most of the
//! drawing here is about showing a reading honestly - which stage of the path was
//! measured, how long ago, and what it means when there is nothing to show.

use super::components::{EmptyState, PageHeader};
use super::*;
use gpui_kit::component::Sizable as _;

/// The proxy row's fixed columns.
///
/// Sized for the longest thing each holds in either language: an IPv6 endpoint
/// with a port, "not assigned" or a name and a count, the longest reading a test
/// leaves behind, and the Test button beside its menu. The name takes what is
/// left, which is what keeps a table readable in a narrow window instead of
/// pushing the actions off the row.
const COLUMN_ENDPOINT: f32 = 220.0;
const COLUMN_USAGE: f32 = 150.0;
const COLUMN_TEST: f32 = 260.0;
const COLUMN_ACTIONS: f32 = 140.0;

pub(super) fn proxies_header(cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    PageHeader::new(t.nav_proxies)
        .summary("proxies-summary", t.proxies_intro, t.proxies_intro)
        .action(
            div()
                .flex()
                .items_center()
                .gap_2()
                // Importing is the quieter of the two ways a proxy arrives, so
                // it is the outline button beside the one filled action.
                .child(
                    Button::new("import-proxy")
                        .icon(icons::action(icons::glyph::IMPORT_LINK))
                        .label(t.import_from_link)
                        .outline()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_import_proxy(window, cx)),
                        ),
                )
                .child(
                    Button::new("new-proxy")
                        .icon(icons::action(icons::glyph::NEW))
                        .label(t.new_proxy)
                        .primary()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_edit_proxy(None, window, cx)),
                        ),
                ),
        )
        .render(p)
}

pub(super) fn proxies_body(
    rows: &[ProxyRow],
    tests: &std::collections::HashMap<ProxyId, ProxyTest>,
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
        .when(!rows.is_empty(), |this| this.child(list_header(p, t)))
        .child(
            div()
                .id("proxies-scroll")
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
                            "proxies-empty",
                            icons::glyph::EMPTY_PROXIES,
                            t.empty_proxies_title,
                            t.proxies_empty,
                        )
                        .action(
                            Button::new("empty-new-proxy")
                                .icon(icons::action(icons::glyph::NEW))
                                .label(t.new_proxy)
                                .primary()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.on_edit_proxy(None, window, cx)
                                })),
                        )
                        .render(p),
                    )
                })
                // The rows are built inline: a helper would have to return a type
                // borrowing the context, which the closure cannot hand back.
                .children(rows.iter().enumerate().map(|(index, row)| {
                    let id = row.proxy.id;
                    let test = tests.get(&id);
                    let running = test.is_some_and(ProxyTest::is_running);
                    div()
                        .id(format!("proxy-{index}"))
                        .test_support()
                        .flex()
                        .items_center()
                        .gap_4()
                        .px_4()
                        .min_h(px(components::ROW_HEIGHT))
                        .rounded(px(components::RADIUS_SURFACE))
                        .border_1()
                        .border_color(rgb(p.border))
                        .bg(rgb(p.panel))
                        .hover(|this| this.bg(rgb(p.hover)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(row.proxy.name.clone()),
                        )
                        .child(endpoint_cell(row, p, t))
                        .child(usage_cell(row, p, t))
                        .child(proxy_test_cell(test, id, cx, p, t))
                        .child(
                            div()
                                .w(px(COLUMN_ACTIONS))
                                .flex_shrink_0()
                                .flex()
                                .justify_end()
                                .items_center()
                                .gap_2()
                                // The one reading a user takes repeatedly keeps its
                                // button; editing and deleting a proxy are things done
                                // once, and belong in the menu beside it. While a test
                                // is in flight the button carries the loading mark
                                // rather than being pressed a second time.
                                .child(
                                    Button::new(format!("test-proxy-{index}"))
                                        .icon(icons::action(icons::glyph::TEST))
                                        .label(t.test)
                                        .outline()
                                        .loading(running)
                                        .disabled(running)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.on_test_proxy(id, cx)
                                        })),
                                )
                                .child(proxy_menu(row, index, cx, t)),
                        )
                })),
        )
}

/// The Proxies page's column headings.
///
/// The same widths and the same gaps as a row, so a heading sits over the column
/// it names. The transparent border is what makes the two line up: a row draws a
/// one-pixel border and the heading does not, and the pixel it would add is a
/// pixel of the name column.
pub(super) fn list_header(p: Palette, t: &Text) -> Div {
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
                .id("column-proxy")
                .test_support()
                .flex_1()
                .min_w_0()
                .child(t.column_profile),
        )
        .child(
            div()
                .id("column-endpoint")
                .test_support()
                .w(px(COLUMN_ENDPOINT))
                .flex_shrink_0()
                .child(t.column_endpoint),
        )
        .child(
            div()
                .id("column-usage")
                .test_support()
                .w(px(COLUMN_USAGE))
                .flex_shrink_0()
                .child(t.column_usage),
        )
        .child(
            div()
                .id("column-test")
                .test_support()
                .w(px(COLUMN_TEST))
                .flex_shrink_0()
                .child(t.column_last_test),
        )
        .child(
            div()
                .id("column-actions")
                .test_support()
                .w(px(COLUMN_ACTIONS))
                .flex_shrink_0()
                .text_right()
                .child(t.column_actions),
        )
}

/// What the proxy dials. The endpoint carries no credentials, so it is shown
/// whole in the tooltip; the column is where a long IPv6 address is truncated.
fn endpoint_cell(row: &ProxyRow, p: Palette, t: &Text) -> impl IntoElement {
    let endpoint = row.endpoint();
    div()
        .id(format!("proxy-endpoint-{}", row.proxy.id))
        .test_support()
        .w(px(COLUMN_ENDPOINT))
        .flex_shrink_0()
        .truncate()
        .text_xs()
        .text_color(rgb(p.muted))
        .tooltip(components::tooltip(t.proxy_endpoint_help(&endpoint)))
        .child(endpoint)
}

/// Who is using it, named rather than counted when there is room for one name.
fn usage_cell(row: &ProxyRow, p: Palette, t: &Text) -> impl IntoElement {
    let used = row.is_used();
    let label = row.usage_label(t);
    div()
        .id(format!("proxy-usage-{}", row.proxy.id))
        .test_support()
        .w(px(COLUMN_USAGE))
        .flex_shrink_0()
        .truncate()
        .text_xs()
        .text_color(rgb(if used { p.success } else { p.muted }))
        .tooltip(components::tooltip(row.used_by.join(", ")))
        .child(label)
}

/// The result of the last test of one proxy, or the state of never having run one.
///
/// "Not tested" is its own state and not a blank: a proxy nobody has asked about
/// is not a proxy that answered. A pass shows the address the traffic left from,
/// because that is the whole point of asking; the engine's own account, the time
/// it took and which engine was probed are one hover away, and the full fault is
/// a link to the log rather than a wrapped paragraph in a row.
pub(super) fn proxy_test_cell(
    test: Option<&ProxyTest>,
    id: ProxyId,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &'static Text,
) -> impl IntoElement {
    // The whole reading, which is more than a column can hold: the words of the
    // fault for a failure, and the address, the time and the engine for a pass.
    // It is what the cell is announced by, and what its hover shows.
    let aria = match test {
        None => t.proxy_untested.to_string(),
        // What the row says, then the whole reading behind it: the class is what
        // the eye gets, and the reason has to reach a reader who cannot see the
        // row at all.
        Some(test) => match test.detail(t) {
            Some(detail) => format!("{} - {detail}", test.label(t)),
            None => test.label(t),
        },
    };
    let hint = match test.and_then(ProxyTest::fault) {
        Some(fault) => t.proxy_fault_help(&fault.to_string()),
        None => aria.clone(),
    };
    let cell = div()
        .id(format!("proxy-test-{id}"))
        .test_support()
        .aria_label(aria)
        .tooltip(components::tooltip(hint))
        .w(px(COLUMN_TEST))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap_1()
        .min_w_0();
    let Some(test) = test else {
        return cell.child(
            div()
                .truncate()
                .text_xs()
                .text_color(rgb(p.dim))
                .child(t.proxy_untested),
        );
    };
    cell.child(match test {
        ProxyTest::Running => div()
            .flex()
            .items_center()
            .gap_2()
            .text_xs()
            .text_color(rgb(p.info))
            .child(
                gpui_kit::component::spinner::Spinner::new()
                    .with_size(px(14.0))
                    .color(rgb(p.info).into()),
            )
            .child(test.label(t)),
        ProxyTest::Passed(reading) => {
            let colour = if reading.live { p.success } else { p.info };
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(colour))
                        .child(test.label(t)),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(p.dim))
                        .child(t.proxy_test_elapsed(reading.elapsed.as_millis())),
                )
        }
        ProxyTest::Failed(_) => div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .child(
                div()
                    .truncate()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(p.danger))
                    .child(test.label(t)),
            )
            .child(
                Button::new(format!("proxy-diagnostics-{id}"))
                    .label(t.proxy_diagnostics)
                    .ghost()
                    .compact()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.on_show_proxy_diagnostics(id, cx)),
                    ),
            ),
    })
}

/// A proxy row's overflow menu: the two things a row is not worth a button for.
fn proxy_menu(
    row: &ProxyRow,
    index: usize,
    cx: &mut Context<AppView>,
    t: &'static Text,
) -> impl IntoElement {
    let id = row.proxy.id;
    let name = row.proxy.name.clone();
    let view = cx.entity().downgrade();
    components::icon_button(
        format!("more-proxy-{index}"),
        icons::glyph::MORE,
        t.row_more(&name),
    )
    .dropdown_menu(move |menu, _window, _cx| {
        menu.item(components::menu_item(
            &view,
            t.edit,
            icons::glyph::EDIT,
            false,
            move |view, window, cx| view.on_edit_proxy(Some(id), window, cx),
        ))
        .separator()
        .item(components::menu_item(
            &view,
            t.delete,
            icons::glyph::DELETE,
            false,
            move |view, window, cx| view.on_delete_proxy(id, window, cx),
        ))
    })
}

impl AppView {
    /// Takes a failed test's reader to the line the engine wrote.
    ///
    /// A row is one column wide and a fault is a paragraph, so the row says which
    /// class failed and this is the way to the rest: the Log page, filtered to
    /// the errors, which is where `finish_proxy_test` writes the engine's own
    /// words. The filter is set rather than assumed - a reader who left the page
    /// on "warnings" would otherwise arrive at a list that hides the line.
    pub(super) fn on_show_proxy_diagnostics(&mut self, _id: ProxyId, cx: &mut Context<Self>) {
        self.state.set_log_filter(LogFilter::Errors);
        self.state.set_page(Page::Log);
        cx.notify();
    }

    /// Sends one request through a proxy and reports what left.
    ///
    /// The work is a socket held open for as long as the far end takes, so it
    /// runs on a worker and the window stays responsive. `live` travels with
    /// the answer because the row has to say whether the engine probed was
    /// already carrying a profile's traffic or was started for the test.
    pub(super) fn on_test_proxy(&mut self, id: ProxyId, cx: &mut Context<Self>) {
        let job = match self.state.begin_proxy_test(id) {
            Ok(job) => job,
            Err(error) => {
                self.state.push_notice(error.to_string(), true);
                cx.notify();
                return;
            }
        };
        let tester = Arc::clone(&self.tester);
        let sender = self.proxy_test_tx.clone();
        std::thread::spawn(move || {
            let outcome = tester.test(&job);
            let _ = sender.send((job, outcome));
        });
        cx.notify();
    }

    /// Deleting a proxy that is still assigned is refused, and says by whom.
    pub(super) fn on_delete_proxy(
        &mut self,
        id: ProxyId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.state.text();
        let (name, endpoint, used_by) = match self.state.proxy(id) {
            Some(proxy) => {
                let used_by = self
                    .state
                    .proxy_rows()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|row| row.proxy.id == id)
                    .map(|row| row.used_by)
                    .unwrap_or_default();
                let endpoint = proxy.endpoint();
                (proxy.name, endpoint, used_by)
            }
            None => (id.to_string(), String::new(), Vec::new()),
        };
        let description = if used_by.is_empty() {
            t.delete_proxy_confirm(&name, &endpoint)
        } else {
            t.proxy_in_use(&name, &used_by.join(", "))
        };
        let view = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let description = description.clone();
            alert
                .title(t.delete_proxy_title)
                .description(description)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(t.delete)
                        .cancel_text(t.keep)
                        .show_cancel(true)
                        .on_ok(move |_, _, cx| {
                            if let Some(view) = view.upgrade() {
                                view.update(cx, |view, cx| {
                                    let _ = view.state.delete_proxy(id);
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

    /// Opens the paste dialog for a share link.
    ///
    /// The dialog parses and the view stores; on a refusal the dialog stays open
    /// with the parser's own sentence, because the link is the thing to fix.
    pub(super) fn on_import_proxy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = self.state.text();
        let proxy_import = cx.new(|cx| ProxyImport::new(t, window, cx));
        self.proxy_import = Some(proxy_import.clone());
        let view = cx.entity().downgrade();
        let accepted = proxy_import.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let proxy_import = proxy_import.clone();
            let view = view.clone();
            let accepted = accepted.clone();
            dialog
                .title(proxy_import.read(cx).title())
                .w(px(640.0))
                .child(proxy_import.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label(t.cancel).outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label(t.import))),
                )
                .on_ok(move |_, _, cx| {
                    let imported: Result<(), String> = match proxy_import.read(cx).read_link(cx) {
                        Ok((name, outbound)) => view
                            .update(cx, |view, _| {
                                match view.state.create_proxy(&name, outbound) {
                                    Ok(_) => Ok(()),
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
                    match imported {
                        Ok(()) => {
                            accepted.update(cx, |import, cx| {
                                import.set_error(None);
                                cx.notify();
                            });
                            true
                        }
                        Err(message) => {
                            accepted.update(cx, |import, cx| {
                                import.set_error(Some(message));
                                cx.notify();
                            });
                            false
                        }
                    }
                })
        });
        cx.notify();
    }

    /// Opens the form for a new proxy, or for one that is already stored.
    pub(super) fn on_edit_proxy(
        &mut self,
        id: Option<ProxyId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = self.state.text();
        let editing = id.and_then(|id| self.state.proxy(id));
        if id.is_some() && editing.is_none() {
            self.state.push_notice(t.proxy_gone.to_string(), true);
            cx.notify();
            return;
        }
        let proxy_editor = cx.new(|cx| match &editing {
            Some(proxy) => ProxyEditor::for_proxy(proxy, t, window, cx),
            None => ProxyEditor::new(t, window, cx),
        });
        self.proxy_editor = Some(proxy_editor.clone());
        let view = cx.entity().downgrade();
        let accepted = proxy_editor.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let proxy_editor = proxy_editor.clone();
            let view = view.clone();
            let accepted = accepted.clone();
            let title = proxy_editor.read(cx).title();
            dialog
                .title(title)
                .w(px(640.0))
                .child(proxy_editor.clone())
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().trigger(|button| button.label(t.cancel).outline()),
                        )
                        .child(DialogAction::new().child(Button::new("ok").label(t.save))),
                )
                .on_ok(move |_, _, cx| {
                    // A refusal from the service is shown in the form as well as
                    // the banner: the form is where the mistake is.
                    let saved: Result<(), String> = match proxy_editor.read(cx).build_proxy(cx) {
                        Ok(proxy) => {
                            let existing = proxy_editor.read(cx).is_edit();
                            view.update(cx, |view, _| {
                                let result = if existing {
                                    view.state.update_proxy(proxy)
                                } else {
                                    view.state
                                        .create_proxy(&proxy.name, proxy.outbound)
                                        .map(|_| ())
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
                            .unwrap_or_else(|_| Err(t.window_gone.to_string()))
                        }
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
