//! System notifications for Rune apps: post one with an id, title, body and string user info,
//! take it back out of the system's list, and hear about clicks.
//!
//! - **macOS:** `UNUserNotificationCenter`. Only a bundled app can post (`available`); the first
//!   post asks the person for permission, and posts are dropped until they allow it.
//! - **Windows:** toast notifications (`Windows.UI.Notifications`) under the app's
//!   `AppUserModelID`. `register` gives the process that identity, with a name and icon for its
//!   toasts; nothing posts before it. With an activation scheme (`Identity::activation`) a click
//!   opens a link on that scheme, which reaches the app whether it runs or not, as long as the
//!   toast is in the notification center; `click_from_url` turns the link back into the click.
//! - **Elsewhere:** `available` is false and every call does nothing.
//!
//! Notifications are silent: they never play a sound.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "windows")]
mod windows;

/// One notification to post, built with `Notification::new(id, title)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notification {
    id: String,
    title: String,
    body: String,
    info: Vec<(String, String)>,
}

impl Notification {
    /// Posting another notification with the same `id` replaces this one.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Notification { id: id.into(), title: title.into(), ..Default::default() }
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    /// A string the click hands back (`Click::info`), such as what to open.
    pub fn info(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.info.push((key.into(), value.into()));
        self
    }
}

/// A click on one of the app's notifications, or on its default action.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Click {
    pub id: String,
    pub info: Vec<(String, String)>,
}

impl Click {
    /// The user-info value posted under `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.info.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// Who the app's notifications come from, built with `Identity::new(id, name)`. Windows shows
/// the name and icon on each toast and in Settings › System › Notifications; macOS takes both
/// from the app's bundle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Identity {
    id: String,
    name: String,
    icon: Option<PathBuf>,
    scheme: Option<String>,
}

impl Identity {
    /// `id` is the app's reverse-DNS identifier (on Windows its `AppUserModelID`, which also
    /// groups its windows on the taskbar); `name` is what people see.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Identity { id: id.into(), name: name.into(), icon: None, scheme: None }
    }

    /// An image file (PNG or ICO) for the app's toasts on Windows.
    pub fn icon(mut self, path: impl Into<PathBuf>) -> Self {
        self.icon = Some(path.into());
        self
    }

    /// Windows: clicks reach the app as `<scheme>://notification/…` links, so a click on a toast
    /// left in the notification center starts the app after it quit. `register` points the
    /// scheme at this executable (`HKEY_CURRENT_USER\Software\Classes\<scheme>`); the app hands
    /// the links it receives (`rune::App::on_open_url`) to `click_from_url`. With
    /// `rune::single_instance`, a click while the app runs reaches the running process the same
    /// way. Without a scheme a click reaches only the process that posted the toast, while it runs.
    pub fn activation(mut self, scheme: impl Into<String>) -> Self {
        self.scheme = Some(scheme.into());
        self
    }
}

/// Gives this process the app's identity. Call it first thing in `main`, before any window
/// opens, so the taskbar groups the windows under it.
///
/// Windows: sets the process's `AppUserModelID` and registers the name and icon for it under
/// `HKEY_CURRENT_USER\Software\Classes\AppUserModelId\<id>`, the identity an app without a
/// package or Start-menu shortcut posts toasts with, and the activation scheme, if any, under
/// `HKEY_CURRENT_USER\Software\Classes\<scheme>`. macOS reads the identity from the bundle, so
/// this does nothing there, nor elsewhere.
pub fn register(identity: &Identity) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        windows::register(identity)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = identity;
        Ok(())
    }
}

type Handler = Box<dyn Fn(Click) + Send + Sync>;

fn handler() -> &'static Mutex<Option<Handler>> {
    static HANDLER: OnceLock<Mutex<Option<Handler>>> = OnceLock::new();
    HANDLER.get_or_init(|| Mutex::new(None))
}

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
fn deliver(click: Click) {
    if let Some(f) = handler().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        f(click);
    }
}

/// Whether this process can post: on macOS it runs from a `.app` bundle with a bundle
/// identifier; on Windows `register` gave it an identity.
pub fn available() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::bundled()
    }
    #[cfg(target_os = "windows")]
    {
        windows::registered()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Posts `n` now (on macOS, asking for permission first if the person hasn't been asked).
/// Returns false when the app can't post at all (not `available`), or Windows refused the toast.
#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(unused_variables))]
pub fn post(n: &Notification) -> bool {
    if !available() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        mac::post(n.clone());
        true
    }
    #[cfg(target_os = "windows")]
    {
        windows::post(n)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Takes the app's delivered notifications with these ids out of Notification Center (on
/// Windows, the notification center).
pub fn remove(ids: &[&str]) {
    if ids.is_empty() || !available() {
        return;
    }
    #[cfg(target_os = "macos")]
    mac::remove(ids);
    #[cfg(target_os = "windows")]
    windows::remove(ids);
}

/// The click a notification link stands for, when `url` is one of this app's
/// (`Identity::activation`); None for any other link. Windows only: elsewhere a click never
/// arrives as a link.
pub fn click_from_url(url: &str) -> Option<Click> {
    #[cfg(target_os = "windows")]
    {
        windows::click_from_url(url)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = url;
        None
    }
}

/// Calls `f` for every click on one of the app's notifications, from any thread (wake the app's
/// loop from it). macOS brings the app forward on a click; call `bring_to_front` for the window.
/// Install it at launch: on macOS a click that launched the app arrives too. On Windows a click
/// arrives while the process that posted the toast runs, unless the identity has an activation
/// scheme: then every click comes as a link (`click_from_url`).
pub fn on_click(f: impl Fn(Click) + Send + Sync + 'static) {
    *handler().lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(f));
    #[cfg(target_os = "macos")]
    if available() {
        mac::install_delegate();
    }
}

/// Whether the app is in front with one of its windows focused, the moment it matters to a
/// notification: one that would show while the person is looking at the app is noise.
pub fn app_focused() -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::app_focused()
    }
    #[cfg(target_os = "windows")]
    {
        windows::app_focused()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}

/// Brings the app's main window forward and focuses it, restoring it if minimized (on macOS,
/// out of the Dock). Call it on the main thread (from `poll`), say after a click.
pub fn bring_to_front() {
    #[cfg(target_os = "macos")]
    mac::bring_to_front();
    #[cfg(target_os = "windows")]
    windows::bring_to_front();
}

#[cfg(test)]
mod tests;
