//! The window's icon vocabulary, and the assets that draw it.
//!
//! Icons are one family, used one way. Every icon comes from the Lucide set the
//! component library already bundles, so nothing here is a second art style: the
//! window's own marks and the widgets beside them are drawn by the same
//! strokes at the same weight.
//!
//! Three things live here and nowhere else:
//!
//! - **The sizes.** [`NAV`], [`ACTION`] and [`EMPTY`] are the only sizes an icon
//!   is drawn at, so a row of icons has one rhythm instead of five.
//! - **The meanings.** [`glyph`] names the icons after what they mean in this
//!   window - [`glyph::NAV_PROFILES`], not `PanelsTopLeft` - so a page asks for
//!   the concept and the set decides the drawing.
//! - **The asset source.** [`AppAssets`] serves those icons, the component
//!   library's own defaults underneath them, and the application's brand mark.
//!   It is registered once, in `main`, and a test below holds every name in
//!   [`glyph`] to it, which is what keeps a typo from becoming a blank square
//!   in a screenshot nobody looked at.
//!
//! Colours are deliberately absent: an icon inherits the text colour it sits
//! beside, so a muted row draws a muted icon and an active one draws an active
//! icon without either of them naming a colour.

use crate::theme::Palette;
use gpui_kit::assets::{Assets as ComponentIcons, IconName, icon_assets};
use gpui_kit::component::Icon;
use gpui_kit::component::Sizable as _;
use gpui_kit::*;

/// A navigation icon: the sidebar, where the icon carries the label.
pub const NAV: f32 = 18.0;
/// An action icon: buttons, list rows and menu items.
pub const ACTION: f32 = 16.0;
/// The single icon an empty page shows.
pub const EMPTY: f32 = 36.0;

/// The brand mark, as the application ships it.
///
/// The same artwork the packaging scripts turn into the window, the tray and the
/// Windows resource section, served through this source rather than copied into a
/// second file that could drift from them.
///
/// The PNG rather than the SVG beside it: GPUI renders an SVG's *fills* and not
/// its strokes, and that mark is drawn almost entirely with strokes - served as
/// the SVG it is a dark square and a sub-pixel dot, which is to say nothing at
/// all on a dark sidebar.
pub const BRAND: &str = "brand/icon.png";

/// Where the brand mark is read from at build time.
///
/// `CARGO_MANIFEST_DIR` rather than a relative path: this file sits four
/// directories below the asset, and a path written out by hand would be a
/// second copy of the directory layout, wrong the first time anything moved.
const BRAND_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/icon.png"
));

// The icons the window uses that the component library's default bundle does
// not carry. The default bundle is small on purpose - it holds what the widgets
// draw for themselves - and the full Lucide catalogue is 7 MB of SVG. This is
// the third option: exactly the marks this window asks for, embedded and
// nothing else. Adding an icon to `glyph` means adding it here, and the test
// below is what says so.
icon_assets!(
    ExtraIcons,
    [
        Activity,
        Archive,
        Box,
        Database,
        Download,
        PanelsTopLeft,
        Pencil,
        Plug,
        Power,
        RefreshCw,
        ScrollText,
        ShieldCheck,
        Square,
        Stethoscope,
        Trash,
        Upload,
    ]
);

/// Everything the window can draw: its own icons, the component defaults, and
/// the brand mark.
///
/// Registered with `Application::with_assets`, which is the only place a GPUI
/// application can be told where its assets come from.
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        // The window's own marks first, then the brand, then the library's
        // defaults: a name this source knows is answered here, and everything
        // else falls through to the bundle the widgets expect to find.
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        if path == BRAND {
            return Ok(Some(std::borrow::Cow::Borrowed(BRAND_PNG)));
        }
        ComponentIcons.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = ExtraIcons.list(path)?;
        paths.extend(ComponentIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

/// The icons this window means, named for the meaning.
///
/// A page asks for [`glyph::START`] and gets a play triangle; if the set is ever
/// revisited, one line changes here instead of every call site.
pub mod glyph {
    use super::IconName;

    // ---- navigation ----
    pub const NAV_PROFILES: IconName = IconName::PanelsTopLeft;
    pub const NAV_PROXIES: IconName = IconName::Network;
    pub const NAV_CORES: IconName = IconName::Cpu;
    pub const NAV_LOG: IconName = IconName::ScrollText;
    pub const NAV_SETTINGS: IconName = IconName::Settings;

    // ---- the shell ----
    pub const MORE: IconName = IconName::Ellipsis;
    /// Putting the navigation's labels away, and bringing them back: a sidebar
    /// drawn with a mark, and one drawn on its own.
    pub const SIDEBAR_COLLAPSE: IconName = IconName::PanelLeftClose;
    pub const SIDEBAR_EXPAND: IconName = IconName::PanelLeftOpen;
    pub const ABOUT: IconName = IconName::Info;
    pub const CLOSE_WINDOW: IconName = IconName::Close;
    pub const QUIT_ALL: IconName = IconName::Power;
    /// Leaving a panel that has taken the place of the thing it describes.
    pub const BACK: IconName = IconName::ArrowLeft;
    /// A group that is closed, and one that is open.
    pub const EXPAND: IconName = IconName::ChevronRight;
    pub const COLLAPSE: IconName = IconName::ChevronDown;

    // ---- the pages' primary actions ----
    pub const NEW: IconName = IconName::Plus;
    pub const TEST: IconName = IconName::Activity;
    pub const REDETECT: IconName = IconName::RefreshCw;
    pub const IMPORT_LINK: IconName = IconName::Download;
    pub const EXPORT: IconName = IconName::Upload;
    pub const IMPORT_FILE: IconName = IconName::Download;
    pub const RESTORE: IconName = IconName::Archive;
    pub const COPY: IconName = IconName::Copy;
    pub const CLEAR: IconName = IconName::Trash;

    // ---- the row menus ----
    pub const EDIT: IconName = IconName::Pencil;
    pub const DUPLICATE: IconName = IconName::Copy;
    pub const OPEN_DIR: IconName = IconName::FolderOpen;
    pub const VERIFY: IconName = IconName::ShieldCheck;
    pub const DELETE: IconName = IconName::Trash;
    pub const RESTART: IconName = IconName::RotateCw;
    pub const START: IconName = IconName::Play;
    pub const STOP: IconName = IconName::Square;
    /// A state, not an action: the mark beside a failure summary.
    pub const FAILED: IconName = IconName::TriangleAlert;

    // ---- the close dialog's three answers ----
    //
    // The window goes away, the browsers keep going, or everything stops. Each
    // answer is drawn by a mark that already means that thing elsewhere in the
    // window, so the dialog needs no vocabulary of its own.
    pub const EXIT_BACKGROUND: IconName = IconName::Close;
    pub const EXIT_KEEP_RUNNING: IconName = IconName::Play;
    pub const EXIT_STOP_ALL: IconName = IconName::Power;

    // ---- the settings cards ----
    pub const APPEARANCE: IconName = IconName::Palette;
    pub const EXIT_BEHAVIOUR: IconName = IconName::Close;
    pub const DATA: IconName = IconName::Database;
    pub const DIAGNOSTICS: IconName = IconName::Stethoscope;

    // ---- empty pages ----
    pub const EMPTY_PROFILES: IconName = IconName::Box;
    pub const EMPTY_PROXIES: IconName = IconName::Plug;
    pub const EMPTY_CORES: IconName = IconName::Cpu;
    pub const EMPTY_SEARCH: IconName = IconName::Search;
}

/// An action-sized icon, in the colour of the text beside it.
pub fn action(icon: IconName) -> Icon {
    Icon::new(icon).with_size(px(ACTION))
}

/// A navigation-sized icon, in the colour of the text beside it.
pub fn nav(icon: IconName) -> Icon {
    Icon::new(icon).with_size(px(NAV))
}

/// The one icon an empty page shows, quieter than the text below it.
pub fn empty(icon: IconName, p: Palette) -> Icon {
    Icon::new(icon).with_size(px(EMPTY)).text_color(rgb(p.dim))
}

#[cfg(test)]
mod tests {
    // Named rather than globbed: `gpui_kit::*` carries GPUI's own `test` macro
    // once the test-support feature is on, and a glob import would shadow the
    // built-in attribute with it.
    use super::{AppAssets, BRAND, glyph};
    use gpui_kit::AssetSource as _;
    use gpui_kit::assets::IconName;

    /// Every mark [`glyph`] names is one this source can actually draw.
    ///
    /// The failure this catches is a name that exists in the catalogue but was
    /// not added to [`ExtraIcons`]: it compiles, and the icon it draws is
    /// nothing at all. A screenshot would show it; a test shows it first.
    #[test]
    fn every_icon_the_window_names_is_drawable() {
        let names = [
            glyph::NAV_PROFILES,
            glyph::NAV_PROXIES,
            glyph::NAV_CORES,
            glyph::NAV_LOG,
            glyph::NAV_SETTINGS,
            glyph::MORE,
            glyph::ABOUT,
            glyph::CLOSE_WINDOW,
            glyph::QUIT_ALL,
            glyph::BACK,
            glyph::EXPAND,
            glyph::COLLAPSE,
            glyph::NEW,
            glyph::TEST,
            glyph::REDETECT,
            glyph::IMPORT_LINK,
            glyph::EXPORT,
            glyph::IMPORT_FILE,
            glyph::RESTORE,
            glyph::COPY,
            glyph::CLEAR,
            glyph::EDIT,
            glyph::DUPLICATE,
            glyph::OPEN_DIR,
            glyph::VERIFY,
            glyph::DELETE,
            glyph::RESTART,
            glyph::START,
            glyph::STOP,
            glyph::FAILED,
            glyph::EXIT_BACKGROUND,
            glyph::EXIT_KEEP_RUNNING,
            glyph::EXIT_STOP_ALL,
            glyph::APPEARANCE,
            glyph::EXIT_BEHAVIOUR,
            glyph::DATA,
            glyph::DIAGNOSTICS,
            glyph::EMPTY_PROFILES,
            glyph::EMPTY_PROXIES,
            glyph::EMPTY_CORES,
            glyph::EMPTY_SEARCH,
        ];
        for name in names {
            let path = name.path();
            let loaded = AppAssets.load(&path).expect("the asset source never fails");
            assert!(loaded.is_some(), "{path} is not in the bundle");
        }
    }

    /// The brand mark is served, and served as the PNG it is declared to be.
    #[test]
    fn the_brand_mark_is_served() {
        let bytes = AppAssets
            .load(BRAND)
            .expect("the asset source never fails")
            .expect("the brand mark is bundled");
        assert!(
            bytes.starts_with(&[0x89, b'P', b'N', b'G']),
            "the brand mark is not a PNG"
        );
    }

    /// The component library's own icons still resolve through this source.
    ///
    /// A window that serves only its own marks draws buttons and selects with
    /// missing glyphs, which looks like a broken build rather than a missing
    /// file.
    #[test]
    fn the_component_defaults_are_still_served() {
        let check = IconName::Check.path();
        assert!(
            AppAssets
                .load(&check)
                .expect("the asset source never fails")
                .is_some(),
            "{check} fell through the component bundle"
        );
    }
}
