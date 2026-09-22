//! The Browser Cores page: which cores are registered and what each binary is.
//!
//! Deliberately spare: a core has a name, a path, a version and the profiles that
//! launch with it, and the page is those four things rather than a dashboard.

use super::*;

pub(super) fn cores_header(cx: &mut Context<AppView>, t: &Text) -> Div {
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
                        .child(t.nav_cores),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(t.cores_intro),
                ),
        )
        .child(
            Button::new("new-core")
                .label(t.add_core)
                .primary()
                .on_click(cx.listener(|this, _, window, cx| this.on_edit_core(None, window, cx))),
        )
}

pub(super) fn cores_body(
    rows: &[CoreRow],
    cx: &mut Context<AppView>,
    t: &Text,
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
                div()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .bg(rgb(p.panel))
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child(t.cores_empty),
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
                                .label(t.redetect)
                                .outline()
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.on_redetect_core(id, cx)
                                    }),
                                ),
                        )
                        .child(
                            Button::new(format!("edit-core-{index}"))
                                .label(t.edit)
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_edit_core(Some(id), window, cx)
                                })),
                        )
                        .child(
                            Button::new(format!("delete-core-{index}"))
                                .label(t.delete)
                                .outline()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.on_delete_core(id, window, cx)
                                })),
                        ),
                )
        }))
}
