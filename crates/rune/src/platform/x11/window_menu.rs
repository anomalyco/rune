//! The window manager's window menu for a custom titlebar on X11, through GTK's
//! `_GTK_SHOW_WINDOW_MENU` client message, which Mutter, KWin, Xfwm and others advertise in
//! `_NET_SUPPORTED`. winit's X11 `show_window_menu` does nothing, so Rune sends it itself on the
//! shared X11 connection (`platform::x11::query`), and only when the window manager advertises it.

/// The XI2 virtual core pointer, the device GTK names in `data.l[0]` for a mouse press. Window
/// managers read only the position.
pub(crate) const CORE_POINTER: u32 = 2;

/// The five 32-bit words of a `_GTK_SHOW_WINDOW_MENU` `ClientMessage`, as GTK sends them: the
/// device, then the root-window pixel the menu opens at.
pub(crate) fn show_window_menu_data(root_x: i32, root_y: i32) -> [u32; 5] {
    [CORE_POINTER, root_x as u32, root_y as u32, 0, 0]
}

/// Whether `supported` (`_NET_SUPPORTED`'s atoms) lists `atom`.
pub(crate) fn advertised(supported: &[u32], atom: u32) -> bool {
    atom != 0 && supported.contains(&atom)
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
mod native {
    use super::{advertised, show_window_menu_data};
    use x11rb::connection::Connection as _;
    use x11rb::errors::ReplyError;
    use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt as _, EventMask};

    /// Where the menu opens.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub(crate) enum At {
        /// A pixel of the window.
        Window(i32, i32),
        /// Wherever the pointer is now.
        Pointer,
    }

    /// The X11 window behind `window` and its screen, or `None` off X11.
    fn handle(window: &winit::window::Window) -> Option<(u32, Option<usize>)> {
        use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
        let xwindow = match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(w) => u32::try_from(w.window).ok()?,
            RawWindowHandle::Xcb(w) => w.window.get(),
            _ => return None,
        };
        let screen = match window.display_handle().ok().map(|h| h.as_raw()) {
            Some(RawDisplayHandle::Xlib(d)) => usize::try_from(d.screen).ok(),
            Some(RawDisplayHandle::Xcb(d)) => usize::try_from(d.screen).ok(),
            _ => None,
        };
        Some((xwindow, screen))
    }

    /// The `_GTK_SHOW_WINDOW_MENU` atom when the window manager on `root` advertises it.
    fn menu_atom(conn: &x11rb::rust_connection::RustConnection, root: u32) -> Result<Option<u32>, ReplyError> {
        let supported = conn.intern_atom(false, b"_NET_SUPPORTED")?;
        let menu = conn.intern_atom(true, b"_GTK_SHOW_WINDOW_MENU")?;
        let (supported, menu) = (supported.reply()?.atom, menu.reply()?.atom);
        if menu == 0 { return Ok(None); }
        let atoms = conn.get_property(false, root, supported, AtomEnum::ATOM, 0, 4096)?.reply()?;
        let atoms: Vec<u32> = atoms.value32().map(Iterator::collect).unwrap_or_default();
        Ok(advertised(&atoms, menu).then_some(menu))
    }

    /// Whether the window manager shows `window`'s window menu on request. `false` off X11.
    pub(crate) fn has_window_menu(window: &winit::window::Window) -> bool {
        let Some((_, screen)) = handle(window) else { return false };
        super::super::query::with_connection(|q| {
            let Some(root) = q.screen_info(screen).map(|s| s.root) else { return super::super::query::QueryOutcome::NoAnswer };
            super::super::query::classify_reply(menu_atom(&q.connection, root).map(|atom| atom.map(|_| ())))
        })
        .is_some()
    }

    /// Asks the window manager to open `window`'s window menu `at`. Returns `false` when it
    /// doesn't advertise `_GTK_SHOW_WINDOW_MENU`, the window isn't on X11, or the server can't be
    /// reached.
    pub(crate) fn show_window_menu(window: &winit::window::Window, at: At) -> bool {
        let Some((xwindow, screen)) = handle(window) else { return false };
        super::super::query::with_connection(|q| {
            super::super::query::classify_reply((|| -> Result<Option<()>, ReplyError> {
                let Some(root) = q.screen_info(screen).map(|s| s.root) else { return Ok(None) };
                let conn = &q.connection;
                let Some(atom) = menu_atom(conn, root)? else { return Ok(None) };
                let (x, y) = match at {
                    At::Window(x, y) => {
                        let to = conn.translate_coordinates(xwindow, root, x.clamp(-32768, 32767) as i16, y.clamp(-32768, 32767) as i16)?.reply()?;
                        if !to.same_screen { return Ok(None) }
                        (i32::from(to.dst_x), i32::from(to.dst_y))
                    }
                    At::Pointer => {
                        let p = conn.query_pointer(root)?.reply()?;
                        if !p.same_screen { return Ok(None) }
                        (i32::from(p.root_x), i32::from(p.root_y))
                    }
                };
                let event = ClientMessageEvent::new(32, xwindow, atom, show_window_menu_data(x, y));
                conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event)?.ignore_error();
                conn.flush()?;
                Ok(Some(()))
            })())
        })
        .is_some()
    }
}

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) use native::{At, has_window_menu, show_window_menu};

#[cfg(test)]
#[path = "window_menu_tests.rs"]
mod tests;
