//! The pieces every page is built from, and the measurements they share.
//!
//! Five pages that each lay themselves out are five chances to disagree about
//! how tall a title is, how far apart two rows sit or what a disabled control
//! looks like. The baseline lives here instead: one page header, one card, one
//! status badge, one empty state, and the handful of numbers the rest of the
//! window measures against.
//!
//! The numbers are a scale rather than a set of magic values - 4, 8, 12, 16, 24,
//! 32, which are `gap_1` to `gap_8` in GPUI - so spacing is chosen from a
//! vocabulary instead of per page. Only the sizes GPUI has no utility for are
//! named here: the page title, a section heading, a control's height and the
//! sidebar's width.

use super::*;
use crate::ui::icons;
use gpui_kit::assets::IconName;

/// The sidebar's width, excluding its right border.
///
/// Wide enough for the longest navigation label in either language - "Browser
/// Cores", an 18px icon and 8px between them - and no wider: the workspace
/// beside it is what the reader came for.
pub const SIDEBAR: f32 = 200.0;

/// The padding around a page's content.
pub const PAGE_PADDING: f32 = 24.0;

/// Below this viewport width, resource metadata moves below the name. The
/// widest table needs 770px of fixed columns, 64px of gaps, 34px of row padding
/// and borders, plus a readable name (200px), sidebar and page padding. Keep
/// this independent of the profile details panel's docking breakpoint.
pub const RESOURCE_TABLE_MIN: f32 = 1364.0;

/// A page title's size.
pub const TITLE: f32 = 22.0;

/// A card or group heading's size.
pub const SECTION: f32 = 15.0;

/// The height of a control this window draws itself: a chip, a row, a tab.
///
/// Buttons and fields come from the component library at 32px, which is its
/// medium size and sits well beside this one; what needs a number of its own is
/// everything the window paints, so a row of mixed controls lines up along one
/// edge instead of stepping by two pixels per element.
pub const CONTROL: f32 = 34.0;

/// The radius of a control.
pub const RADIUS_CONTROL: f32 = 6.0;

/// The radius of a list, a card or a panel.
pub const RADIUS_SURFACE: f32 = 8.0;

/// The height of a list row on the pages that show a table.
///
/// Tall enough for a name and the state under it, and the same on every page: a
/// reader who has learned where a row's controls are on one list finds them in
/// the same place on the next.
pub const ROW_HEIGHT: f32 = 64.0;

/// A page's heading: what the page is, what it is showing, and the one thing to
/// do next.
///
/// The title and its summary are one column and the action is another, so every
/// page puts its primary action in the same place. The summary carries an
/// accessible name of its own because it is a status line: a screen reader
/// should hear the count, not the sentence that happens to hold it.
pub struct PageHeader {
    id: &'static str,
    title: SharedString,
    summary: SharedString,
    aria: SharedString,
    action: Option<AnyElement>,
}

impl PageHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        let title = title.into();
        Self {
            id: "page-summary",
            aria: title.clone(),
            summary: SharedString::default(),
            title,
            action: None,
        }
    }

    /// The line under the title, and what it says when it is read aloud.
    pub fn summary(
        mut self,
        id: &'static str,
        text: impl Into<SharedString>,
        aria: impl Into<SharedString>,
    ) -> Self {
        self.id = id;
        self.summary = text.into();
        self.aria = aria.into();
        self
    }

    /// The page's one primary action, on the right.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    pub fn render(self, p: Palette) -> Div {
        div()
            .flex()
            .items_start()
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
                            .text_size(px(TITLE))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(p.text))
                            .child(self.title),
                    )
                    .child(
                        div()
                            .id(self.id)
                            .test_support()
                            .aria_label(self.aria)
                            .text_xs()
                            .text_color(rgb(p.muted))
                            .child(self.summary),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .children(self.action),
            )
    }
}

/// A card: the surface a group of controls or values sits on.
///
/// A list or a card is a surface, not a control: one step rounder than a button,
/// with the subtle border rather than a heavy one. Rows inside it are separated
/// by their own rules, not by a card each.
pub fn card(p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .px_4()
        .py_4()
        .rounded(px(RADIUS_SURFACE))
        .border_1()
        .border_color(rgb(p.border))
        .bg(rgb(p.panel))
}

/// A card's heading, and the sentence that explains it.
///
/// The mark beside the title is the group's, not the card's: the eye finds
/// "the appearance card" by the same drawing every time, and the icon is held to
/// the muted colour so eight of them do not turn the page into a colour chart.
pub fn card_heading(icon: IconName, title: &str, body: &str, p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(icons::action(icon).text_color(rgb(p.muted)))
                .child(
                    div()
                        .text_size(px(SECTION))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(p.text))
                        .child(title.to_string()),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(p.muted))
                .child(body.to_string()),
        )
}

/// The dim line under a card's controls: what would happen, or what did.
pub fn card_note(text: String, p: Palette) -> Div {
    div().text_xs().text_color(rgb(p.muted)).child(text)
}

/// A labelled row of controls inside a card.
pub fn card_row(label: &str, p: Palette) -> Div {
    div().flex().flex_wrap().gap_2().child(
        div()
            .w_full()
            .text_xs()
            .text_color(rgb(p.muted))
            .child(label.to_string()),
    )
}

/// One chip of a small set of choices.
///
/// The chosen one is filled and lit; the others are outlined and quiet. Both are
/// always on screen, which is the point: a set this small should not hide its
/// alternatives behind a menu or a cycle.
pub fn chip(id: String, label: &str, active: bool, p: Palette) -> Button {
    // A choice is a control: use the library's persistent focus handle, keyboard
    // activation and focus ring instead of a pointer-only div.
    Button::new(id)
        .label(label.to_string())
        .toggled(active)
        // `toggled` describes accessibility; `selected` also prevents the
        // ordinary hover/pressed palette from replacing the selected colours.
        .selected(active)
        .outline()
        .h(px(CONTROL))
        .rounded(px(RADIUS_CONTROL))
        .when(active, |this| {
            this.bg(rgb(p.selected))
                .border_color(rgb(p.accent))
                .text_color(rgb(p.accent))
                .font_weight(FontWeight::MEDIUM)
        })
}

/// How a status reads: the five tints the window uses, and nothing else.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Neither good nor bad: stopped, unset, not used.
    Neutral,
    /// Running, or a check that passed.
    Success,
    /// Waiting, or something that needs attention.
    Warning,
    /// Failed, crashed, or refused.
    Danger,
    /// Information the reader did not ask for.
    Info,
}

/// A status, as a pill.
///
/// The tints are the palette's, so a badge reads the same on a card and in a
/// row, and each of them is checked against its own background by the contrast
/// test in [`crate::theme`]. The tone is carried by more than the colour: the
/// pill always holds the state's own word, which is what a reader who cannot
/// separate the greens from the reds goes by.
pub fn status_badge(
    id: impl Into<ElementId>,
    tone: Tone,
    label: impl Into<SharedString>,
    aria: impl Into<SharedString>,
    p: Palette,
) -> impl IntoElement {
    let (background, foreground) = match tone {
        Tone::Neutral => (p.hover, p.secondary),
        Tone::Success => (p.success_bg, p.success_strong),
        Tone::Warning => (p.warning_bg, p.warning),
        Tone::Danger => (p.danger_bg_soft, p.danger_strong),
        Tone::Info => (p.selected, p.accent),
    };
    div()
        .id(id)
        .role(Role::Status)
        .test_support()
        .aria_label(aria)
        .flex()
        .items_center()
        .h(px(22.0))
        .px_2()
        .rounded_full()
        .bg(rgb(background))
        .text_color(rgb(foreground))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .child(label.into())
}

/// A page with nothing on it, and the one way out.
///
/// The three empty pages are not the same page: no cores, no profiles and no
/// search results have different causes and different answers. What they share
/// is the shape - a mark, a reason, and the action that changes it - which is
/// what stops a reader from having to work out which kind of empty they are in.
pub struct EmptyState {
    id: &'static str,
    icon: IconName,
    title: SharedString,
    description: SharedString,
    action: Option<AnyElement>,
}

impl EmptyState {
    pub fn new(
        id: &'static str,
        icon: IconName,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
    ) -> Self {
        Self {
            id,
            icon,
            title: title.into(),
            description: description.into(),
            action: None,
        }
    }

    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }

    pub fn render(self, p: Palette) -> impl IntoElement {
        div()
            .id(self.id)
            .test_support()
            .aria_label(format!("{} {}", self.title, self.description))
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .px_6()
            .py_8()
            .rounded(px(RADIUS_SURFACE))
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(p.panel))
            .child(icons::empty(self.icon, p))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(p.text))
                    .child(self.title),
            )
            .child(
                div()
                    .max_w(px(520.0))
                    .text_center()
                    .text_xs()
                    .text_color(rgb(p.muted))
                    .child(self.description),
            )
            .children(self.action.map(|action| div().pt_2().child(action)))
    }
}

/// An icon-only button, with the name it is announced by.
///
/// The label is not optional: a button whose only content is a drawing has no
/// name unless one is given, and "the ⋯ one" is not a name a screen reader can
/// say. The tooltip is the same string, so the hover text and accessible name
/// agree. The library currently shows tooltips only on hover, not on focus.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: IconName,
    label: impl Into<SharedString>,
) -> Button {
    let label: SharedString = label.into();
    Button::new(id)
        .ghost()
        .icon(icons::action(icon))
        .tooltip(label.clone())
        .accessibility_label(label)
}

/// A tooltip for any element, holding the words that element is named by.
///
/// The component library's own tooltip renderer rather than the window's: a
/// tooltip that looked like nothing else in the window would be a second visual
/// language for one hover.
pub fn tooltip(
    label: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let label: SharedString = label.into();
    move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
}

/// One item of a row's menu, bound to the object the row stands for.
///
/// The closure receives the window rather than capturing it: every action a row
/// offers is a method on [`AppView`], and the view is the only thing that can
/// open a dialog, write to the clipboard or change a page.
pub fn menu_item(
    view: &WeakEntity<AppView>,
    label: &'static str,
    icon: IconName,
    disabled: bool,
    run: impl Fn(&mut AppView, &mut Window, &mut Context<AppView>) + 'static,
) -> PopupMenuItem {
    let view = view.clone();
    PopupMenuItem::new(label)
        .icon(icons::action(icon))
        .disabled(disabled)
        .on_click(move |_, window, cx| {
            if let Some(view) = view.upgrade() {
                view.update(cx, |view, cx| run(view, window, cx));
            }
        })
}
