//! The window's own keyboard: the two keys that belong to the window rather
//! than to a control.
//!
//! Nearly all of this window's keyboard is the component library's. Every button,
//! menu, dialog and text field carries its own binding, its own tab stop and its
//! own focus ring, and a row of the Profiles list is a target of its own. What is
//! here is only what none of them can own: a shortcut that works wherever the
//! window is, and one key that puts the top layer away.
//!
//! Both are bound globally, with no key context, and that is deliberate. A
//! binding that exists on one page and not another is a shortcut nobody can
//! remember; and the layers that must win over them - a dialog, a menu, a
//! dropdown - bind the same key in a *context*, which the keymap prefers because
//! it is more specific. Escape with a dialog open therefore closes the dialog,
//! and the details panel never steals a key that belonged to the layer above it.
//!
//! Escape is deliberately not "close the window". The decision to quit belongs to
//! the window's close button, the tray and the brand menu, each of which asks;
//! a key that quit the program from anywhere in it would be a key nobody dares
//! press.

gpui_kit::actions!(fingerprint_browser, [FocusProfileFilter, CloseDetails]);

/// The keys this window answers to itself.
///
/// Bound once at startup, and once per window in a test: the two actions do
/// nothing on their own, they only ask the view for the change, so a binding that
/// exists before any page is drawn is harmless.
pub fn bind(cx: &mut gpui_kit::App) {
    cx.bind_keys([
        // The filter is the one field that is worth reaching from anywhere: the
        // list it narrows is the page the window opens on.
        gpui_kit::KeyBinding::new("ctrl-f", FocusProfileFilter, None),
        // One layer at a time: a menu or a dialog binds this key itself, and
        // this is what is left when there is neither.
        gpui_kit::KeyBinding::new("escape", CloseDetails, None),
    ]);
}
