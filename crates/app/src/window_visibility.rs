//! The window on the screen, and off it.
//!
//! "Keep running in the background" is the one exit mode that needs this: the
//! program stays and the window goes, and the tray icon is then what says so and
//! what brings the window back. What "goes" means is the whole of this module,
//! and the platforms do not agree on it:
//!
//! - **Windows** is `ShowWindowAsync`: `SW_HIDE` takes the window out of the
//!   taskbar and out of Alt+Tab, and `SW_RESTORE` brings it back.
//! - **Linux/X11** is `UnmapWindow` and `MapWindow` on the window's own XCB
//!   connection. An unmapped window is *withdrawn*: the window manager stops
//!   managing it, so it leaves the taskbar, the window list and Alt+Tab, and it
//!   comes back through an ordinary `MapRequest`. That is the X11 spelling of
//!   `SW_HIDE`, and deliberately not `WM_CHANGE_STATE` - which is *minimize*, and
//!   leaves the window manager managing a window that Cinnamon's panel still has
//!   a button for.
//! - **Linux/Wayland** is the compositor's own minimize, because `xdg_toplevel`
//!   has no withdrawn state: the protocol offers `set_minimized` and nothing
//!   underneath it, and a surface cannot be unmapped without destroying the
//!   object GPUI draws into. So on Wayland the window leaves the screen but the
//!   compositor still owns it, and the tray is a signpost rather than the only
//!   way back.
//!
//! GPUI itself will not do this. `PlatformWindow` has `minimize`, `activate` and
//! a `map_window` for window creation, and no `unmap` at all, and the
//! application-level `hide` is a no-op on every backend. What GPUI does
//! implement, on both of this program's platforms, is `raw-window-handle`:
//! `Window` is a `HasWindowHandle` and a `HasDisplayHandle`, and that is the way
//! in this module takes. It is why "the framework cannot hide a window" is a
//! reason for this file rather than for keeping the window on the screen.

use gpui_kit::Window;

#[cfg(windows)]
pub fn hide(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindowAsync};

    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::Win32(win32) = handle.as_raw()
    {
        let hwnd = win32.hwnd.get() as windows_sys::Win32::Foundation::HWND;
        if !hwnd.is_null() {
            unsafe {
                ShowWindowAsync(hwnd, SW_HIDE);
            }
        }
    }
}

#[cfg(windows)]
pub fn show(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_RESTORE, ShowWindowAsync};

    if let Ok(handle) = HasWindowHandle::window_handle(window)
        && let RawWindowHandle::Win32(win32) = handle.as_raw()
    {
        let hwnd = win32.hwnd.get() as windows_sys::Win32::Foundation::HWND;
        if !hwnd.is_null() {
            unsafe {
                ShowWindowAsync(hwnd, SW_RESTORE);
            }
        }
    }
    window.activate_window();
}

/// Takes the window off the screen, and out of everything that lists windows.
///
/// A window that cannot be unmapped falls back to the compositor's minimize
/// rather than refusing: a window out of the way is what the mode asked for, and
/// the tray icon still says the program is there. A window with no platform
/// window behind it at all - the test harness, the headless backend - is left
/// alone, because there is nothing to take away.
#[cfg(target_os = "linux")]
pub fn hide(window: &Window) {
    match kind_of(window) {
        Kind::X11 {
            connection,
            window_id,
        } => match adopt(connection) {
            Some(connection) => {
                let _ = connection.unmap_window(window_id);
                let _ = connection.flush();
            }
            // The handle named a connection that would not come up. The window is
            // still there, and minimizing it beats leaving it in the way.
            None => window.minimize_window(),
        },
        Kind::Wayland => window.minimize_window(),
        Kind::Nothing => {}
    }
}

/// Brings it back, and gives it the focus, which is what a window the user has
/// just asked for should have.
#[cfg(target_os = "linux")]
pub fn show(window: &Window) {
    if let Kind::X11 {
        connection,
        window_id,
    } = kind_of(window)
        && let Some(connection) = adopt(connection)
    {
        let _ = connection.map_window(window_id);
        let _ = connection.flush();
    }
    // The compositor's own activation, which is also what clicking the taskbar
    // button does. On X11 it raises and focuses the window that was just mapped;
    // on Wayland it is the only thing the protocol offers for bringing a
    // minimized surface forward.
    window.activate_window();
}

/// What kind of window this is, as far as putting it away is concerned.
#[cfg(target_os = "linux")]
enum Kind {
    /// X11: the connection to send the request through, and the window to name.
    X11 {
        connection: NonNull<c_void>,
        window_id: XWindow,
    },
    /// Wayland: a surface that cannot be unmapped, only minimized.
    Wayland,
    /// No platform window behind this one - the test harness or the headless
    /// backend. There is nothing to put away.
    Nothing,
}

/// What the platform says this window is.
#[cfg(target_os = "linux")]
fn kind_of(window: &Window) -> Kind {
    let (Ok(handle), Ok(display)) = (
        HasWindowHandle::window_handle(window),
        HasDisplayHandle::display_handle(window),
    ) else {
        return Kind::Nothing;
    };
    kind(&handle.as_raw(), &display.as_raw())
}

/// The part of [`kind_of`] that reads raw handles instead of a window, so that
/// which handles can be unmapped is asserted without a display.
#[cfg(target_os = "linux")]
fn kind(handle: &RawWindowHandle, display: &RawDisplayHandle) -> Kind {
    let RawWindowHandle::Xcb(window) = handle else {
        return match handle {
            RawWindowHandle::Wayland(_) => Kind::Wayland,
            _ => Kind::Nothing,
        };
    };
    let RawDisplayHandle::Xcb(display) = display else {
        return Kind::Nothing;
    };
    match display.connection {
        Some(connection) => Kind::X11 {
            connection,
            window_id: window.window.get(),
        },
        // A connection may be left empty, and a handle that cannot say which
        // connection it belongs to is not one to send a request through.
        None => Kind::Nothing,
    }
}

/// Adopts the connection GPUI owns for the window rather than taking it over:
/// `should_drop` is false, so dropping this wrapper never disconnects a
/// connection it does not own, and the window outlives the caller either way.
#[cfg(target_os = "linux")]
fn adopt(connection: NonNull<c_void>) -> Option<XCBConnection> {
    // SAFETY: the pointer is the connection the platform window was created with
    // and is still owned by it, so it outlives this call.
    unsafe { XCBConnection::from_raw_xcb_connection(connection.as_ptr(), false) }.ok()
}

#[cfg(target_os = "linux")]
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
#[cfg(target_os = "linux")]
use std::ffi::c_void;
#[cfg(target_os = "linux")]
use std::ptr::NonNull;
#[cfg(target_os = "linux")]
use x11rb::connection::Connection as _;
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::{ConnectionExt as _, Window as XWindow};
#[cfg(target_os = "linux")]
use x11rb::xcb_ffi::XCBConnection;

/// A platform with neither an XCB connection nor a Win32 message: the window can
/// only be asked to get out of the way in the one way every backend implements.
#[cfg(not(any(windows, target_os = "linux")))]
pub fn hide(window: &Window) {
    window.minimize_window();
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn show(window: &Window) {
    window.activate_window();
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use raw_window_handle::{
        WaylandWindowHandle, WebWindowHandle, XcbDisplayHandle, XcbWindowHandle,
    };
    use std::num::NonZeroU32;

    /// Which raw handles name something that can be unmapped, and which name
    /// something that has to be minimized or left alone instead. A wrong answer
    /// is not a crash - it is the window staying where it is, or being minimized
    /// rather than hidden - which is why it is asserted rather than left to a
    /// desktop to notice.
    #[test]
    fn only_an_x11_window_with_a_connection_can_be_unmapped() {
        let xcb_window =
            RawWindowHandle::Xcb(XcbWindowHandle::new(NonZeroU32::new(7).expect("non-zero")));
        let with_connection =
            RawDisplayHandle::Xcb(XcbDisplayHandle::new(Some(NonNull::dangling()), 0));
        assert!(
            matches!(
                kind(&xcb_window, &with_connection),
                Kind::X11 { window_id: 7, .. }
            ),
            "an X11 window with a connection is the case that unmaps"
        );

        let without_connection = RawDisplayHandle::Xcb(XcbDisplayHandle::new(None, 0));
        assert!(
            matches!(kind(&xcb_window, &without_connection), Kind::Nothing),
            "no connection, nothing to unmap through"
        );

        let wayland = RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::dangling()));
        assert!(
            matches!(kind(&wayland, &with_connection), Kind::Wayland),
            "a Wayland surface is minimized, because it cannot be unmapped"
        );

        // Any other platform's window: no X11 request can be sent for it, and no
        // Wayland surface is there to minimize.
        let other = RawWindowHandle::Web(WebWindowHandle::new(7));
        assert!(
            matches!(kind(&other, &with_connection), Kind::Nothing),
            "another platform's window is not one to send an X11 request for"
        );
    }
}
