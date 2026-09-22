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
