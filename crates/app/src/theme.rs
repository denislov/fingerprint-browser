//! The window's two palettes, and the switch between them.
//!
//! The window paints its own chrome, so it needs colours of its own; the
//! component library has its own theme, and the two have to move together or a
//! button's outline disappears into the window behind it. So the component
//! theme is the single source of truth for *which* mode is in force
//! ([`Theme::change`]), and [`palette`] reads it back to choose the window's
//! colours. There is no second switch to keep in step, and no way for the two
//! layers to disagree.
//!
//! Moving together is more than picking the same mode. The library's default
//! theme is a neutral one - its primary is near-black in the light palette and
//! near-white in the dark one - so a window that painted a *blue* primary
//! button of its own would have two primary colours on screen. [`apply`] is the
//! answer: it switches the mode and then writes this window's accent family
//! into the component theme, so the button the library draws and the sidebar
//! this window draws are the same blue. That is why the mode is never changed
//! with a bare `Theme::change` anywhere else in the program.
//!
//! Colours are `u32` in `0xRRGGBB` and reach the tree through `rgb(...)`, which
//! is how the window painted before there was more than one palette. [`Palette`]
//! is `Copy`, so a helper that has no context of its own can take one by value
//! rather than reaching for a global.

use crate::text::Text;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::theme::{Theme, ThemeMode, ThemeTokens};
use gpui_kit::{App, Hsla, rgb};

/// The user's choice of appearance, as it is stored and shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    /// The palette the program has always painted.
    #[default]
    Dark,
    Light,
}

impl ThemeChoice {
    /// Every choice, in the order the switch shows them.
    pub const ALL: [ThemeChoice; 2] = [Self::Dark, Self::Light];

    /// The name it is stored under.
    pub fn code(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// Reads a stored name, falling back to dark for anything it does not know.
    ///
    /// Falling back rather than refusing: a config file written by a later build
    /// that names a third theme must still start, and the dark palette is what
    /// this program has always painted.
    pub fn from_code(code: &str) -> Self {
        match code.trim().to_ascii_lowercase().as_str() {
            "light" => Self::Light,
            _ => Self::Dark,
        }
    }

    /// What the switch calls it, in the language the window is speaking.
    pub fn label(self, t: &Text) -> &'static str {
        match self {
            Self::Dark => t.theme_dark,
            Self::Light => t.theme_light,
        }
    }

    /// The component-library mode this choice means.
    pub fn mode(self) -> ThemeMode {
        match self {
            Self::Dark => ThemeMode::Dark,
            Self::Light => ThemeMode::Light,
        }
    }
}

/// The window's colours for one mode.
///
/// Semantic names rather than the colour they happen to be, so a light palette
/// is a second set of values and not a second set of call sites: `danger` is the
/// colour a failure reads in, whichever mode is on.
///
/// The names follow the roles a desktop tool needs, in the order the eye meets
/// them: surfaces first ([`Palette::bg`], [`Palette::panel`], the interaction
/// states), then text from primary to least important, then the one accent, then
/// the status colours. A name that is not used is not here - this is a palette,
/// not a colour wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The window background: the workspace behind every list and card.
    pub bg: u32,
    /// A raised surface: cards, the active navigation item's neighbours, the
    /// panel behind a list.
    pub panel: u32,
    /// A surface with the pointer over it: navigation, rows, icon buttons.
    pub hover: u32,
    /// The same surface, held down.
    pub pressed: u32,
    /// Lines between surfaces.
    pub border: u32,
    /// The tint behind a selected row or the current navigation item. Its
    /// foreground is [`Palette::accent`], which is checked against it.
    pub selected: u32,
    /// Primary text.
    pub text: u32,
    /// Secondary text that still has to read clearly.
    pub text_soft: u32,
    /// Supporting text.
    pub muted: u32,
    /// The least important text: hints and disabled rows.
    pub dim: u32,
    /// Text between [`Palette::muted`] and [`Palette::text_soft`].
    pub secondary: u32,
    /// The accent, as text or a mark on the window or on a selected surface.
    ///
    /// Not the accent as a *background*: text in this colour has to read at 4.5:1
    /// on both [`Palette::bg`] and [`Palette::selected`], which a colour dark
    /// enough for a white label cannot do in the dark palette. That colour is
    /// [`Palette::accent_fill`].
    pub accent: u32,
    /// A filled accent control: the one primary button on a page.
    pub accent_fill: u32,
    /// That control with the pointer over it.
    pub accent_fill_hover: u32,
    /// That control, held down.
    pub accent_fill_pressed: u32,
    /// The label on a filled accent control.
    pub accent_on_fill: u32,
    /// A confirmation, as text.
    pub success: u32,
    /// A confirmation, as a solid mark.
    pub success_strong: u32,
    /// A confirmation's background.
    pub success_bg: u32,
    /// A warning or a value that needs attention.
    pub warning: u32,
    /// A warning's background.
    pub warning_bg: u32,
    /// A failure, as text.
    pub danger: u32,
    /// A failure, as a solid mark.
    pub danger_strong: u32,
    /// A failure's background for a banner.
    pub danger_bg: u32,
    /// A failure's background for a small badge.
    pub danger_bg_soft: u32,
    /// An informational mark.
    pub info: u32,
}

impl Palette {
    /// The palette the window has always painted.
    pub const DARK: Self = Self {
        bg: 0x15171c,
        panel: 0x1d2027,
        hover: 0x2b303a,
        pressed: 0x343a46,
        border: 0x343a46,
        selected: 0x223653,
        text: 0xedf0f5,
        text_soft: 0xd4d8df,
        secondary: 0xc3cad6,
        muted: 0xa7b0bf,
        dim: 0x7c8798,
        accent: 0x60a5fa,
        accent_fill: 0x2563eb,
        accent_fill_hover: 0x1d4ed8,
        accent_fill_pressed: 0x1e40af,
        accent_on_fill: 0xffffff,
        success: 0x86efac,
        success_strong: 0x4ade80,
        success_bg: 0x14351f,
        warning: 0xfbbf24,
        warning_bg: 0x3a2f12,
        danger: 0xfca5a5,
        danger_strong: 0xf87171,
        danger_bg: 0x2a1a1a,
        danger_bg_soft: 0x3a1717,
        info: 0x7dd3fc,
    };

    /// The light palette, chosen for contrast against a white window.
    ///
    /// The status colours are darker than their dark-mode counterparts because
    /// they are read on white rather than on near-black: the same hue that reads
    /// as "green" on `0x15171c` is nearly invisible on `0xffffff`.
    ///
    /// The `_strong` variants are darker still rather than brighter, which is the
    /// opposite of what "strong" means in the dark palette. The name is about the
    /// role - a status read on a tinted background, or as a mark - not about
    /// lightness; on white, the readable end of the ramp is the dark one.
    ///
    /// The accent does not flip the way the status colours do: `0x2563eb` on
    /// white reads at 5:1, and the same fill carries a white label at the same
    /// ratio, so one value serves both roles in this mode.
    ///
    /// None of it is taken on trust: see [`both_palettes_have_readable_contrast`].
    pub const LIGHT: Self = Self {
        bg: 0xf6f7f9,
        panel: 0xffffff,
        hover: 0xeef1f5,
        pressed: 0xe4e9f0,
        border: 0xe2e6ec,
        selected: 0xeaf1ff,
        text: 0x202632,
        text_soft: 0x3f4a5c,
        secondary: 0x4a5567,
        muted: 0x596579,
        dim: 0x8a93a3,
        accent: 0x2563eb,
        accent_fill: 0x2563eb,
        accent_fill_hover: 0x1d4ed8,
        accent_fill_pressed: 0x1e40af,
        accent_on_fill: 0xffffff,
        success: 0x15803d,
        success_strong: 0x166534,
        success_bg: 0xdcfce7,
        warning: 0x92400e,
        warning_bg: 0xfef3c0,
        danger: 0xb91c1c,
        danger_strong: 0x991b1b,
        danger_bg: 0xfef2f2,
        danger_bg_soft: 0xfee2e2,
        info: 0x0369a1,
    };

    /// The palette for a choice.
    ///
    /// The window reads the palette back out of the component theme at render
    /// time ([`palette`]), which is what keeps the two layers from disagreeing.
    /// This is the same mapping without a context: [`apply`] needs it before
    /// there is a theme to read, and the test below pins it to the two
    /// constants.
    pub fn of(choice: ThemeChoice) -> Self {
        match choice {
            ThemeChoice::Dark => Self::DARK,
            ThemeChoice::Light => Self::LIGHT,
        }
    }
}

/// Switches the window to a palette, in both layers.
///
/// The library's theme decides what its widgets paint and the window's palette
/// decides what the window paints. Loading the library's theme alone leaves the
/// two with different primary colours - a neutral button beside a blue
/// selection - so the accent family is written across after the switch, in the
/// one place that switches.
///
/// This is the only way the mode should ever change: `Theme::change` on its own
/// is the half of the operation that leaves the layers disagreeing.
pub fn apply(choice: ThemeChoice, cx: &mut App) {
    Theme::change(choice.mode(), None, cx);
    let p = Palette::of(choice);
    let theme = Theme::global_mut(cx);
    theme.colors.primary = colour(p.accent_fill);
    theme.colors.primary_hover = colour(p.accent_fill_hover);
    theme.colors.primary_active = colour(p.accent_fill_pressed);
    theme.colors.primary_foreground = colour(p.accent_on_fill);
    // The button tokens are resolved when a theme is loaded rather than read
    // through the primary ones at paint time, so a primary button keeps the
    // library's neutral unless it is written here too.
    theme.colors.button_primary = colour(p.accent_fill);
    theme.colors.button_primary_hover = colour(p.accent_fill_hover);
    theme.colors.button_primary_active = colour(p.accent_fill_pressed);
    theme.colors.button_primary_foreground = colour(p.accent_on_fill);
    // Focus and selection are the other two places the accent has to mean the
    // same thing in both layers.
    theme.colors.ring = colour(p.accent);
    theme.colors.selection = colour(p.selected);
    // The widgets do not read those fields. They read the *legacy token table*,
    // which is a projection of them taken when the theme was loaded - so a
    // button would go on painting the library's neutral while the semantic
    // colours said blue. The projection is recomputed from the colours just
    // written, which is also why no token can be forgotten here.
    let tokens = ThemeTokens::from(&theme.colors);
    theme.tokens = tokens;
}

/// A palette colour as a theme colour.
fn colour(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// The palette the window should paint with right now.
///
/// Reads the component theme rather than the stored choice, because the
/// component theme is what [`apply`] has just set - so a render and the widgets
/// beside it are always the same mode.
pub fn palette(cx: &App) -> Palette {
    if cx.theme().is_dark() {
        Palette::DARK
    } else {
        Palette::LIGHT
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// The component theme is process-wide state, and `cargo test` runs the
    /// tests of one binary on several threads. Two tests that switch the theme
    /// at once would each see the other's palette, and fail on a race rather
    /// than on a mistake. Everything that calls [`super::apply`] takes this
    /// first, which turns that race back into a sequence.
    ///
    /// A poisoned lock is taken anyway: the panic that poisoned it is the
    /// failure a reader wants to see, not a second one about the lock.
    pub(crate) fn exclusive() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::Rgba;

    #[test]
    fn every_choice_round_trips_through_its_stored_name() {
        for choice in ThemeChoice::ALL {
            assert_eq!(ThemeChoice::from_code(choice.code()), choice);
        }
        // Case and spacing do not decide what a saved value means.
        assert_eq!(ThemeChoice::from_code("  Light "), ThemeChoice::Light);
        assert_eq!(ThemeChoice::from_code("DARK"), ThemeChoice::Dark);
    }

    /// A name this build does not know is not a reason to fail to start.
    #[test]
    fn an_unknown_name_falls_back_to_dark() {
        assert_eq!(ThemeChoice::from_code("solarized"), ThemeChoice::Dark);
        assert_eq!(ThemeChoice::from_code(""), ThemeChoice::Dark);
    }

    /// The two palettes must actually differ, or the switch would do nothing.
    #[test]
    fn the_palettes_are_different() {
        assert_ne!(Palette::DARK, Palette::LIGHT);
        assert_ne!(Palette::DARK.bg, Palette::LIGHT.bg);
        assert_ne!(Palette::DARK.text, Palette::LIGHT.text);
        assert_eq!(Palette::of(ThemeChoice::Dark), Palette::DARK);
        assert_eq!(Palette::of(ThemeChoice::Light), Palette::LIGHT);
    }

    /// Every choice names a distinct component mode, which is what the window
    /// switches: two choices mapping to one mode would be a switch that does
    /// nothing, and would look like a broken button rather than a broken mapping.
    #[test]
    fn the_choices_map_to_distinct_component_modes() {
        assert_ne!(ThemeChoice::Dark.mode(), ThemeChoice::Light.mode());
        assert_eq!(ThemeChoice::Dark.mode(), ThemeMode::Dark);
        assert_eq!(ThemeChoice::Light.mode(), ThemeMode::Light);
    }

    /// The window follows the component theme, which is what makes the two
    /// layers impossible to disagree.
    #[gpui_kit::test]
    fn the_palette_follows_the_component_theme(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = testing::exclusive();
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            apply(ThemeChoice::Dark, cx);
            assert_eq!(palette(cx), Palette::DARK);
            apply(ThemeChoice::Light, cx);
            assert_eq!(palette(cx), Palette::LIGHT);
            apply(ThemeChoice::Dark, cx);
            assert_eq!(palette(cx), Palette::DARK);
        });
    }

    /// The accent the window paints and the accent the widgets paint are one
    /// colour, in both modes.
    ///
    /// Without [`apply`]'s second half the library keeps its own neutral
    /// primary, and the first blue button beside a grey one is the bug this
    /// catches - with no way to see it but a screenshot.
    #[gpui_kit::test]
    fn both_layers_carry_the_same_accent(cx: &mut gpui_kit::TestAppContext) {
        let _exclusive = testing::exclusive();
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            for choice in ThemeChoice::ALL {
                apply(choice, cx);
                let p = Palette::of(choice);
                let colors = cx.theme().colors;
                assert_eq!(
                    bytes(colors.primary),
                    bytes(rgb(p.accent_fill).into()),
                    "{choice:?}"
                );
                assert_eq!(
                    bytes(colors.button_primary),
                    bytes(rgb(p.accent_fill).into()),
                    "{choice:?}"
                );
                assert_eq!(
                    bytes(colors.primary_foreground),
                    bytes(rgb(p.accent_on_fill).into()),
                    "{choice:?}"
                );
                assert_eq!(
                    bytes(colors.ring),
                    bytes(rgb(p.accent).into()),
                    "{choice:?}"
                );
                assert_eq!(
                    bytes(colors.selection),
                    bytes(rgb(p.selected).into()),
                    "{choice:?}"
                );
                // What the widgets actually paint with: the token table is a
                // projection of the colours above, and a stale one is a white
                // button on a white label rather than a compile error.
                let tokens = cx.theme().tokens;
                assert_eq!(bytes(*tokens.primary), bytes(rgb(p.accent_fill).into()));
                assert_eq!(
                    bytes(*tokens.button_primary),
                    bytes(rgb(p.accent_fill).into()),
                    "{choice:?}"
                );
                assert_eq!(
                    bytes(*tokens.button_primary_foreground),
                    bytes(rgb(p.accent_on_fill).into()),
                    "{choice:?}"
                );
                assert_eq!(bytes(*tokens.ring), bytes(rgb(p.accent).into()));
            }
            apply(ThemeChoice::Dark, cx);
        });
    }

    /// A colour as the three channels it is written in.
    ///
    /// Rounded rather than compared as floats: the component theme reaches its
    /// colours through a hex-to-HSL conversion, and a difference in the last
    /// bit of a mantissa is not a difference a user can see.
    fn bytes(colour: Hsla) -> [u8; 3] {
        let Rgba { r, g, b, .. } = colour.to_rgb();
        [
            (r * 255.0).round() as u8,
            (g * 255.0).round() as u8,
            (b * 255.0).round() as u8,
        ]
    }

    /// Relative luminance, per WCAG 2.1.
    fn luminance(colour: u32) -> f64 {
        let channel = |c: u32| {
            let c = c as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let r = channel((colour >> 16) & 0xff);
        let g = channel((colour >> 8) & 0xff);
        let b = channel(colour & 0xff);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// The contrast ratio between two colours, from 1:1 to 21:1.
    fn contrast(a: u32, b: u32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Both palettes are readable, which is a claim a test can check.
    ///
    /// "Chosen for contrast" is otherwise a sentence a reader has to take on
    /// trust, and the light palette is why that is not enough: its status colours
    /// were picked by eye and three of them read at 3:1 or just under once they
    /// were used as text. The thresholds are WCAG 2.1 - 4.5:1 for body text,
    /// 2:1 for `dim`, which is hints and disabled rows and is deliberately the
    /// least legible thing on screen.
    ///
    /// Supporting text is held to the body-text floor rather than the old 3:1:
    /// the page's explanations, the settings rows' help and the log's columns
    /// are all read, and a reader should not have to lean in for them. Every
    /// status colour is checked in both places it is read - on the window, and
    /// on the tinted background it is paired with inside a badge - and the
    /// accent in the three places this window uses it: on the window, on a
    /// selected row, and as the label of a filled control.
    #[test]
    fn both_palettes_have_readable_contrast() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            let pairs = [
                ("text on the window", p.text, p.bg, 4.5),
                ("text on a card", p.text, p.panel, 4.5),
                ("text on a hovered row", p.text, p.hover, 4.5),
                ("text on a selected row", p.text, p.selected, 4.5),
                ("soft text on the window", p.text_soft, p.bg, 4.5),
                ("supporting text", p.muted, p.bg, 4.5),
                ("supporting text on a card", p.muted, p.panel, 4.5),
                ("supporting text on a hovered row", p.muted, p.hover, 4.5),
                ("the least important text", p.dim, p.bg, 2.0),
                ("the least important text on a card", p.dim, p.panel, 2.0),
                ("a value on a card", p.secondary, p.panel, 4.5),
                ("the accent on the window", p.accent, p.bg, 4.5),
                ("the accent on a card", p.accent, p.panel, 4.5),
                ("the accent on a selected row", p.accent, p.selected, 4.5),
                (
                    "a label on the accent",
                    p.accent_on_fill,
                    p.accent_fill,
                    4.5,
                ),
                (
                    "a label on a hovered accent",
                    p.accent_on_fill,
                    p.accent_fill_hover,
                    4.5,
                ),
                (
                    "a label on a pressed accent",
                    p.accent_on_fill,
                    p.accent_fill_pressed,
                    4.5,
                ),
                ("a warning on the window", p.warning, p.bg, 4.5),
                (
                    "a warning on its own background",
                    p.warning,
                    p.warning_bg,
                    4.5,
                ),
                ("a failure on the window", p.danger, p.bg, 4.5),
                ("a failure on a banner", p.danger, p.danger_bg, 4.5),
                ("a failure on a badge", p.danger, p.danger_bg_soft, 4.5),
                ("a failure mark on the window", p.danger_strong, p.bg, 4.5),
                (
                    "a failure mark on a badge",
                    p.danger_strong,
                    p.danger_bg_soft,
                    4.5,
                ),
                ("a confirmation on the window", p.success, p.bg, 4.5),
                (
                    "a confirmation on its background",
                    p.success,
                    p.success_bg,
                    4.5,
                ),
                (
                    "a confirmation mark on the window",
                    p.success_strong,
                    p.bg,
                    4.5,
                ),
                (
                    "a confirmation mark on a badge",
                    p.success_strong,
                    p.success_bg,
                    4.5,
                ),
                ("information on the window", p.info, p.bg, 4.5),
            ];
            for (label, foreground, background, at_least) in pairs {
                let got = contrast(foreground, background);
                assert!(
                    got >= at_least,
                    "{name}: {label} reads at {got:.2}:1, and {at_least}:1 is the floor"
                );
            }
        }
    }
}
