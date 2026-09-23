//! The Runtime Details panel: the selected profile, what is running, and the
//! diagnostics behind it.
//!
//! The panel is where a profile's live state is explained - the launch arguments
//! the core was given, the fingerprint verification, and the engine's own log - so
//! it is the one place in the window that reads from three sources at once.
//!
//! It is opened by choosing a profile and closed by the reader, rather than kept
//! on screen: it used to sit under the list at a fixed height whether or not
//! anybody was reading it, which cost the list a third of the window on every
//! frame. Where it appears depends on how much room the window has. Wide enough
//! for both, it takes a column of its own beside the list and the two stay in
//! view together; narrower, it covers the list and carries the way back, because
//! a panel narrow enough to leave a readable list beside it would not be worth
//! reading itself. The threshold is in [`super::DETAILS_DOCK_MIN`].

use super::components::{Tone, status_badge};
use super::logs::log_level_color;
use super::*;

/// The width the panel takes when it sits beside the list.
///
/// Wide enough for a label column and a path, which is the widest thing it
/// holds, and inside the 360-400 the design asks for. What it costs the list is
/// [`super::DETAILS_DOCK_MIN`] minus the sidebar, the page padding and this.
pub(super) const PANEL_WIDTH: f32 = 380.0;

/// The label column of a value row, so every value in every section starts on
/// the same line whether its label is "Seed" or "Dropped events".
const LABEL_WIDTH: f32 = 108.0;

/// The panel: the profile it describes, the three views of it, and the way out.
///
/// Seven parameters, and each one is a different thing the panel is built from:
/// the profile, its reading, the view in force, its log, where it goes, and the
/// context and palette every element needs. A struct would only move the list.
#[allow(clippy::too_many_arguments)]
pub(super) fn details_panel(
    selected: Option<&ProfileRow>,
    verification: Option<Verification>,
    tab: DetailsTab,
    log_tail: &[LogRow],
    docked: bool,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &'static Text,
) -> impl IntoElement {
    let body: AnyElement = match selected {
        None => div()
            .text_xs()
            .text_color(rgb(p.muted))
            .child(t.details_empty)
            .into_any_element(),
        Some(row) => match tab {
            DetailsTab::Details => details_body(row, verification.clone(), cx, p, t),
            DetailsTab::Args => args_body(row, cx, p, t),
            DetailsTab::Log => panel_log(log_tail, p, t).into_any_element(),
        },
    };

    div()
        .id("details-panel")
        .test_support()
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .h_full()
        .bg(rgb(p.panel))
        // Docked it is a surface beside the list, with its own edge; covering
        // the list it is that list's replacement, so only the shared edge is
        // drawn and the width is the whole column.
        .when(docked, |this| {
            this.w(px(PANEL_WIDTH))
                .flex_shrink_0()
                .rounded(px(components::RADIUS_SURFACE))
                .border_1()
                .border_color(rgb(p.border))
        })
        .when(!docked, |this| {
            this.w_full().border_l_1().border_color(rgb(p.border))
        })
        .child(details_header(
            selected,
            verification.as_ref(),
            docked,
            cx,
            p,
            t,
        ))
        .child(details_tabs(selected, tab, cx, t))
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

/// What the panel is about, and the control that puts it away.
///
/// The name rather than the words "Runtime Details": in a window that can show
/// one profile's details at a time, the useful title is which profile's.
fn details_header(
    selected: Option<&ProfileRow>,
    verification: Option<&Verification>,
    docked: bool,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &'static Text,
) -> Div {
    let title = selected
        .map(|row| row.profile.name.clone())
        .unwrap_or_else(|| t.runtime_details.to_string());
    // The state is worth repeating here: covering the list, the panel hides the
    // row the badge would otherwise be read from.
    let subtitle = selected.map(|row| match row.state_message() {
        Some(message) => t.profile_state_message(row.state_label(t), message),
        None => row.state_label(t).to_string(),
    });
    // Asking for a reading belongs with the profile's name rather than at the
    // bottom of a panel that scrolls: an action worth taking is worth being able
    // to see, and the panel's height is the window's, not the content's.
    let verify = selected.map(|row| verify_button(row, verification, docked, cx, t));
    let close = if docked {
        components::icon_button("details-close", icons::glyph::CLOSE_WINDOW, t.details_close)
            .on_click(cx.listener(|this, _, _, cx| this.on_close_details(cx)))
    } else {
        Button::new("details-close")
            .icon(icons::action(icons::glyph::BACK))
            .label(t.details_back)
            .outline()
            .on_click(cx.listener(|this, _, _, cx| this.on_close_details(cx)))
    };

    div()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_size(px(components::SECTION))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .children(subtitle.map(|subtitle| {
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(p.muted))
                        .child(subtitle)
                })),
        )
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_2()
                .children(verify)
                .child(close),
        )
}

/// The three views of one profile, as tabs.
///
/// One question at a time: identity, the launch line, and what the profile has
/// done. The chosen tab is filled rather than outlined, but with the quiet
/// fill - the page's one blue button is the one that creates things, and a tab
/// does not.
fn details_tabs(
    selected: Option<&ProfileRow>,
    tab: DetailsTab,
    cx: &mut Context<AppView>,
    t: &'static Text,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .children(DetailsTab::ALL.map(|candidate| {
            let active = candidate == tab;
            Button::new(candidate.id())
                .label(candidate.label(t))
                .when(active, |button| button.secondary())
                .when(!active, |button| button.ghost())
                .disabled(selected.is_none())
                .on_click(cx.listener(move |this, _, _, cx| this.on_set_details_tab(candidate, cx)))
        }))
}

/// The overview: what the profile is, what it is doing, and what was read back
/// from it.
fn details_body(
    row: &ProfileRow,
    verification: Option<Verification>,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &'static Text,
) -> AnyElement {
    // The two halves of "what is this": what was configured, and what that
    // configuration is doing right now. They are read at different times and
    // belong in different groups, not in one seventeen-row list.
    let mut configuration = details_section(t.details_group_configuration, p);
    for (label, value) in [
        (t.field_profile_id, row.profile.id.to_string()),
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
                .unwrap_or_else(|| t.direct.to_string()),
        ),
    ] {
        configuration = configuration.child(value_row(label, value_text(value), p));
    }
    // The path is the one value with something to do to it, so the control that
    // opens it sits beside it rather than in a row of buttons above.
    configuration = configuration.child(value_row(t.field_data_dir, data_dir_value(row, cx, t), p));

    let mut runtime = details_section(t.details_group_runtime, p);
    for (label, value) in [
        (t.field_state, row.state_label(t).to_string()),
        (t.field_browser_pid, optional(row.browser_pid())),
        (t.field_xray_pid, optional(row.xray_pid())),
        (t.field_cdp_port, optional(row.cdp_port())),
        (t.field_socks_port, optional(row.socks_port())),
        (t.field_started, elapsed(row, t)),
        (t.field_dropped_events, row.dropped_events().to_string()),
    ] {
        runtime = runtime.child(value_row(label, value_text(value), p));
    }
    if let Some(warning) = row.last_warning() {
        runtime = runtime.child(note_line(t.warning_line(warning), p.warning));
    }
    if let Some(error) = row.last_error() {
        runtime = runtime.child(note_line(t.error_line(error), p.danger_strong));
    }

    let verification_section = details_section(t.details_group_verification, p)
        .child(verification_block(verification, p, t));

    div()
        .id("details-grid")
        .test_support()
        .flex()
        .flex_col()
        .gap_4()
        .child(configuration)
        .child(runtime)
        .child(verification_section)
        .into_any_element()
}

/// A group heading inside the panel: a quiet word, then the rows under it.
fn details_section(title: &str, p: Palette) -> Div {
    div().flex().flex_col().gap_2().child(
        div()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(p.dim))
            .child(title.to_string()),
    )
}

/// One label and one value, on the same two lines as every other row.
fn value_row(label: &str, value: AnyElement, p: Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .child(
            div()
                .w(px(LABEL_WIDTH))
                .flex_shrink_0()
                .text_color(rgb(p.muted))
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .text_color(rgb(p.text_soft))
                .child(value),
        )
}

/// A value that is only text, cut to the panel rather than wrapped into it.
fn value_text(value: String) -> AnyElement {
    div().truncate().child(value).into_any_element()
}

/// The data directory, and the button that opens it.
///
/// The path is here rather than in a row of its own above because this is the
/// only place it is read, and the way to open it belongs beside it. The button
/// only appears where the desktop can actually be asked: an opener that is not
/// there says so in the banner rather than being hidden here.
fn data_dir_value(row: &ProfileRow, cx: &mut Context<AppView>, t: &'static Text) -> AnyElement {
    let id = row.profile.id;
    div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(row.profile.user_data_dir.display().to_string()),
        )
        .child(
            components::icon_button(
                format!("open-dir-{id}"),
                icons::glyph::OPEN_DIR,
                t.open_data_dir,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.on_open_data_dir(id, cx))),
        )
        .into_any_element()
}

/// A warning or an error, as the panel's own line of text.
fn note_line(text: String, colour: u32) -> Div {
    div().text_xs().text_color(rgb(colour)).child(text)
}

/// The one action the verification block carries: ask for a reading.
fn verify_button(
    row: &ProfileRow,
    verification: Option<&Verification>,
    docked: bool,
    cx: &mut Context<AppView>,
    t: &'static Text,
) -> Button {
    let id = row.profile.id;
    let button = if docked {
        components::icon_button(
            format!("verify-{id}"),
            icons::glyph::VERIFY,
            t.verify_fingerprint,
        )
    } else {
        Button::new(format!("verify-{id}"))
            .icon(icons::action(icons::glyph::VERIFY))
            .label(t.verify_fingerprint)
            .outline()
    };
    button
        .disabled(!can_verify(Some(row), verification))
        .on_click(cx.listener(move |this, _, _, cx| this.on_verify(id, cx)))
}

/// The launch line, and the one thing to do with it.
fn args_body(
    row: &ProfileRow,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &'static Text,
) -> AnyElement {
    let args = row.effective_args();
    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(if args.is_empty() {
                    t.no_launch_recorded.to_string()
                } else {
                    t.effective_args(args.len())
                }),
        )
        .child(
            Button::new("copy-args")
                .icon(icons::action(icons::glyph::COPY))
                .label(t.copy_args)
                .outline()
                .disabled(args.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.on_copy_args(cx))),
        );

    div()
        .id("args-body")
        .test_support()
        .flex()
        .flex_col()
        .gap_3()
        .child(header)
        .when(!args.is_empty(), |this| {
            this.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(px(components::RADIUS_CONTROL))
                    .bg(rgb(p.bg))
                    .text_xs()
                    .text_color(rgb(p.secondary))
                    .children(args.iter().map(|arg| div().child(arg.clone()))),
            )
        })
        .into_any_element()
}

/// The tail of one profile's activity log, for the panel's Log view.
pub(super) fn panel_log(rows: &[LogRow], p: Palette, t: &Text) -> impl IntoElement {
    if rows.is_empty() {
        return div()
            .id("panel-log-body")
            .test_support()
            .text_xs()
            .text_color(rgb(p.dim))
            .child(t.panel_log_empty);
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
        return div()
            .text_xs()
            .text_color(rgb(p.dim))
            .child(t.fingerprint_unverified);
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

    /// Puts the panel away. The profile stays chosen: the row it belongs to is
    /// still where the reader left it, and the keyboard goes back to it - the
    /// panel may have been opened with Enter, and the reader should not have to
    /// find the row again to carry on down the list.
    pub(super) fn on_close_details(&mut self, cx: &mut Context<Self>) {
        // Only when it was open: Escape is bound everywhere, and a key that
        // moved the keyboard out of wherever it was on a page with no panel
        // would be a key that did something invisible.
        self.focus_row = self.state.details_open();
        self.state.close_details();
        cx.notify();
    }
}
