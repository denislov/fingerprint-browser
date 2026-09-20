//! The window's two palettes, and the switch between them.
//!
//! The window paints its own chrome, so it needs colours of its own; the
//! component library has its own theme, and the two have to move together or a
//! button's outline disappears into the window behind it. So the component
//! theme is the single source of truth for *which* mode is in force
//! (`Theme::change`), and [`palette`] reads it back to choose the window's
//! colours. There is no second switch to keep in step, and no way for the two
//! layers to disagree.
//!
//! Colours are `u32` in `0xRRGGBB` and reach the tree through `rgb(...)`, which
//! is how the window painted before there was more than one palette. [`Palette`]
//! is `Copy`, so a helper that has no context of its own can take one by value
//! rather than reaching for a global.

use crate::text::Text;
use gpui_kit::App;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::theme::ThemeMode;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// The window background.
    pub bg: u32,
    /// A raised surface: cards, the active navigation item.
    pub panel: u32,
    /// Lines between surfaces.
    pub border: u32,
    /// Primary text.
    pub text: u32,
    /// Secondary text that still has to read clearly.
    pub text_soft: u32,
    /// Supporting text.
    pub muted: u32,
    /// The least important text: hints, disabled rows.
    pub dim: u32,
    /// Text between [`Palette::muted`] and [`Palette::text_soft`].
    pub secondary: u32,
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
        bg: 0x18181b,
        panel: 0x1f1f23,
        border: 0x27272a,
        text: 0xf4f4f5,
        text_soft: 0xd4d4d8,
        muted: 0x71717a,
        dim: 0x52525b,
        secondary: 0xa1a1aa,
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
    /// as "green" on `0x18181b` is nearly invisible on `0xffffff`.
    ///
    /// The `_strong` variants are darker still rather than brighter, which is the
    /// opposite of what "strong" means in the dark palette. The name is about the
    /// role - a status read on a tinted background, or as a mark - not about
    /// lightness; on white, the readable end of the ramp is the dark one. They
    /// are checked, not assumed: see [`both_palettes_have_readable_contrast`].
    pub const LIGHT: Self = Self {
        bg: 0xffffff,
        panel: 0xf4f4f5,
        border: 0xd4d4d8,
        text: 0x18181b,
        text_soft: 0x3f3f46,
        muted: 0x52525b,
        dim: 0xa1a1aa,
        secondary: 0x52525b,
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
    /// Only the tests need this: at render time the window reads the palette back
    /// out of the component theme ([`palette`]), which is what keeps the two
    /// layers from disagreeing. This is the same mapping without a context, and
    /// the test below is where it is pinned to the two constants.
    #[cfg(test)]
    pub fn of(choice: ThemeChoice) -> Self {
        match choice {
            ThemeChoice::Dark => Self::DARK,
            ThemeChoice::Light => Self::LIGHT,
        }
    }
}

/// The palette the window should paint with right now.
///
/// Reads the component theme rather than the stored choice, because the
/// component theme is what [`crate::ui`] has just set - so a render and the
/// widgets beside it are always the same mode.
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
    /// than on a mistake. Everything that calls `Theme::change` takes this
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
    use gpui_kit::component::theme::Theme;

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
            Theme::change(ThemeMode::Dark, None, cx);
            assert_eq!(palette(cx), Palette::DARK);
            Theme::change(ThemeMode::Light, None, cx);
            assert_eq!(palette(cx), Palette::LIGHT);
            Theme::change(ThemeMode::Dark, None, cx);
            assert_eq!(palette(cx), Palette::DARK);
        });
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
    /// were used as text. The thresholds are WCAG 2.1 - 4.5:1 for body text, 3:1
    /// for supporting text, 2:1 for `dim`, which is hints and disabled rows and
    /// is deliberately the least legible thing on screen.
    ///
    /// Every status colour is checked in both places it is read: on the window,
    /// and on the tinted background it is paired with inside a badge. A palette
    /// that reads in one place and not the other is how a badge turns invisible.
    #[test]
    fn both_palettes_have_readable_contrast() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            let pairs = [
                ("text on the window", p.text, p.bg, 4.5),
                ("text on a card", p.text, p.panel, 4.5),
                ("soft text on the window", p.text_soft, p.bg, 4.5),
                ("supporting text", p.muted, p.bg, 3.0),
                ("the least important text", p.dim, p.bg, 2.0),
                ("a value on a card", p.secondary, p.panel, 4.5),
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
