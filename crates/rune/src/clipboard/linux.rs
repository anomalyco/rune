//! What arboard leaves out on Linux for secrets: whose copy the clipboard holds, and a wipe
//! that only takes Rune's.
//!
//! On X11 the clipboard's owner is a window, so Rune remembers the one that took it with the
//! secret's write and tells another app's copy of the same text from its own. The wipe runs
//! with the server grabbed: no other client's request is handled between finding that window
//! still owns the clipboard and taking the clipboard away from it. Rune grabs nothing else and
//! asks arboard nothing while it holds the grab, so arboard's thread can't wait on it.
//!
//! Wayland has no owner to ask and no conditional clear. A secret Rune copies carries the
//! password-manager hint (arboard's exclusion from clipboard history), so a copy without it isn't
//! Rune's; one with it and the same text can't be told apart, and an app that copies between
//! the check and the clear loses its copy.
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use super::Wipe;

/// The MIME type arboard adds to a copy excluded from clipboard history.
const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

/// The display server arboard picked, chosen the way arboard chooses it.
pub(super) enum Session {
    X11(Option<X11>),
    Wayland,
}

impl Session {
    pub(super) fn open() -> Self {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some() && wl_clipboard_rs::utils::is_primary_selection_supported().is_ok();
        if wayland { Session::Wayland } else { Session::X11(X11::connect()) }
    }

    pub(super) fn has_writer(&self) -> bool { matches!(self, Session::X11(Some(_))) }

    /// The window that owns the clipboard (0 for none), on X11.
    pub(super) fn writer(&mut self) -> Result<Option<u64>, String> {
        match self {
            Session::X11(Some(x11)) => x11.owner().map(Some),
            Session::X11(None) | Session::Wayland => Ok(None),
        }
    }

    /// Takes the clipboard from `writer` if it still owns it, as one step.
    pub(super) fn release_if(&mut self, writer: u64) -> Wipe {
        match self {
            Session::X11(Some(x11)) => x11.release_if(writer),
            Session::X11(None) | Session::Wayland => Wipe::Failed,
        }
    }

    /// Whether the clipboard's copy carries the password hint Rune's secret copies carry.
    pub(super) fn marked_secret(&self) -> Result<bool, String> {
        use wl_clipboard_rs::paste::{ClipboardType, Seat, get_mime_types};
        match self {
            Session::Wayland => get_mime_types(ClipboardType::Regular, Seat::Unspecified)
                .map(|types| types.contains(PASSWORD_HINT))
                .map_err(|e| e.to_string()),
            Session::X11(_) => Ok(true),
        }
    }
}

/// A connection of Rune's own, beside arboard's, to ask who owns the clipboard.
pub(super) struct X11 {
    conn: RustConnection,
    clipboard: u32,
}

impl X11 {
    fn connect() -> Option<Self> {
        let (conn, _) = x11rb::connect(None).ok()?;
        let clipboard = conn.intern_atom(false, b"CLIPBOARD").ok()?.reply().ok()?.atom;
        Some(Self { conn, clipboard })
    }

    fn owner(&self) -> Result<u64, String> {
        let reply = self.conn.get_selection_owner(self.clipboard).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?;
        Ok(u64::from(reply.owner))
    }

    fn release_if(&self, writer: u64) -> Wipe {
        if self.conn.grab_server().is_err() { return Wipe::Failed; }
        let wipe = match self.owner() {
            Err(_) => Wipe::Failed,
            Ok(owner) if owner != writer || owner == 0 => Wipe::NotOurs,
            Ok(_) => match self.conn.set_selection_owner(x11rb::NONE, self.clipboard, x11rb::CURRENT_TIME) {
                Ok(_) => Wipe::Done,
                Err(_) => Wipe::Failed,
            },
        };
        // A round trip: the server has handled the release and the ungrab before this returns.
        let released = self.conn.ungrab_server().is_ok() && self.conn.get_input_focus().ok().and_then(|c| c.reply().ok()).is_some();
        if released { wipe } else { Wipe::Failed }
    }
}
