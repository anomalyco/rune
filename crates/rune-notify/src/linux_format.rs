//! Platform-independent Linux notification formatting, backend selection, and portal state.
//! Compiled on Linux and in tests on every host so formatting, sandbox detection, and version
//! gating run in unit tests without a D-Bus daemon.

use std::borrow::Cow;
use std::ffi::OsStr;

use super::{ACTION, Action, Click, Notification, TOKEN};

/// The action key invoked for a click on the notification itself (both FDO and portal).
pub(crate) const DEFAULT_ACTION: &str = "default";
/// Themeable freedesktop sound name used on `org.freedesktop.Notifications` when sound is enabled.
pub(crate) const FDO_SOUND_NAME: &str = "message-new-instant";
/// Portal v2 `sound` value for the desktop's default notification sound.
pub(crate) const PORTAL_SOUND_DEFAULT: &str = "default";
/// Portal v2 `sound` value for a silent notification.
pub(crate) const PORTAL_SOUND_SILENT: &str = "silent";
/// Minimum `org.freedesktop.portal.Notification` version that accepts v2 keys such as `sound`.
pub(crate) const PORTAL_V2: u32 = 2;
/// Maximum portal notifications kept in memory to pair `ActionInvoked` clicks with user info.
pub(crate) const PORTAL_KEEP: usize = 256;
/// Prefix applied to button ids that start with `app.` on the portal, because actions starting
/// with `app.` are routed to `org.freedesktop.Application.ActivateAction` instead of
/// `ActionInvoked`.
const PORTAL_ACTION_PREFIX: &str = "rune.action.";

/// Whether the process is running inside a Flatpak sandbox: either `FLATPAK_ID` is non-empty or
/// `/.flatpak-info` exists.
pub(crate) fn in_flatpak(flatpak_id: Option<&OsStr>, flatpak_info_exists: bool) -> bool {
    flatpak_id.is_some_and(|id| !id.is_empty()) || flatpak_info_exists
}

/// What the `org.freedesktop.Notifications` server can do, from `GetCapabilities`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Caps {
    /// Clicks and buttons: without it the notification posts without either.
    pub(crate) actions: bool,
    /// The body is read as a small markup language, so it is escaped.
    pub(crate) markup: bool,
}

impl Caps {
    pub(crate) fn from_list(caps: &[String]) -> Self {
        Caps {
            actions: caps.iter().any(|c| c == "actions"),
            markup: caps.iter().any(|c| c == "body-markup"),
        }
    }
}

/// Sound hint sent to `org.freedesktop.Notifications`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FdoSound {
    /// `suppress-sound: true` (default, silent).
    Suppress,
    /// `sound-name: "<name>"` without `suppress-sound`.
    Name(&'static str),
}

pub(crate) fn fdo_sound(sound: bool) -> FdoSound {
    if sound { FdoSound::Name(FDO_SOUND_NAME) } else { FdoSound::Suppress }
}

/// `sound` entry for `org.freedesktop.portal.Notification.AddNotification`.
///
/// Only emitted when the portal reports `version >= 2`: v1 portals reject unknown keys.
pub(crate) fn portal_sound(version: u32, sound: bool) -> Option<&'static str> {
    if version < PORTAL_V2 {
        return None;
    }
    Some(if sound { PORTAL_SOUND_DEFAULT } else { PORTAL_SOUND_SILENT })
}

/// Wire action name for a portal button: keeps ids that do not start with `app.` verbatim, and
/// prefixes `app.*` ids so the portal emits `ActionInvoked` instead of D-Bus activating
/// `org.freedesktop.Application`.
pub(crate) fn portal_action_wire(id: &str) -> Cow<'_, str> {
    if id.starts_with("app.") {
        Cow::Owned(format!("{PORTAL_ACTION_PREFIX}{id}"))
    } else {
        Cow::Borrowed(id)
    }
}

/// Restores the Rune button id from a portal `ActionInvoked` action name.
pub(crate) fn portal_action_unwire(wire: &str) -> &str {
    wire.strip_prefix(PORTAL_ACTION_PREFIX).unwrap_or(wire)
}

/// One serialized button in `org.freedesktop.portal.Notification.AddNotification`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortalButton<'a> {
    pub(crate) label: &'a str,
    pub(crate) action: Cow<'a, str>,
}

/// Structured `AddNotification` payload prior to `zvariant::Value` encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortalPayload<'a> {
    pub(crate) id: &'a str,
    pub(crate) title: &'a str,
    pub(crate) body: Option<&'a str>,
    pub(crate) default_action: &'static str,
    pub(crate) buttons: Vec<PortalButton<'a>>,
    pub(crate) sound: Option<&'static str>,
}

/// Builds the portal `AddNotification` payload for `n` at portal interface `version`.
///
/// Text boxes (`Action::Reply`) are omitted, matching the `org.freedesktop.Notifications` backend.
/// The body is sent as plain text (`body`), never HTML-escaped.
pub(crate) fn portal_payload(n: &Notification, version: u32) -> PortalPayload<'_> {
    let buttons = n
        .actions
        .iter()
        .filter_map(|action| match action {
            Action::Button { id, label } => Some(PortalButton {
                label: label.as_str(),
                action: portal_action_wire(id),
            }),
            Action::Reply { .. } => None,
        })
        .collect();
    PortalPayload {
        id: &n.id,
        title: &n.title,
        body: (!n.body.is_empty()).then_some(n.body.as_str()),
        default_action: DEFAULT_ACTION,
        buttons,
        sound: portal_sound(version, n.sound),
    }
}

/// The `Notify` action list: key, label pairs, flattened. A click on the notification itself
/// invokes `default`; each button its own id. Text boxes have no equivalent and are left out.
pub(crate) fn actions_for(n: &Notification, caps: Caps) -> Vec<&str> {
    if !caps.actions {
        return Vec::new();
    }
    let mut list = vec![DEFAULT_ACTION, ""];
    for action in &n.actions {
        if let Action::Button { id, label } = action {
            list.extend([id.as_str(), label.as_str()]);
        }
    }
    list
}

/// The body as the FDO server reads it: escaped where it would take it for markup.
pub(crate) fn body_for(body: &str, caps: Caps) -> String {
    if !caps.markup {
        return body.to_owned();
    }
    let mut out = String::with_capacity(body.len());
    for c in body.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// The click for action `key` on notification `id`: its user info, plus the button and the
/// activation token when there are any.
pub(crate) fn click_for(id: &str, info: &[(String, String)], key: &str, token: Option<String>) -> Click {
    let mut info = info.to_vec();
    if key != DEFAULT_ACTION {
        info.push((ACTION.to_owned(), key.to_owned()));
    }
    if let Some(token) = token {
        info.push((TOKEN.to_owned(), token));
    }
    Click { id: id.to_owned(), info }
}

/// Bounded map from posted portal notification id to its user info. The portal has no
/// `NotificationClosed` signal, so entries are capped to prevent unbounded growth.
#[derive(Debug)]
pub(crate) struct PortalShown {
    entries: Vec<(String, Vec<(String, String)>)>,
    cap: usize,
}

impl PortalShown {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn new() -> Self {
        Self::with_cap(PORTAL_KEEP)
    }

    pub(crate) fn with_cap(cap: usize) -> Self {
        Self { entries: Vec::new(), cap }
    }

    pub(crate) fn record(&mut self, id: String, info: Vec<(String, String)>) {
        if self.cap == 0 {
            return;
        }
        self.entries.retain(|(k, _)| k != &id);
        if self.entries.len() >= self.cap {
            self.entries.remove(0);
        }
        self.entries.push((id, info));
    }

    pub(crate) fn remove(&mut self, id: &str) {
        self.entries.retain(|(k, _)| k != id);
    }

    pub(crate) fn info(&self, id: &str) -> &[(String, String)] {
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k == id)
            .map_or(&[], |(_, info)| info.as_slice())
    }
}

#[cfg(test)]
#[path = "linux_tests.rs"]
mod tests;
