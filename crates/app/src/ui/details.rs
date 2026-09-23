//! The Runtime Details panel: the selected profile, what is running, and the
//! diagnostics behind it.
//!
//! The panel is where a profile's live state is explained - the launch arguments
//! the core was given, the fingerprint verification, and the engine's own log - so
//! it is the one place in the window that reads from three sources at once.

use super::components::{Tone, status_badge};
use super::logs::log_level_color;
use super::*;

pub(super) fn details_panel(
    selected: Option<&ProfileRow>,
    verification: Option<Verification>,
    tab: DetailsTab,
    log_tail: &[LogRow],
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Div {
    // The three views are different element types once an id makes them
    // stateful, so the panel erases them before choosing one.
    let body: AnyElement = match selected {
        None => div()
            .text_xs()
            .text_color(rgb(p.muted))
            .child(t.details_empty)
            .into_any_element(),
        Some(row) => {
            let mut grid = div()
                .id("details-grid")
                .test_support()
                .flex()
                .flex_wrap()
                .gap_x_6()
                .gap_y_2();
            let verification = verification.clone();
            for (label, value) in [
                (t.field_profile_id, row.profile.id.to_string()),
                (
                    t.field_state,
                    match row.state_message() {
                        Some(message) => t.profile_state_message(row.state_label(t), message),
                        None => row.state_label(t).to_string(),
                    },
                ),
                (t.field_seed, row.profile.fingerprint.seed.to_string()),
                ("Brand", row.profile.fingerprint.brand.to_string()),
                ("Platform", row.profile.fingerprint.platform.to_string()),
                (t.language_title, row.profile.fingerprint.language.clone()),
                ("Timezone", row.profile.fingerprint.timezone.clone()),
                (t.field_core, row.core_name.clone()),
                (
                    t.field_proxy,
                    row.proxy_name
                        .clone()
                        .unwrap_or_else(|| "direct".to_string()),
                ),
                (
                    t.field_data_dir,
                    row.profile.user_data_dir.display().to_string(),
                ),
                (t.field_browser_pid, optional(row.browser_pid())),
                (t.field_xray_pid, optional(row.xray_pid())),
                (t.field_cdp_port, optional(row.cdp_port())),
                (t.field_socks_port, optional(row.socks_port())),
                (t.field_started, elapsed(row, t)),
                (t.field_dropped_events, row.dropped_events().to_string()),
            ] {
                grid = grid.child(key_value(label, value, p));
            }

            let details = div()
                .flex()
                .flex_col()
                .gap_3()
                .child(grid)
                .child(verification_block(verification, p, t))
                .children(row.last_warning().map(|warning| {
                    div()
                        .text_xs()
                        .text_color(rgb(p.warning))
                        .child(t.warning_line(warning))
                }))
                .children(row.last_error().map(|error| {
                    div()
                        .text_xs()
                        .text_color(rgb(p.danger_strong))
                        .child(t.error_line(error))
                }));

            // Only one of the three questions is answered at a time, so the
            // panel scrolls a view rather than the whole history of the session.
            match tab {
                DetailsTab::Details => details.into_any_element(),
                DetailsTab::Args => div()
                    .id("args-body")
                    .test_support()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(effective_args(row, p, t))
                    .into_any_element(),
                DetailsTab::Log => panel_log(log_tail, p, t).into_any_element(),
            }
        }
    };

    div()
        .flex()
        .flex_col()
        .gap_3()
        .max_h(px(320.0))
        .p_4()
        .rounded_md()
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t.runtime_details),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("copy-args")
                                .label(t.copy_args)
                                .outline()
                                .disabled(
                                    selected
                                        .map(|row| row.effective_args().is_empty())
                                        .unwrap_or(true),
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.on_copy_args(cx))),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("open-dir-{}", row.profile.id))
                                    .unwrap_or_else(|| "open-dir".to_string()),
                            )
                            .label(t.open_data_dir)
                            .outline()
                            .disabled(selected.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(row) = this.state.selected() {
                                    let id = row.profile.id;
                                    this.on_open_data_dir(id, cx);
                                }
                            })),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("verify-{}", row.profile.id))
                                    .unwrap_or_else(|| "verify".to_string()),
                            )
                            .label(t.verify_fingerprint)
                            .outline()
                            .disabled(!can_verify(selected, verification.as_ref()))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(row) = this.state.selected() {
                                    let id = row.profile.id;
                                    this.on_verify(id, cx);
                                }
                            })),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("edit-{}", row.profile.id))
                                    .unwrap_or_else(|| "edit".to_string()),
                            )
                            .label(t.edit)
                            .outline()
                            .disabled(selected.is_none())
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    if let Some(row) = this.state.selected() {
                                        let id = row.profile.id;
                                        this.on_edit(id, window, cx);
                                    }
                                },
                            )),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("duplicate-{}", row.profile.id))
                                    .unwrap_or_else(|| "duplicate".to_string()),
                            )
                            .label(t.duplicate)
                            .outline()
                            .disabled(selected.is_none())
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(row) = this.state.selected() {
                                    let id = row.profile.id;
                                    this.on_duplicate(id, cx);
                                }
                            })),
                        )
                        .child(
                            Button::new(
                                selected
                                    .map(|row| format!("delete-{}", row.profile.id))
                                    .unwrap_or_else(|| "delete".to_string()),
                            )
                            .label(t.delete)
                            .outline()
                            .disabled(selected.is_none())
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    if let Some(row) = this.state.selected() {
                                        let id = row.profile.id;
                                        this.on_delete(id, window, cx);
                                    }
                                },
                            )),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .children(DetailsTab::ALL.map(|candidate| {
                    let active = candidate == tab;
                    Button::new(candidate.id())
                        .label(candidate.label(t))
                        .when(active, |button| button.primary())
                        .when(!active, |button| button.ghost())
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.on_set_details_tab(candidate, cx)
                            }),
                        )
                })),
        )
        .child(
            div()
                .id("details-scroll")
                .test_support()
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
}

/// The tail of one profile's activity log, for the panel's Log view.
pub(super) fn panel_log(rows: &[LogRow], p: Palette, t: &Text) -> impl IntoElement {
    if rows.is_empty() {
        return div()
            .id("panel-log-body")
            .test_support()
            .text_xs()
            .text_color(rgb(p.dim))
            .child(
                "Nothing logged for this profile yet. The Log page has the whole session, \
                 including window-level lines.",
            );
    }

    div()
        .id("panel-log-body")
        .test_support()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(t.panel_log_title(rows.len())),
        )
        .children(rows.iter().enumerate().map(|(index, row)| {
            div()
                .id(("panel-log", index))
                .test_support()
                .flex()
                .items_center()
                .gap_3()
                .text_xs()
                .child(
                    div()
                        .w(px(56.0))
                        .flex_shrink_0()
                        .text_color(rgb(log_level_color(row.level, p)))
                        .child(row.level.label(t)),
                )
                .child(
                    div()
                        .w(px(48.0))
                        .flex_shrink_0()
                        .text_color(rgb(p.dim))
                        .child(format_age(row.at, t)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(rgb(p.text_soft))
                        .child(row.message.clone()),
                )
        }))
}

/// Whether the selected profile can be verified right now.
///
/// A fingerprint can only be read out of a live browser that is not already
/// being read, and the browser publishes its debug port only once running.
pub(super) fn can_verify(
    selected: Option<&ProfileRow>,
    verification: Option<&Verification>,
) -> bool {
    let Some(row) = selected else {
        return false;
    };
    row.cdp_port().is_some()
        && row.state() == RuntimeState::Running
        && !verification.is_some_and(Verification::is_running)
}

/// The verification result for one profile, or a hint that it has not run.
pub(super) fn verification_block(verification: Option<Verification>, p: Palette, t: &Text) -> Div {
    let Some(verification) = verification else {
        return div().text_xs().text_color(rgb(p.dim)).child(
            "Fingerprint not verified in this session. Verification reads the \
                 running browser in its own tab and compares it with the profile.",
        );
    };
    if verification.is_running() {
        return div()
            .text_xs()
            .text_color(rgb(p.muted))
            .child(t.reading_fingerprint);
    }
    if let Some(reason) = verification.failure() {
        return div()
            .text_xs()
            .text_color(rgb(p.danger_strong))
            .child(t.fingerprint_unreadable(reason));
    }
    let found = verification.disagreements();
    let headline = if found.is_empty() {
        div()
            .text_xs()
            .text_color(rgb(p.success_strong))
            .child(t.fingerprint_confirmed)
    } else {
        div()
            .text_xs()
            .text_color(rgb(p.warning))
            .child(t.claims_not_reproduced(found.len()))
    };

    let mut block = div().flex().flex_col().gap_1().child(headline);
    // The path is the half of the answer no other panel carries, and it is the
    // reason to trust a proxy at all: an address that matches the one the proxy
    // was tested at is worth seeing without having to open the log.
    if let Some(report) = verification.report()
        && let Some(label) = report.exit_label(t)
    {
        let colour = if report.exit_ip.is_some() {
            p.info
        } else {
            p.dim
        };
        block = block.child(
            div()
                .id("exit-address")
                .test_support()
                .text_xs()
                .text_color(rgb(colour))
                .child(label),
        );
    }
    if found.is_empty() {
        return block;
    }
    // The panel scrolls, so a long list of findings stays reachable instead of
    // being clipped to the first few.
    block.children(found.iter().enumerate().map(|(index, discrepancy)| {
        div()
            .id(("disagreement", index))
            .test_support()
            .text_xs()
            .text_color(rgb(p.warning))
            .child(t.claim_line(
                discrepancy.claim,
                &discrepancy.expected,
                &discrepancy.observed,
            ))
    }))
}

/// A compact marker for the row: the user should not have to select a profile
/// to know whether its fingerprint was confirmed.
///
/// "Running" is information rather than a warning: a check in progress is not
/// something to fix. What the reader must not read into it is the row's own
/// state - a green row is a running browser, and this badge is the only thing
/// that speaks for the fingerprint.
pub(super) fn verification_badge(
    verification: Option<&Verification>,
    p: Palette,
    t: &Text,
) -> Option<impl IntoElement> {
    let verification = verification?;
    let tone = match verification {
        Verification::Confirmed(_) => Tone::Success,
        Verification::Running => Tone::Info,
        Verification::Disagreements(_) => Tone::Warning,
        Verification::Unreadable(_) => Tone::Danger,
    };
    Some(status_badge(
        format!("verification-{}", verification.label(t)),
        tone,
        verification.label(t),
        verification.label(t),
        p,
    ))
}

pub(super) fn effective_args(row: &ProfileRow, p: Palette, t: &Text) -> Div {
    let args = row.effective_args();
    if args.is_empty() {
        return div()
            .text_xs()
            .text_color(rgb(p.dim))
            .child(t.no_launch_recorded);
    }

    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(t.effective_args(args.len())),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(rgb(p.bg))
                .text_xs()
                .text_color(rgb(p.secondary))
                .children(args.iter().map(|arg| div().child(arg.clone()))),
        )
}

pub(super) fn key_value(label: &str, value: String, p: Palette) -> Div {
    div()
        .flex()
        .gap_2()
        .text_xs()
        .child(
            div()
                .w(px(96.0))
                .flex_shrink_0()
                .text_color(rgb(p.muted))
                .child(label.to_string()),
        )
        .child(div().text_color(rgb(p.text_soft)).child(value))
}

pub(super) fn optional<T: ToString>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "—".to_string())
}

pub(super) fn elapsed(row: &ProfileRow, t: &Text) -> String {
    let started = row
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.started_at);
    match started.and_then(|started| SystemTime::now().duration_since(started).ok()) {
        Some(elapsed) => t.elapsed_seconds(elapsed.as_secs()),
        None => "—".to_string(),
    }
}

impl AppView {
    pub(super) fn on_set_details_tab(&mut self, tab: DetailsTab, cx: &mut Context<Self>) {
        self.state.set_details_tab(tab);
        cx.notify();
    }
}
