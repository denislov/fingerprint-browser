//! The pieces the two release dialogs draw the same way.
//!
//! A core and an engine are offered by two dialogs - [`super::cores`]'s and
//! [`super::xray`]'s - and what they differ in is what a row offers, which is
//! the whole point of there being two. What they draw the same way sits here:
//! the caption above the list, the line standing in for it while it loads or
//! when it is empty, the failure box, the progress row, and the two buttons in
//! the footer.
//!
//! The ids and the words stay the callers': what is shared is how the pieces are
//! built, and a test looks for the dialog it opened rather than for "a dialog".

use super::*;
use crate::releases::Download;
use gpui_kit::component::progress::Progress;

/// The line above the list: where the builds come from, and what the dialog
/// offers.
pub(super) fn caption(
    id: &'static str,
    source: String,
    intro: &str,
    p: Palette,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .id(id)
                .test_support()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(source),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.dim))
                .child(intro.to_string()),
        )
}

/// A centred line standing in for the list: loading, or nothing to list.
pub(super) fn note(id: &'static str, text: &str, colour: u32) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .py_6()
        .flex()
        .justify_center()
        .text_xs()
        .text_color(rgb(colour))
        .child(text.to_string())
}

/// Why the list could not be read.
pub(super) fn failure(id: &'static str, message: &str, p: Palette) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(rgb(p.danger_bg))
        .text_xs()
        .text_color(rgb(p.danger))
        .child(message.to_string())
}

/// How far the download on one row has got.
///
/// The bar is drawn from the fraction only when the server said how large the
/// file is; without a total the bytes that have arrived are still shown, because
/// "42 MB" with nothing to compare it to is at least true, and a bar that
/// guessed would not be.
pub(super) fn progress(
    id: &'static str,
    bar: &'static str,
    running: &Download,
    working: &str,
    p: Palette,
    t: &'static Text,
) -> impl IntoElement {
    let received = crate::releases::size_label(running.received);
    let total = crate::releases::size_label(running.total);
    let line = if running.total > 0 {
        t.release_progress(&received, &total)
    } else {
        received.clone()
    };

    div()
        .id(id)
        .test_support()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(div().min_w_0().truncate().child(running.asset.clone()))
                .child(line),
        )
        .child(
            Progress::new(bar)
                .value(running.fraction().unwrap_or(0.0) * 100.0)
                .accessibility_label(working),
        )
}

/// The footer's button that opens the directory downloads are kept in.
pub(super) fn open_dir_button(
    id: &'static str,
    label: &str,
    view: &WeakEntity<AppView>,
    open: fn(&mut AppView, &mut Context<AppView>),
) -> Button {
    let view = view.clone();
    Button::new(id)
        .icon(icons::action(icons::glyph::OPEN_DIR))
        .label(label)
        .outline()
        .on_click(move |_, _, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, open);
            }
        })
}

/// The footer's button that asks for the list again.
pub(super) fn refresh_button(
    id: &'static str,
    label: &str,
    view: &WeakEntity<AppView>,
    loading: bool,
    refresh: fn(&mut AppView, &mut Context<AppView>),
) -> Button {
    let view = view.clone();
    Button::new(id)
        .icon(icons::action(icons::glyph::REFRESH))
        .label(label)
        .outline()
        // A list already on its way is not asked for again: two requests for one
        // dialog is one more than the API's rate limit was asked to carry.
        .disabled(loading)
        .on_click(move |_, _, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, refresh);
            }
        })
}
