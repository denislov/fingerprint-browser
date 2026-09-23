//! The Logs page: what happened this session, filtered and paged.
//!
//! A log line is a time, a level, a profile and a sentence, and the page's job is
//! to put those four in columns and let the reader narrow them down.

use super::components::PageHeader;
use super::*;

pub(super) fn logs_header(
    filter: LogFilter,
    status: &Result<String, String>,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Div {
    let status_line = match status {
        Ok(path) => div()
            .id("log-file-status")
            .test_support()
            .aria_label(t.log_written_to(path))
            .text_xs()
            .text_color(rgb(p.muted))
            .child(t.log_written_to(path)),
        // A log that cannot be written is a problem rather than a note: it is
        // the one line on this page that is drawn in the failure colour.
        Err(error) => div()
            .id("log-file-status")
            .test_support()
            .aria_label(t.log_not_written(error))
            .text_xs()
            .text_color(rgb(p.danger))
            .child(t.log_not_written(error)),
    };

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            PageHeader::new(t.nav_log)
                .summary("log-summary", t.log_intro, t.log_intro)
                .action(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("copy-log")
                                .icon(icons::action(icons::glyph::COPY))
                                .label(t.copy)
                                .outline()
                                .on_click(cx.listener(|this, _, _, cx| this.on_copy_log(cx))),
                        )
                        .child(
                            Button::new("clear-log")
                                .icon(icons::action(icons::glyph::CLEAR))
                                .label(t.clear)
                                .outline()
                                .on_click(cx.listener(|this, _, _, cx| this.on_clear_log(cx))),
                        ),
                )
                .render(p),
        )
        .child(status_line)
        // The filters are the page's own toolbar: they narrow what the list
        // below shows, so they sit between the heading and the list rather than
        // competing with the actions in the heading.
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .children(LogFilter::ALL.map(|candidate| {
                    let active = candidate == filter;
                    Button::new(candidate.id())
                        .label(candidate.label(t))
                        .when(active, |button| button.primary())
                        .when(!active, |button| button.outline())
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.on_set_log_filter(candidate, cx)
                            }),
                        )
                })),
        )
}

pub(super) fn logs_body(
    rows: &[LogRow],
    total: usize,
    filter: LogFilter,
    _cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> impl IntoElement {
    div()
        .id("logs-scroll")
        .test_support()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_1()
        .overflow_y_scroll()
        .when(rows.is_empty(), |this| {
            // An empty page says which kind of empty it is: nothing happened,
            // or the filter is hiding what did.
            let message = match total {
                0 => t.log_empty.to_string(),
                count => t.log_none_of_kind(filter.noun(t), count),
            };
            this.child(
                div()
                    .px_4()
                    .py_3()
                    .rounded_md()
                    .bg(rgb(p.panel))
                    .text_sm()
                    .text_color(rgb(p.muted))
                    .child(message),
            )
        })
        .children(rows.iter().enumerate().map(|(index, row)| {
            let label = format!("{} [{}] {}", row.who, row.level.label(t), row.message);
            div()
                .id(format!("log-{index}"))
                .test_support()
                .aria_label(label)
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(rgb(p.border))
                .child(
                    div()
                        .w(px(64.0))
                        .flex_shrink_0()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(log_level_color(row.level, p)))
                        .child(row.level.label(t)),
                )
                .child(
                    div()
                        .w(px(56.0))
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(rgb(p.dim))
                        .child(format_age(row.at, t)),
                )
                .child(
                    div()
                        .w(px(140.0))
                        .flex_shrink_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(row.who.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(rgb(p.text_soft))
                        .child(row.message.clone()),
                )
        }))
}

pub(super) fn log_level_color(level: LogLevel, p: Palette) -> u32 {
    match level {
        LogLevel::Info => p.muted,
        LogLevel::Warning => p.warning,
        LogLevel::Error => p.danger_strong,
    }
}

/// How long ago a line was written, freshly computed on each render.
pub(super) fn format_age(at: SystemTime, t: &Text) -> String {
    let seconds = SystemTime::now()
        .duration_since(at)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    match seconds {
        0..=1 => t.just_now.to_string(),
        seconds if seconds < 60 => format!("{seconds}s"),
        seconds if seconds < 3600 => format!("{}m", seconds / 60),
        seconds => format!("{}h", seconds / 3600),
    }
}

impl AppView {
    pub(super) fn on_copy_log(&mut self, cx: &mut Context<Self>) {
        let t = self.state.text();
        let text: String = self
            .state
            .log_rows()
            .iter()
            .map(|row| format!("{} [{}] {}", row.who, row.level.label(t), row.message))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn on_set_log_filter(&mut self, filter: LogFilter, cx: &mut Context<Self>) {
        self.state.set_log_filter(filter);
        cx.notify();
    }

    pub(super) fn on_clear_log(&mut self, cx: &mut Context<Self>) {
        self.state.clear_log();
        cx.notify();
    }
}
