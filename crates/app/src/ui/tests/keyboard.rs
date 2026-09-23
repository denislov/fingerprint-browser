//! The window's own keyboard: Tab, Enter, Escape and the one shortcut.
//!
//! These are the tests the plan's keyboard row of the acceptance matrix asks for:
//! navigation, the list, the panel, and the window's own key. What the component
//! library owns - a dialog's Escape, a menu's arrows - is locked here only where
//! this window is what wires it up.

use super::*;
use crate::state::Page;
use gpui_kit::Focusable as _;

/// Whether the keyboard is on the element with this id.
///
/// The id is owned and moved in: the test helper keeps the closure it is given
/// past the call, so a borrowed name would not live long enough to be looked up.
fn is_focused(cx: &mut gpui_kit::VisualTestContext, id: String) -> bool {
    cx.update(move |window, _| window.find(id).focused() == Some(true))
}

/// Clicks the element with this id, as a pointer would.
fn click(cx: &mut gpui_kit::VisualTestContext, id: String) {
    cx.update(move |window, cx| window.click(id, cx));
}

/// Tabs until the keyboard is on `row`, and says whether it ever arrived.
///
/// The tab order is the library's, built from the tree this window renders, so a
/// test cannot assume a number: it walks the order and stops when it is there.
/// The bound is generous - three times the controls a page has - so a failure
/// means "not reachable", not "one tab further along than the test expected".
fn tab_to_row(cx: &mut gpui_kit::VisualTestContext, row: String) -> bool {
    for _ in 0..60 {
        if is_focused(cx, row.clone()) {
            return true;
        }
        cx.update(|window, cx| window.press("tab", cx));
    }
    is_focused(cx, row)
}

#[gpui_kit::test]
fn a_row_is_something_tab_reaches_and_enter_opens_it(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let id = seed_profile(cx, &view);
    let cx = window(cx, &view);
    let row = format!("profile-{id}");

    assert!(
        !is_focused(cx, row.clone()),
        "the window opens with nothing chosen"
    );
    assert!(tab_to_row(cx, row.clone()), "Tab reaches a profile row");

    // Enter opens what the row is about, which is the one thing a row does.
    cx.update(|window, cx| window.press("enter", cx));
    settle(cx);
    assert!(
        view.read_with(cx, |view, _| view.state().details_open()),
        "Enter opens the details of the row the keyboard is on"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().selected_id()),
        Some(id)
    );
}

#[gpui_kit::test]
fn clicking_a_row_hands_it_the_keyboard(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let id = seed_profile(cx, &view);
    let cx = window(cx, &view);
    let row = format!("profile-{id}");

    click(cx, row.clone());
    settle(cx);
    assert!(
        is_focused(cx, row.clone()),
        "a row that was clicked is the row the keyboard is on"
    );
    assert!(
        cx.update(|window, _| window.find(row).label().is_some()),
        "and it says which profile it is, for a reader who cannot see it"
    );
}

#[gpui_kit::test]
fn closing_the_panel_gives_the_keyboard_back_to_the_row(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let id = seed_profile(cx, &view);
    let cx = window(cx, &view);
    let row = format!("profile-{id}");

    click(cx, row.clone());
    settle(cx);
    assert!(view.read_with(cx, |view, _| view.state().details_open()));
    click(cx, "details-close".to_string());
    settle(cx);

    assert!(
        !view.read_with(cx, |view, _| view.state().details_open()),
        "the panel is away"
    );
    assert!(
        is_focused(cx, row.clone()),
        "and the keyboard is back on the row that opened it"
    );
}

#[gpui_kit::test]
fn escape_puts_the_panel_away_and_leaves_the_rest_of_the_window_alone(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let id = seed_profile(cx, &view);
    let cx = window(cx, &view);

    click(cx, format!("profile-{id}"));
    settle(cx);
    assert!(view.read_with(cx, |view, _| view.state().details_open()));

    cx.update(|window, cx| window.press("escape", cx));
    settle(cx);
    assert!(
        !view.read_with(cx, |view, _| view.state().details_open()),
        "Escape is the way out of the top layer"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().selected_id()),
        Some(id),
        "the row stays chosen: the panel was put away, not the choice"
    );

    // With no layer open the key is inert: it does not change the page, and it
    // does not close the window the way a program-wide quit key would.
    cx.update(|window, cx| window.press("escape", cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        Page::Profiles
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().selected_id()),
        Some(id)
    );
}

#[gpui_kit::test]
fn ctrl_f_puts_the_keyboard_in_the_filter_from_any_page(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    seed_profile(cx, &view);
    let cx = window(cx, &view);
    click(cx, "nav-Settings".to_string());
    settle(cx);

    cx.update(|window, cx| window.press("ctrl-f", cx));
    settle(cx);

    assert_eq!(
        view.read_with(cx, |view, _| view.state().page()),
        Page::Profiles,
        "the shortcut brings the page that has the field with it"
    );
    let field = view
        .read_with(cx, |view, _| view.filter_input())
        .expect("the first render builds the filter field");
    assert!(
        cx.update(|window, cx| field.read(cx).focus_handle(cx).is_focused(window)),
        "and the keyboard is in the field"
    );

    // Typing goes into the field, which is the whole point of the shortcut.
    cx.update(|window, cx| window.input("Profile 2", cx));
    settle(cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.state().profile_filter().to_string()),
        "Profile 2"
    );
}

#[gpui_kit::test]
fn the_row_menu_answers_the_arrow_keys(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let id = seed_profile(cx, &view);
    let cx = window(cx, &view);

    click(cx, format!("more-{id}"));
    settle(cx);
    assert!(
        cx.update(|window, _| window.try_find("popup-menu").is_some()),
        "the menu is open"
    );

    // The first item is Edit: one step down from nothing chosen, then Enter.
    cx.update(|window, cx| window.press("down", cx));
    cx.update(|window, cx| window.press("enter", cx));
    settle(cx);
    assert!(
        view.read_with(cx, |view, _| view.editor().is_some()),
        "the keyboard reached the first item and ran it"
    );
    assert!(
        cx.update(|window, cx| window.has_active_dialog(cx)),
        "which opened the editor as a dialog"
    );

    // Escape belongs to the layer above the window's own: with a dialog open it
    // closes the dialog rather than putting anything under it away.
    let panel_open = view.read_with(cx, |view, _| view.state().details_open());
    cx.update(|window, cx| window.press("escape", cx));
    settle(cx);
    assert!(
        !cx.update(|window, cx| window.has_active_dialog(cx)),
        "Escape closed the dialog, which is the layer the keyboard was not in"
    );
    assert!(
        cx.update(|window, _| window.try_find("ok").is_none()),
        "and its buttons went with it"
    );
    assert_eq!(
        view.read_with(cx, |view, _| view.state().details_open()),
        panel_open,
        "and did not touch the layer under it"
    );
}

#[gpui_kit::test]
fn the_keyboard_stays_on_its_row_when_the_runtime_reports(cx: &mut TestAppContext) {
    cx.update(gpui_kit::init);
    let (view, _runtime) = view(cx);
    let id = seed_profile(cx, &view);
    let cx = window(cx, &view);
    let row = format!("profile-{id}");
    assert!(tab_to_row(cx, row.clone()), "Tab reaches a profile row");

    // A state change redraws every row. A handle made fresh each frame would take
    // the keyboard with it, which is the failure this test exists for: the window
    // polls the runtime every 200ms, so a row that could not survive a redraw
    // would be unusable.
    view.update(cx, |view, _| {
        view.state_mut().record_event(&RuntimeEvent::Warning {
            profile_id: id,
            message: "the engine is slower than usual".to_string(),
        });
    });
    settle(cx);

    assert!(
        is_focused(cx, row.clone()),
        "the keyboard is still on the row it was on"
    );
    assert!(
        cx.update(|window, _| window.find(row).label().is_some()),
        "and the row still says which profile it is"
    );
}
