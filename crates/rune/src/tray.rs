//! A system tray icon with a flat menu: StatusNotifierItem and `com.canonical.dbusmenu` over
//! zbus on Linux, `Shell_NotifyIcon` on its own message-only window on Windows. Each tray runs
//! on its own thread, so it needs no event loop of the app's and answers while the app's window
//! is closed. A primary click sends [`TrayEvent::Activate`]; a secondary click opens the menu,
//! whose picks send [`TrayEvent::Menu`]. Dropping the [`Tray`] removes the icon.
//!
//! Linux shows the icon only where a tray host runs (KDE, waybar, most bars; GNOME needs an
//! extension). Without one, [`Tray::new`] still succeeds and the icon appears once a host
//! starts. The Linux backend needs the default `dbus` feature. macOS isn't supported yet.
#[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "windows"))]
use std::sync::Arc;

#[cfg(all(target_os = "linux", feature = "dbus"))]
#[path = "tray/sni.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "tray/win32.rs"]
mod native;

/// The icon as straight (not premultiplied) RGBA rows, `width × height × 4` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcon { pub(crate) width: u32, pub(crate) height: u32, pub(crate) rgba: Vec<u8> }
impl TrayIcon {
    pub fn rgba(width: u32, height: u32, pixels: Vec<u8>) -> Self { Self { width, height, rgba: pixels } }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
#[non_exhaustive]
pub enum TrayEvent {
    /// The icon's primary click. `token` is the XDG activation token a Wayland tray host hands
    /// over with it, for showing a window with focus.
    Activate { token: Option<String> },
    /// A menu item's `id`.
    Menu(String),
}

#[derive(Clone, Debug)]
#[cfg_attr(not(all(target_os = "linux", feature = "dbus")), allow(dead_code))]
pub struct TraySpec {
    /// A stable application id (Linux `Id`).
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) tooltip: String,
    pub(crate) icon_name: String,
    pub(crate) icon: TrayIcon,
    pub(crate) menu: Vec<TrayItem>,
}
impl TraySpec {
    pub fn new(id: impl Into<String>, title: impl Into<String>, icon: TrayIcon) -> Self {
        Self { id: id.into(), title: title.into(), tooltip: String::new(), icon_name: String::new(), icon, menu: Vec::new() }
    }
    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self { self.tooltip = tooltip.into(); self }
    /// A freedesktop icon theme name (Linux `IconName`) a host may draw instead of the pixels.
    pub fn icon_name(mut self, name: impl Into<String>) -> Self { self.icon_name = name.into(); self }
    pub fn menu(mut self, menu: Vec<TrayItem>) -> Self { self.menu = menu; self }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrayError { Unsupported, Backend(String) }
impl std::fmt::Display for TrayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Self::Unsupported => f.write_str("no system tray on this platform"), Self::Backend(e) => f.write_str(e) }
    }
}
impl std::error::Error for TrayError {}

/// Runs on the tray's thread; keep it short and hand work to the app.
#[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "windows"))]
pub(crate) type Handler = Arc<dyn Fn(TrayEvent) + Send + Sync>;

pub struct Tray {
    #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "windows"))]
    native: native::Native,
}

/// Takes every tray icon off the notification area now, before a fast exit ends the process
/// (`crate::fast_exit`). Elsewhere the icon goes with the process: its D-Bus name (Linux) or
/// status item (macOS) ends when the connection does.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn remove_all() {
    #[cfg(target_os = "windows")]
    native::remove_all();
}

impl Tray {
    pub fn new(spec: TraySpec, on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Result<Self, TrayError> {
        if spec.icon.rgba.len() != spec.icon.width as usize * spec.icon.height as usize * 4 {
            return Err(TrayError::Backend("tray icon size doesn't match its pixels".into()));
        }
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "windows"))]
        { native::Native::new(spec, Arc::new(on_event)).map(|native| Self { native }) }
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "windows")))]
        { let _ = (spec, on_event); Err(TrayError::Unsupported) }
    }
    pub fn set_menu(&self, menu: Vec<TrayItem>) {
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "windows"))]
        self.native.set_menu(menu);
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "windows")))]
        let _ = menu;
    }
    pub fn set_tooltip(&self, tooltip: impl Into<String>) {
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "windows"))]
        self.native.set_tooltip(tooltip.into());
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "windows")))]
        let _ = tooltip;
    }
}

/// Escapes a tray menu label so Win32 `AppendMenuW` shows it literally: `&` would underline the
/// next character as a mnemonic, `\t` would start a right-aligned shortcut column, and `\0`
/// would truncate the wide string.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(super) fn win32_menu_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        match c {
            '&' => out.push_str("&&"),
            '\t' => out.push(' '),
            '\0' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Escapes a tray menu label so `com.canonical.dbusmenu` shows it literally: a single `_` marks
/// a keyboard mnemonic and is stripped by tray hosts unless doubled.
#[cfg_attr(not(any(all(target_os = "linux", feature = "dbus"), test)), allow(dead_code))]
pub(super) fn dbusmenu_label(label: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars() {
        match c {
            '_' => out.push_str("__"),
            '\0' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Encodes `text` as UTF-16 code units fitting in `max_units` without splitting a surrogate pair
/// or embedding interior NULs.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub(super) fn truncate_utf16(text: &str, max_units: usize) -> Vec<u16> {
    let mut out = Vec::with_capacity(text.len().min(max_units));
    let mut buf = [0u16; 2];
    for c in text.chars() {
        if c == '\0' { continue; }
        let encoded = c.encode_utf16(&mut buf);
        if out.len() + encoded.len() > max_units { break; }
        out.extend_from_slice(encoded);
    }
    out
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
        let spec = TraySpec::new("t", "T", TrayIcon::rgba(2, 2, vec![0; 3]));
        assert!(matches!(Tray::new(spec, |_| {}), Err(TrayError::Backend(_))));
    }

    #[test]
    fn tray_menu_labels_escape_platform_mnemonics_and_control_markers() {
        assert_eq!(win32_menu_label("Save & Quit"), "Save && Quit");
        assert_eq!(win32_menu_label("A\tB\0C"), "A BC");
        assert_eq!(win32_menu_label("open_file"), "open_file");
        assert_eq!(win32_menu_label(""), "");

        assert_eq!(dbusmenu_label("open_file"), "open__file");
        assert_eq!(dbusmenu_label("Save & Quit"), "Save & Quit");
        assert_eq!(dbusmenu_label("A\0B"), "AB");
        assert_eq!(dbusmenu_label(""), "");
    }

    #[test]
    fn utf16_truncation_never_splits_a_surrogate_pair() {
        let tip = format!("{}\u{1F600}", "a".repeat(126));
        let units = truncate_utf16(&tip, 127);
        assert_eq!(units.len(), 126);
        assert_eq!(String::from_utf16(&units).unwrap(), "a".repeat(126));

        let fits = format!("{}\u{1F600}", "a".repeat(125));
        let units = truncate_utf16(&fits, 127);
        assert_eq!(units.len(), 127);
        assert_eq!(String::from_utf16(&units).unwrap(), fits);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_tooltip_truncation_keeps_utf16_surrogate_pairs_intact() {
        // 126 ASCII chars (126 UTF-16 units) + 1 emoji (2 UTF-16 units = 128 > 127 max).
        let tip = format!("{}\u{1F600}", "a".repeat(126));
        let mut d = native::data(std::ptr::null_mut(), 0);
        native::set_tip(&mut d, &tip);
        let len = d.szTip.iter().position(|&c| c == 0).unwrap();
        assert!(String::from_utf16(&d.szTip[..len]).is_ok(), "tooltip split a UTF-16 surrogate pair: {:?}", &d.szTip[len.saturating_sub(2)..len]);
        assert_eq!(len, 126);
    }
}
