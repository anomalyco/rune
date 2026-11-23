//! Focusing an already-open X11 window with a forwarded `DESKTOP_STARTUP_ID` token.
//!
//! winit 0.30 consumes a startup token only while creating a window; `focus_window` sends
//! `_NET_ACTIVE_WINDOW` with `CurrentTime` (`0`) and no startup-notification `remove:` broadcast.
//! This module parses the token's `_TIME` timestamp and builds the EWMH `_NET_ACTIVE_WINDOW` and
//! Startup Notification `remove: ID=` payloads in platform-independent helpers (tested on every
//! host), then delivers both on the shared X11 connection (`platform::x11::query`).

/// EWMH `_NET_ACTIVE_WINDOW` `data.l[0]` source indication: `1` = normal application
/// (`0` = none/legacy client, `2` = pager / direct user action).
pub(crate) const ACTIVE_WINDOW_SOURCE_APPLICATION: u32 = 1;

/// Which Startup Notification `ClientMessage` atom a 20-byte chunk uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartupChunkKind {
    /// First 20-byte chunk (`_NET_STARTUP_INFO_BEGIN`).
    Begin,
    /// Continuation chunk (`_NET_STARTUP_INFO`).
    Continue,
}

/// One 20-byte format-8 `ClientMessage` chunk of a Startup Notification broadcast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StartupChunk {
    pub(crate) kind: StartupChunkKind,
    pub(crate) data: [u8; 20],
}

/// Extracts the X server timestamp (`CARD32`) from the `_TIME<timestamp>` suffix of a
/// `DESKTOP_STARTUP_ID` value (Startup Notification Specification, "Desktop Entry" / "Startup
/// IDs"). Returns `None` when the suffix is missing, empty, non-decimal, or overflows `u32`.
pub(crate) fn parse_startup_timestamp(token: &str) -> Option<u32> {
    let (_, suffix) = token.rsplit_once("_TIME")?;
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    suffix.parse::<u32>().ok()
}

/// Builds the five 32-bit words (`data.l[0..5]`) of an EWMH `_NET_ACTIVE_WINDOW` `ClientMessage`:
/// - `data.l[0]`: source indication (`1` = normal application)
/// - `data.l[1]`: timestamp from `DESKTOP_STARTUP_ID`'s `_TIME` suffix, or `0` (`CurrentTime`)
/// - `data.l[2]`: requestor's currently active window (`0` = none)
/// - `data.l[3..5]`: `0`
pub(crate) fn active_window_data(token: &str) -> [u32; 5] {
    let timestamp = parse_startup_timestamp(token).unwrap_or(0);
    [ACTIVE_WINDOW_SOURCE_APPLICATION, timestamp, 0, 0, 0]
}

/// Formats the NUL-terminated UTF-8 payload for a Startup Notification `remove: ID=<escaped>`
/// message (Startup Notification Specification, "Format of startup notification messages").
///
/// Values containing ASCII whitespace, `"`, or `\` are enclosed in double quotes with `"` and
/// `\` escaped by a leading `\`; plain IDs are emitted unquoted. Returns `None` for empty tokens
/// or tokens containing an embedded NUL byte.
pub(crate) fn remove_message_bytes(token: &str) -> Option<Vec<u8>> {
    if token.is_empty() || token.as_bytes().contains(&0) {
        return None;
    }
    let needs_quoting = token.bytes().any(|b| {
        b.is_ascii_whitespace() || b == 0x0B || b == b'"' || b == b'\\'
    });
    let mut out = Vec::with_capacity("remove: ID=".len() + token.len() + 3);
    out.extend_from_slice(b"remove: ID=");
    if needs_quoting {
        out.push(b'"');
        for b in token.bytes() {
            if b == b'"' || b == b'\\' {
                out.push(b'\\');
            }
            out.push(b);
        }
        out.push(b'"');
    } else {
        out.extend_from_slice(token.as_bytes());
    }
    // Startup Notification messages are terminated by a NUL byte (0x00).
    out.push(0);
    Some(out)
}

/// Splits a `remove: ID=<escaped>` Startup Notification message (including its terminating NUL
/// byte) into 20-byte format-8 `ClientMessage` chunks, NUL-padding the final chunk (Startup
/// Notification Specification, "Sending a startup notification message").
pub(crate) fn remove_message_chunks(token: &str) -> Option<Vec<StartupChunk>> {
    let bytes = remove_message_bytes(token)?;
    let chunks = bytes
        .chunks(20)
        .enumerate()
        .map(|(index, chunk)| {
            let mut data = [0u8; 20];
            data[..chunk.len()].copy_from_slice(chunk);
            let kind = if index == 0 { StartupChunkKind::Begin } else { StartupChunkKind::Continue };
            StartupChunk { kind, data }
        })
        .collect();
    Some(chunks)
}

/// Activates an open X11 window using `token` (`DESKTOP_STARTUP_ID`): sends `_NET_ACTIVE_WINDOW`
/// with the token's `_TIME` timestamp and broadcasts `remove: ID=<escaped>` via
/// `_NET_STARTUP_INFO_BEGIN` / `_NET_STARTUP_INFO`. Returns `false` when the token is invalid,
/// the window has no X11 handle, or the X server cannot be reached.
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
pub(crate) fn activate(window: &winit::window::Window, token: &str) -> bool {
    use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{
        ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, WindowClass,
        WindowWrapper,
    };

    let Some(chunks) = remove_message_chunks(token) else { return false };
    let xwindow = match window.window_handle().ok().map(|h| h.as_raw()) {
        Some(RawWindowHandle::Xlib(w)) => match u32::try_from(w.window) {
            Ok(id) => id,
            Err(_) => return false,
        },
        Some(RawWindowHandle::Xcb(w)) => w.window.get(),
        _ => return false,
    };
    let screen_index = match window.display_handle().ok().map(|h| h.as_raw()) {
        Some(RawDisplayHandle::Xlib(d)) => usize::try_from(d.screen).ok(),
        Some(RawDisplayHandle::Xcb(d)) => usize::try_from(d.screen).ok(),
        _ => None,
    };
    let active_data = active_window_data(token);

    super::query::with_connection(|q| {
        super::query::classify_reply_or_id((|| -> Result<Option<()>, x11rb::errors::ReplyOrIdError> {
            let Some(screen) = q.screen_info(screen_index) else { return Ok(None) };
            let conn = &q.connection;

            // Pipeline all three atom lookups into one round-trip.
            let active_atom = conn.intern_atom(false, b"_NET_ACTIVE_WINDOW")?;
            let begin_atom = conn.intern_atom(false, b"_NET_STARTUP_INFO_BEGIN")?;
            let cont_atom = conn.intern_atom(false, b"_NET_STARTUP_INFO")?;
            let active_atom = active_atom.reply()?.atom;
            let begin_atom = begin_atom.reply()?.atom;
            let cont_atom = cont_atom.reply()?.atom;

            // EWMH `_NET_ACTIVE_WINDOW` ClientMessage:
            // - destination: screen root window, propagate = false
            // - event_mask: SubstructureRedirectMask | SubstructureNotifyMask
            // - format: 32
            // - window: the client window to activate (`xwindow`)
            // - type_: `_NET_ACTIVE_WINDOW`
            // - data.l[0]: source indication (1 = normal application)
            // - data.l[1]: timestamp from `_TIME<ts>` (or 0 = CurrentTime)
            // - data.l[2]: requestor's currently active window (0 = none)
            // - data.l[3..5]: 0
            let active_event = ClientMessageEvent::new(32, xwindow, active_atom, active_data);
            conn.send_event(
                false,
                screen.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                active_event,
            )?
            .ignore_error();

            // Startup Notification Specification ("Sending a startup notification message"):
            // create an unmapped override-redirect window whose XID identifies this broadcast
            // sequence (no event_mask so destroying it does not queue unread DestroyNotify events
            // on the shared connection), send 20-byte format-8 ClientMessages to the root window
            // with PropertyChangeMask, then destroy the temporary window.
            let sender = WindowWrapper::create_window(
                conn,
                screen.root_depth,
                screen.root,
                -100,
                -100,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                screen.root_visual,
                &CreateWindowAux::new().override_redirect(1),
            )?;
            for chunk in &chunks {
                let message_type = match chunk.kind {
                    StartupChunkKind::Begin => begin_atom,
                    StartupChunkKind::Continue => cont_atom,
                };
                // Startup Notification ClientMessage:
                // - destination: screen root window, propagate = false
                // - event_mask: PropertyChangeMask
                // - format: 8
                // - window: temporary sender window XID
                // - type_: `_NET_STARTUP_INFO_BEGIN` (first chunk) or `_NET_STARTUP_INFO` (rest)
                // - data.b[0..20]: 20-byte chunk of the NUL-terminated UTF-8 message
                let event = ClientMessageEvent::new(8, sender.window(), message_type, chunk.data);
                conn.send_event(false, screen.root, EventMask::PROPERTY_CHANGE, event)?
                    .ignore_error();
            }
            // Destroy the temporary window before flushing so DestroyWindow goes out immediately.
            drop(sender);
            conn.flush()?;
            Ok(Some(()))
        })())
    })
    .is_some()
}

#[cfg(test)]
#[path = "activation_tests.rs"]
mod tests;
