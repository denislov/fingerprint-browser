//! The Proxies page: the list, one proxy's card, and the reading a test left.
//!
//! A proxy's row carries the last thing that came back through it, so most of the
//! drawing here is about showing a reading honestly - which stage of the path was
//! measured, how long ago, and what it means when there is nothing to show.

use super::*;

pub(super) fn proxies_header(cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t.nav_proxies),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(t.proxies_intro),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new("import-proxy")
                        .label(t.import_from_link)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_import_proxy(window, cx)),
                        ),
                )
                .child(
                    Button::new("new-proxy")
                        .label(t.new_proxy)
                        .primary()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_edit_proxy(None, window, cx)),
                        ),
                ),
        )
}

pub(super) fn proxies_body(
    rows: &[ProxyRow],
    tests: &std::collections::HashMap<ProxyId, ProxyTest>,
    cx: &mut Context<AppView>,
    t: &Text,
) -> impl IntoElement {
    let p = palette(cx);
    div()
        .id("proxies-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_2()
        .overflow_y_scroll()
        .when(rows.is_empty(), |this| {
            this.child(
                div()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .bg(rgb(p.panel))
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child(t.proxies_empty),
            )
        })
        // The cards are built inline: a helper would have to return a type
        // borrowing the context, which the closure cannot hand back.
        .children(rows.iter().enumerate().map(|(index, row)| {
            let id = row.proxy.id;
            let test = tests.get(&id);
            div()
                .id(format!("proxy-{index}"))
                .test_support()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .px_4()
                .py_3()
                .rounded_md()
                .border_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child(row.proxy.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(row.endpoint()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(if row.is_used() { p.success } else { p.muted }))
                                .child(row.usage_label(t)),
                        )
                        .children(proxy_test_reading(test, id, p, t)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new(format!("test-proxy-{index}"))
                                .label(t.test)
                                .outline()
                                .disabled(test.is_some_and(ProxyTest::is_running))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.on_test_proxy(id, cx)),
                                ),
                        )
                        .child(
                            Button::new(format!("edit-proxy-{index}"))
                                .label(t.edit)
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_edit_proxy(Some(id), window, cx)
                                })),
                        )
                        .child(
                            Button::new(format!("delete-proxy-{index}"))
                                .label(t.delete)
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_delete_proxy(id, window, cx)
                                })),
                        ),
                )
        }))
}

/// The result of the last test of one proxy, or nothing when it has not run.
///
/// The row says which engine was probed, because "this proxy works" and "this
/// profile's traffic is going through it" are different claims and only the
/// second one is about a leak.
pub(super) fn proxy_test_reading(
    test: Option<&ProxyTest>,
    id: ProxyId,
    p: Palette,
    t: &Text,
) -> Option<AnyElement> {
    let test = test?;
    let colour = match test {
        ProxyTest::Running => p.dim,
        ProxyTest::Passed(reading) if reading.live => p.success,
        ProxyTest::Passed(_) => p.info,
        ProxyTest::Failed(_) => p.danger,
    };
    let summary = match test {
        ProxyTest::Running => test.label(t),
        ProxyTest::Passed(reading) => t.proxy_test_reading(
            &test.label(t),
            if reading.live {
                t.engine_running_profile
            } else {
                t.engine_temporary
            },
        ),
        // The evidence behind the class is in the activity log: a row is one
        // line, and an engine's own words are not.
        ProxyTest::Failed(_) => t.proxy_test_failed(&test.label(t)),
    };
    Some(
        div()
            .id(format!("proxy-test-{id}"))
            .test_support()
            .text_xs()
            .text_color(rgb(colour))
            .child(summary)
            .into_any_element(),
    )
}

impl AppView {
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
