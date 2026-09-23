//! The Logs page: what happened this session, filtered and paged.
//!
//! A log line is a time, a level, a profile and a sentence, and the page's job is
//! to put those four in columns and let the reader narrow them down. Two details
//! are the page's own: a line longer than a row is clamped with a control that
//! opens it rather than being allowed to stretch every row in the list, and the
//! message itself is selectable text, because a log line is something people
//! quote.

use super::components::PageHeader;
use super::*;
use gpui_kit::base::SelectableText;

/// How much of a long message is shown before it is clamped.
///
/// Measured in characters rather than in rows: a row that wraps to four lines
/// pushes everything below it down, and a list where one line is a paragraph is a
/// list nobody scans. Long enough that most lines - a path, a refusal, a
/// sentence from the engine - are shown whole.
const CLAMP: usize = 150;

pub(super) fn logs_header(
    filter: LogFilter,
    status: &Result<String, String>,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Div {
    let (status_line, status_error) = match status {
        Ok(path) => (t.log_written_to(path), false),
        // A log that cannot be written is a problem rather than a note: it is
        // the one line on this page that is drawn in the failure colour.
        Err(error) => (t.log_not_written(error), true),
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
                        // Copying is the ordinary thing to do with a log, so it
                        // is the outlined button; clearing is the one that takes
                        // something away, so it is the quiet one beside it.
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
                                .label(t.log_clear_view)
                                .ghost()
                                .on_click(cx.listener(|this, _, _, cx| this.on_clear_log(cx))),
                        ),
                )
                .render(p),
        )
        .child(
            div()
                .id("log-file-status")
                .test_support()
                .aria_label(status_line.clone())
                .text_xs()
                .text_color(rgb(if status_error { p.danger } else { p.muted }))
                .child(status_line),
        )
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
                }))
                // What clearing does and does not touch, in the one place a
                // reader about to press it is looking. The log file above keeps
                // every line, and a button called "Clear" must not read as
                // "delete the log".
                .child(
                    div()
                        .id("log-clear-scope")
                        .test_support()
                        .aria_label(t.log_clear_scope)
                        .ml_2()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(t.log_clear_scope),
                ),
        )
}

/// The Logs page's column headings, laid out with the rows' own widths.
pub(super) fn list_header(p: Palette, t: &Text) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .py_2()
        .border_1()
        .border_color(hsla(0.0, 0.0, 0.0, 0.0))
        .text_xs()
        .text_color(rgb(p.dim))
        .child(
            div()
                .id("column-time")
                .test_support()
                .w(px(COLUMN_TIME))
                .flex_shrink_0()
                .child(t.column_time),
        )
        .child(
            div()
                .id("column-level")
                .test_support()
                .w(px(COLUMN_LEVEL))
                .flex_shrink_0()
                .child(t.column_level),
        )
        .child(
            div()
                .id("column-log-who")
                .test_support()
                .w(px(COLUMN_WHO))
                .flex_shrink_0()
                .child(t.column_profile),
        )
        .child(
            div()
                .id("column-message")
                .test_support()
                .flex_1()
                .min_w_0()
                .child(t.column_message),
        )
}

pub(super) fn logs_body(
    rows: &[LogRow],
    total: usize,
    filter: LogFilter,
    expanded: &std::collections::HashSet<SystemTime>,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &'static Text,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap_2()
        .when(!rows.is_empty(), |this| this.child(list_header(p, t)))
        .child(
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
                .children(
                    rows.iter().enumerate().map(|(index, row)| {
                        log_row(row, index, expanded.contains(&row.at), cx, p, t)
                    }),
                ),
        )
}

/// One line: when, how bad, whose, and what.
///
/// Every byte of the message is reachable. A line that fits is drawn as it is; a
/// line that does not is clamped and carries the control that opens it, and the
/// text is selectable so the exact wording can be quoted without the row
/// deciding in advance which part of it matters.
fn log_row(
    row: &LogRow,
    index: usize,
    expanded: bool,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &'static Text,
) -> impl IntoElement + use<> {
    let length = row.message.chars().count();
    let clamped = length > CLAMP && !expanded;
    let shown = if clamped {
        let mut text: String = row.message.chars().take(CLAMP).collect();
        text.push('…');
        text
    } else {
        row.message.clone()
    };
    let at = row.at;
    let line = t.log_line(&row.who, row.level.label(t), &row.message);
    div()
        .id(format!("log-{index}"))
        .test_support()
        .aria_label(line)
        .flex()
        .items_start()
        .gap_3()
        .px_4()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(p.border))
        .child(
            div()
                .id(format!("log-time-{index}"))
                .test_support()
                .w(px(COLUMN_TIME))
                .flex_shrink_0()
                .truncate()
                .text_xs()
                .text_color(rgb(p.dim))
                // The list reads in relative time, which is what "did this just
                // happen" needs; the exact instant is one hover away, and is the
                // same stamp the file carries.
                .tooltip(components::tooltip(crate::log_file::timestamp(at)))
                .child(format_age(at, t)),
        )
        .child(
            div()
                .id(format!("log-level-{index}"))
                .test_support()
                .w(px(COLUMN_LEVEL))
                .flex_shrink_0()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(log_level_color(row.level, p)))
                .child(row.level.label(t)),
        )
        .child(
            div()
                .id(format!("log-who-{index}"))
                .test_support()
                .w(px(COLUMN_WHO))
                .flex_shrink_0()
                .truncate()
                .text_xs()
                .text_color(rgb(p.muted))
                .tooltip(components::tooltip(row.who.clone()))
                .child(row.who.clone()),
        )
        .child(
            div()
                .id(format!("log-body-{index}"))
                .test_support()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(p.text_soft))
                        .child(SelectableText::new(format!("log-message-{index}"), shown)),
                )
                .when(length > CLAMP, |this| {
                    this.child(
                        Button::new(format!("log-toggle-{index}"))
                            .label(if expanded {
                                t.log_collapse
                            } else {
                                t.log_expand
                            })
                            .ghost()
                            .compact()
                            .accessibility_label(if expanded {
                                t.log_collapse_aria()
                            } else {
                                t.log_expand_aria(length - CLAMP)
                            })
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.on_toggle_log_line(at, cx)),
                            ),
                    )
                }),
        )
}

/// The three columns the rows and the headings share.
const COLUMN_TIME: f32 = 72.0;
const COLUMN_LEVEL: f32 = 64.0;
const COLUMN_WHO: f32 = 140.0;

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
            .map(|row| t.log_line(&row.who, row.level.label(t), &row.message))
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

    /// Opens one line in full, or closes it again.
    ///
    /// Keyed by when the line was written rather than by its place in the list:
    /// the list is newest-first, so a new line moves every index below it and an
    /// index would open somebody else's line.
    pub(super) fn on_toggle_log_line(&mut self, at: SystemTime, cx: &mut Context<Self>) {
        self.state.toggle_log_expanded(at);
        cx.notify();
    }

    pub(super) fn on_clear_log(&mut self, cx: &mut Context<Self>) {
        self.state.clear_log();
        cx.notify();
    }
}
