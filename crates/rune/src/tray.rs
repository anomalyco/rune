//! A system tray icon with a flat menu: StatusNotifierItem and `com.canonical.dbusmenu` over
//! zbus on Linux, `Shell_NotifyIcon` on its own message-only window on Windows. Each tray runs
//! on its own thread, so it needs no event loop of the app's and answers while the app's window
//! is closed. A primary click sends [`TrayEvent::Activate`]; a secondary click opens the menu,
//! whose picks send [`TrayEvent::Menu`]. Dropping the [`Tray`] removes the icon.
//!
//! Linux shows the icon only where a tray host runs (KDE, waybar, most bars; GNOME needs an
//! extension). Without one, [`Tray::new`] still succeeds and the icon appears once a host
//! starts. macOS isn't supported yet.
use std::sync::Arc;

#[cfg(target_os = "linux")]
#[path = "tray/sni.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "tray/win32.rs"]
mod native;

/// The icon as straight (not premultiplied) RGBA rows, `width × height × 4` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcon { pub width: u32, pub height: u32, pub rgba: Vec<u8> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayItem {
    /// `id` comes back in [`TrayEvent::Menu`] when it's picked.
    Action { id: String, label: String, enabled: bool },
    Separator,
}
impl TrayItem {
    pub fn action(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::Action { id: id.into(), label: label.into(), enabled: true }
    }
    pub fn enabled(self, enabled: bool) -> Self {
        match self { Self::Action { id, label, .. } => Self::Action { id, label, enabled }, separator => separator }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    /// The icon's primary click. `token` is the XDG activation token a Wayland tray host hands
    /// over with it, for showing a window with focus.
    Activate { token: Option<String> },
    /// A menu item's `id`.
    Menu(String),
}

#[derive(Clone, Debug)]
pub struct TraySpec {
    /// A stable application id (Linux `Id`).
    pub id: String,
    pub title: String,
    pub tooltip: String,
    pub icon: TrayIcon,
    pub menu: Vec<TrayItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrayError { Unsupported, Backend(String) }
impl std::fmt::Display for TrayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::Unsupported => f.write_str("no system tray on this platform"), Self::Backend(e) => f.write_str(e) }
    }
}
impl std::error::Error for TrayError {}

/// Runs on the tray's thread; keep it short and hand work to the app.
pub(crate) type Handler = Arc<dyn Fn(TrayEvent) + Send + Sync>;

pub struct Tray {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    native: native::Native,
}

impl Tray {
    pub fn new(spec: TraySpec, on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Result<Self, TrayError> {
        if spec.icon.rgba.len() != spec.icon.width as usize * spec.icon.height as usize * 4 {
            return Err(TrayError::Backend("tray icon size doesn't match its pixels".into()));
        }
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        { native::Native::new(spec, Arc::new(on_event)).map(|native| Self { native }) }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        { let _ = (spec, on_event); Err(TrayError::Unsupported) }
    }
    pub fn set_menu(&self, menu: Vec<TrayItem>) {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        self.native.set_menu(menu);
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let _ = menu;
    }
    pub fn set_tooltip(&self, tooltip: impl Into<String>) {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        self.native.set_tooltip(tooltip.into());
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let _ = tooltip;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_leaves_separators_alone() {
        assert_eq!(TrayItem::Separator.enabled(false), TrayItem::Separator);
        assert_eq!(TrayItem::action("q", "Quit").enabled(false), TrayItem::Action { id: "q".into(), label: "Quit".into(), enabled: false });
    }

    #[test]
    fn a_short_icon_is_refused() {
        let spec = TraySpec { id: "t".into(), title: "T".into(), tooltip: String::new(),
            icon: TrayIcon { width: 2, height: 2, rgba: vec![0; 3] }, menu: Vec::new() };
        assert!(matches!(Tray::new(spec, |_| {}), Err(TrayError::Backend(_))));
    }
}
