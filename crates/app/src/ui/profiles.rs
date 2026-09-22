//! The Profiles page: the list, its rows, and the columns they carry.
//!
//! Separated from the rest of the window's drawing for the same reason the state's
//! profile lifecycle is the largest single topic: a row has to say what the profile
//! is, which core runs it, where its traffic goes, how long it has been up and
//! whether anything is wrong with it, and all of that reads better beside itself.
//!
//! The helper functions are `pub(super)` rather than private because the window's
//! `render` is what calls them, and `render` lives in [`super`].

use super::details::verification_badge;
use super::*;

pub(super) fn profiles_header(header: &ProfilesHeader, cx: &mut Context<AppView>, t: &Text) -> Div {
    let p = palette(cx);
    // While a filter is on, the count is the useful sentence: it is how the user
    // finds out that the list is not the whole list. Without one, the header
    // goes back to explaining what a profile is. The accessible name spells out
    // both either way, because a status line is worth hearing in full.
    let filtering = header.total > 0 && !header.filter.trim().is_empty();
    let subtitle = if filtering {
        t.profiles_showing(header.visible, header.total)
    } else {
        t.profiles_intro.to_string()
    };
    let announcement = if filtering {
        t.profiles_showing_filtered(header.visible, header.total, header.filter.trim())
    } else {
        t.profiles_total(header.total)
    };

    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .min_w_0()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(t.nav_profiles),
                        )
                        .child(
                            div()
                                .id("profile-count")
                                .test_support()
                                .aria_label(announcement)
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(subtitle),
                        ),
                )
                .child(
                    Button::new("new-profile")
                        .label(t.new_profile)
                        .primary()
                        // A profile needs a core to launch, and the empty state
                        // below says where to get one. The button is disabled
                        // rather than opening a dialog the service would refuse.
                        .disabled(!header.has_core)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.on_new_profile(window, cx)),
                        ),
                ),
        )
        .child(
            div().w(px(PROFILE_FILTER_WIDTH)).child(
                Input::new(&header.search)
                    .id("profile-filter")
                    .aria_label(t.profiles_filter_aria)
                    .cleanable(true),
            ),
        )
}

pub(super) fn empty_hint(
    total: usize,
    visible: usize,
    has_core: bool,
    filter: &str,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Option<impl IntoElement> {
    // Two kinds of empty look the same in a list and mean different things:
    // there are no profiles, or a filter is hiding the ones there are. Only the
    // second can be undone from here, so only it offers to.
    let filtering = total > 0 && visible == 0;
    if total > 0 && !filtering {
        return None;
    }

    let message = if filtering {
        t.empty_no_match(filter.trim(), total)
    } else if has_core {
        t.empty_no_profiles.to_string()
    } else {
        t.no_core_found.to_string()
    };

    Some(
        div()
            .id("empty-hint")
            .test_support()
            .aria_label(message.clone())
            .p_4()
            .rounded_md()
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .child(div().text_xs().text_color(rgb(p.muted)).child(message))
            .when(filtering, |this| {
                this.child(
                    Button::new("clear-filter").label(t.clear_filter).on_click(
                        cx.listener(|this, _, window, cx| this.on_clear_filter(window, cx)),
                    ),
                )
            })
            // An empty list with no core to launch is the one empty state that
            // cannot be acted on where it is read: the sentence names an
            // environment variable and the page that matters is another one. So
            // it carries the way there instead of leaving the reader to find it.
            .when(!has_core && !filtering, |this| {
                this.child(
                    Button::new("empty-add-core")
                        .label(t.add_browser_core)
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.on_page(Page::Cores, cx);
                        })),
                )
            }),
    )
}

pub(super) fn profile_list(
    rows: &[ProfileRow],
    selected_id: Option<ProfileId>,
    verifications: &std::collections::HashMap<ProfileId, Verification>,
    cx: &mut Context<AppView>,
    p: Palette,
    t: &Text,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_2()
        .children(rows.iter().map(|row| {
            let id = row.profile.id;
            let is_selected = selected_id == Some(id);

            div()
                .id(format!("profile-{id}"))
                .test_support()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .px_4()
                .py_3()
                .rounded_md()
                .border_1()
                .border_color(rgb(if is_selected { p.dim } else { p.border }))
                .bg(rgb(if is_selected { p.border } else { p.panel }))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| this.on_select(id, cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(row.profile.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.muted))
                                .child(t.profile_seed_line(
                                    row.profile.fingerprint.seed,
                                    row.profile.fingerprint.brand,
                                    row.profile.fingerprint.platform,
                                )),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(p.dim))
                                .child(route_label(row, t)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .items_end()
                                .gap_1()
                                .child(state_badge(row, p, t))
                                .children(verification_badge(verifications.get(&id), p, t))
                                .children(row.last_warning().map(|warning| {
                                    // The full text lives in Runtime Details; the row
                                    // only needs to say that something is off.
                                    div()
                                        .id(format!("warning-{id}"))
                                        .test_support()
                                        .max_w(px(WARNING_WIDTH))
                                        .truncate()
                                        .text_xs()
                                        .text_color(rgb(p.warning))
                                        .child(t.warning_line(warning))
                                }))
                                .children(row.last_error().map(|error| {
                                    div()
                                        .text_xs()
                                        .text_color(rgb(p.danger_strong))
                                        .child(error.to_string())
                                })),
                        )
                        .child(row_actions(row, cx, t)),
                )
        }))
}

pub(super) fn row_actions(row: &ProfileRow, cx: &mut Context<AppView>, t: &Text) -> Div {
    let id = row.profile.id;

    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            Button::new(format!("start-{id}"))
                .label(t.start)
                .primary()
                .disabled(!row.can_start())
                .on_click(cx.listener(move |this, _, _, cx| this.on_start(id, cx))),
        )
        .child(
            Button::new(format!("stop-{id}"))
                .label(t.stop)
                .danger()
                .disabled(!row.can_stop())
                .on_click(cx.listener(move |this, _, _, cx| this.on_stop(id, cx))),
        )
        .child(
            Button::new(format!("restart-{id}"))
                .label(t.restart)
                .outline()
                .disabled(!row.can_restart())
                .on_click(cx.listener(move |this, _, _, cx| this.on_restart(id, cx))),
        )
}

pub(super) fn route_label(row: &ProfileRow, t: &Text) -> String {
    match &row.proxy_name {
        Some(proxy) => t.profile_meta_proxy(&row.core_name, proxy),
        None => t.profile_meta_direct(&row.core_name),
    }
}

pub(super) fn state_badge(row: &ProfileRow, p: Palette, t: &Text) -> impl IntoElement {
    let (background, foreground) = match row.state() {
        RuntimeState::Running => (p.success_bg, p.success_strong),
        RuntimeState::Starting | RuntimeState::Stopping => (p.warning_bg, p.warning),
        RuntimeState::Stopped => (p.border, p.secondary),
        RuntimeState::Failed { .. } | RuntimeState::Crashed { .. } => {
            (p.danger_bg_soft, p.danger_strong)
        }
    };

    div()
        .id(format!("state-{}", row.profile.id))
        .role(Role::Status)
        .test_support()
        .aria_label(row.state_label(t))
        .px_2()
        .py_1()
        .rounded_full()
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(row.state_label(t))
}
