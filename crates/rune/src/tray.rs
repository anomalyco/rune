//! A system tray icon with a flat menu: StatusNotifierItem and `com.canonical.dbusmenu` over
//! zbus on Linux, `NSStatusBar` / `NSStatusItem` on macOS, and `Shell_NotifyIcon` on its own
//! message-only window on Windows. A primary click sends [`TrayEvent::Activate`]; a secondary
//! click (or Control-click on macOS) opens the menu, whose picks send [`TrayEvent::Menu`].
//! Dropping the [`Tray`] removes the icon.
//!
//! Linux and Windows run each tray on its own thread, so it needs no event loop of the app's
//! and answers while the app's window is closed. On macOS [`Tray::new`] must run on the main
//! thread (and works while every window is hidden, including in an accessory app), while
//! [`Tray::set_menu`], [`Tray::set_tooltip`], [`Tray::set_icon`], [`Tray::set_icon_name`],
//! [`Tray::set_title`], [`Tray::set_highlighted`], [`Tray::rect`] and dropping [`Tray`] may
//! happen from any thread.
//!
//! Linux shows the icon only where a tray host runs (KDE, waybar, most bars; GNOME needs an
//! extension). Without one, [`Tray::new`] still succeeds and the icon appears once a host
//! starts. The Linux backend needs the default `dbus` feature.
#[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
use std::sync::Arc;

#[cfg(all(target_os = "linux", feature = "dbus"))]
#[path = "tray/sni.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "tray/macos.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "tray/win32.rs"]
mod native;

/// The icon as straight (not premultiplied) RGBA rows, `width × height × 4` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcon {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
    pub(crate) template: bool,
}
impl TrayIcon {
    pub fn rgba(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self { width, height, rgba: pixels, template: false }
    }

    /// Marks the icon as a macOS template image (`NSImage.isTemplate`).
    ///
    /// Template images use their alpha channel as a mask so macOS tints the glyph for light and
    /// dark menu bars. Ignored on other platforms. Defaults to `false`.
    ///
    /// ```
    /// use rune::TrayIcon;
    ///
    /// let icon = TrayIcon::rgba(32, 32, vec![0; 32 * 32 * 4]).template(true);
    /// # let _ = icon;
    /// ```
    pub fn template(mut self, template: bool) -> Self {
        self.template = template;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrayItem {
    /// `id` comes back in [`TrayEvent::Menu`] when it's picked.
    Action { id: String, label: String, enabled: bool },
    /// A toggleable menu item with a checkmark. `id` comes back in [`TrayEvent::Menu`] when it's
    /// picked; update the checked state by calling [`Tray::set_menu`] with the new items.
    ///
    /// Drawn with a native checkmark on macOS (`NSMenuItem.state`), Windows (`MF_CHECKED`), and
    /// Linux (`com.canonical.dbusmenu` `toggle-type = "checkmark"`, `toggle-state = 0 | 1`).
    Check { id: String, label: String, enabled: bool, checked: bool },
    Separator,
}
impl TrayItem {
    pub fn action(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self::Action { id: id.into(), label: label.into(), enabled: true }
    }

    /// Creates an enabled checkable menu item with initial `checked` state.
    ///
    /// ```
    /// use rune::TrayItem;
    ///
    /// let item = TrayItem::check("hud", "Show HUD", true);
    /// assert!(matches!(item, TrayItem::Check { enabled: true, checked: true, .. }));
    /// ```
    pub fn check(id: impl Into<String>, label: impl Into<String>, checked: bool) -> Self {
        Self::Check { id: id.into(), label: label.into(), enabled: true, checked }
    }

    /// Sets whether an [`Action`](Self::Action) or [`Check`](Self::Check) item can be picked.
    /// Leaves [`Separator`](Self::Separator) unchanged.
    ///
    /// ```
    /// use rune::TrayItem;
    ///
    /// let item = TrayItem::check("hud", "Show HUD", true).enabled(false);
    /// assert!(matches!(item, TrayItem::Check { enabled: false, checked: true, .. }));
    /// ```
    pub fn enabled(self, enabled: bool) -> Self {
        match self {
            Self::Action { id, label, .. } => Self::Action { id, label, enabled },
            Self::Check { id, label, checked, .. } => Self::Check { id, label, enabled, checked },
            Self::Separator => Self::Separator,
        }
    }

    /// Sets the checked state of a menu item, converting [`Action`](Self::Action) into
    /// [`Check`](Self::Check) while keeping its id, label, and enabled state. Leaves
    /// [`Separator`](Self::Separator) unchanged.
    ///
    /// ```
    /// use rune::TrayItem;
    ///
    /// let item = TrayItem::check("mute", "Mute", false).checked(true);
    /// assert!(matches!(item, TrayItem::Check { checked: true, .. }));
    /// ```
    pub fn checked(self, checked: bool) -> Self {
        match self {
            Self::Action { id, label, enabled } | Self::Check { id, label, enabled, .. } => {
                Self::Check { id, label, enabled, checked }
            }
            Self::Separator => Self::Separator,
        }
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

/// Runs on the tray's thread (or the main thread on macOS); keep it short and hand work to the app.
#[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
pub(crate) type Handler = Arc<dyn Fn(TrayEvent) + Send + Sync>;

pub struct Tray {
    #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
    native: native::Native,
}

/// Takes every tray icon off the notification area now, before a fast exit ends the process
/// (`crate::fast_exit`). Elsewhere the icon goes with the process: its D-Bus name (Linux) or
/// status item (macOS) ends when the connection does.
#[cfg(not(target_arch = "wasm32"))]
/// Removes every live tray icon at once without waiting for `Tray::drop` (called by fast exit
/// and by `Windows::exiting` before any background GPU warm-up drain wait so no tray icon
/// stays visible on screen after the user quits). Safe to call before `Tray::drop`.
pub(crate) fn remove_all() {
    #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
    native::remove_all();
}

pub(super) fn validate_icon(icon: &TrayIcon) -> Result<(), TrayError> {
    let expected = usize::try_from(icon.width)
        .ok()
        .and_then(|w| usize::try_from(icon.height).ok()?.checked_mul(w))
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(icon.rgba.len()) {
        return Err(TrayError::Backend("tray icon size doesn't match its pixels".into()));
    }
    Ok(())
}

impl Tray {
    pub fn new(spec: TraySpec, on_event: impl Fn(TrayEvent) + Send + Sync + 'static) -> Result<Self, TrayError> {
        validate_icon(&spec.icon)?;
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
        { native::Native::new(spec, Arc::new(on_event)).map(|native| Self { native }) }
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows")))]
        { let _ = (spec, on_event); Err(TrayError::Unsupported) }
    }
    pub fn set_menu(&self, menu: Vec<TrayItem>) {
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
        self.native.set_menu(menu);
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows")))]
        let _ = menu;
    }
    pub fn set_tooltip(&self, tooltip: impl Into<String>) {
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
        self.native.set_tooltip(tooltip.into());
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows")))]
        let _ = tooltip;
    }

    /// Replaces the tray icon while running, without recreating the tray item.
    ///
    /// Validates that `icon` holds `width × height × 4` RGBA bytes (and non-zero dimensions on
    /// macOS and Windows) before updating the platform backend.
    ///
    /// ```no_run
    /// use rune::{Tray, TrayIcon, TraySpec};
    ///
    /// let icon = TrayIcon::rgba(32, 32, vec![255; 32 * 32 * 4]);
    /// let tray = Tray::new(TraySpec::new("dev.rune.app", "App", icon), |_| {}).unwrap();
    /// let alert = TrayIcon::rgba(32, 32, vec![255, 64, 64, 255].repeat(32 * 32));
    /// tray.set_icon(alert).unwrap();
    /// ```
    pub fn set_icon(&self, icon: TrayIcon) -> Result<(), TrayError> {
        validate_icon(&icon)?;
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
        { self.native.set_icon(icon) }
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows")))]
        { let _ = icon; Ok(()) }
    }

    /// Updates the Freedesktop icon theme name (`IconName`) on Linux StatusNotifierItem hosts
    /// and emits `NewIcon`. Pass an empty string to fall back to the pixel icon set by
    /// [`TraySpec::new`] or [`Tray::set_icon`]. No-op on macOS, Windows, and other platforms.
    ///
    /// ```no_run
    /// use rune::{Tray, TrayIcon, TraySpec};
    ///
    /// let icon = TrayIcon::rgba(16, 16, vec![255; 16 * 16 * 4]);
    /// let tray = Tray::new(TraySpec::new("dev.rune.app", "App", icon), |_| {}).unwrap();
    /// tray.set_icon_name("network-transmit-receive-symbolic");
    /// ```
    pub fn set_icon_name(&self, name: impl Into<String>) {
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
        self.native.set_icon_name(name.into());
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows")))]
        let _ = name;
    }

    /// Updates the tray item's live title text.
    ///
    /// - macOS: sets the `NSStatusBarButton` title beside the icon using the menu-bar font with
    ///   monospaced digits; an empty string returns the button to icon-only.
    /// - Linux: updates the StatusNotifierItem `Title` property and emits `NewTitle`.
    /// - Windows and other platforms: no-op.
    ///
    /// ```no_run
    /// use rune::{Tray, TrayIcon, TraySpec};
    ///
    /// let icon = TrayIcon::rgba(32, 32, vec![255; 32 * 32 * 4]).template(true);
    /// let tray = Tray::new(TraySpec::new("dev.rune.app", "App", icon), |_| {}).unwrap();
    /// tray.set_title("42%");
    /// ```
    pub fn set_title(&self, title: impl Into<String>) {
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
        self.native.set_title(title.into());
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows")))]
        let _ = title;
    }

    /// Holds the tray item's pressed highlight, for apps that open their own popover window
    /// from [`TrayEvent::Activate`] and want the item to look active until it closes.
    ///
    /// - macOS: calls `NSStatusBarButton::highlight` on the main thread (dispatching to the main
    ///   queue off it) and sets it again after [`Tray::set_icon`], [`Tray::set_title`] and the
    ///   context menu, which would otherwise clear it. Called on the main thread, it also applies
    ///   the latest value once more on the next main-queue turn, so a call from the
    ///   [`TrayEvent::Activate`] callback outlasts the button's own unhighlight when its click
    ///   tracking ends.
    /// - Windows, Linux and other platforms: no-op.
    ///
    /// ```no_run
    /// use rune::{Tray, TrayIcon, TraySpec};
    ///
    /// let icon = TrayIcon::rgba(32, 32, vec![255; 32 * 32 * 4]).template(true);
    /// let tray = Tray::new(TraySpec::new("dev.rune.app", "App", icon), |_| {}).unwrap();
    /// tray.set_highlighted(true); // popover opened
    /// tray.set_highlighted(false); // popover closed
    /// ```
    pub fn set_highlighted(&self, highlighted: bool) {
        #[cfg(target_os = "macos")]
        self.native.set_highlighted(highlighted);
        #[cfg(not(target_os = "macos"))]
        let _ = highlighted;
    }

    /// Returns the tray icon's screen bounding rectangle as `[x, y, width, height]` in physical
    /// desktop pixels (top-left origin, matching [`AppHandle::set_position`](crate::AppHandle::set_position)
    /// and [`Display::position`](crate::Display::position)), or `None` when unavailable.
    ///
    /// - macOS: converts the `NSStatusBarButton` frame to physical desktop pixels on the main
    ///   thread (off the main thread, returns the last main-thread reading without blocking).
    /// - Windows: queries `Shell_NotifyIconGetRect`.
    /// - Linux StatusNotifierItem and other platforms: returns `None`.
    ///
    /// ```no_run
    /// use rune::{Tray, TrayIcon, TraySpec};
    ///
    /// let icon = TrayIcon::rgba(32, 32, vec![255; 32 * 32 * 4]);
    /// let tray = Tray::new(TraySpec::new("dev.rune.app", "App", icon), |_| {}).unwrap();
    /// if let Some([x, y, w, h]) = tray.rect() {
    ///     let _ = (x, y, w, h);
    /// }
    /// ```
    pub fn rect(&self) -> Option<[i32; 4]> {
        #[cfg(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows"))]
        { self.native.rect() }
        #[cfg(not(any(all(target_os = "linux", feature = "dbus"), target_os = "macos", target_os = "windows")))]
        { None }
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

#[cfg_attr(not(any(all(target_os = "linux", feature = "dbus"), test)), allow(dead_code))]
pub(super) const DBUSMENU_ROOT: i32 = 0;

#[cfg_attr(not(any(all(target_os = "linux", feature = "dbus"), test)), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum DbusMenuValue {
    StaticStr(&'static str),
    Str(String),
    Bool(bool),
    Int(i32),
}

#[cfg_attr(not(any(all(target_os = "linux", feature = "dbus"), test)), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum DbusMenuKey {
    Action(String, usize),
    Check(String, usize),
    Separator(usize),
}

#[cfg_attr(not(any(all(target_os = "linux", feature = "dbus"), test)), allow(dead_code))]
struct DbusMenuEntry {
    id: i32,
    key: DbusMenuKey,
    item: TrayItem,
}

/// Assigns `com.canonical.dbusmenu` item ids that stay stable for items kept across updates and
/// never reuse an id for another item or kind (KDE Plasma's `libdbusmenu-qt` keeps an existing
/// id's separator/action type across `LayoutUpdated`).
#[cfg_attr(not(any(all(target_os = "linux", feature = "dbus"), test)), allow(dead_code))]
pub(super) struct DbusMenu {
    entries: Vec<DbusMenuEntry>,
    next_id: i32,
    revision: u32,
}

#[cfg_attr(not(any(all(target_os = "linux", feature = "dbus"), test)), allow(dead_code))]
impl DbusMenu {
    pub(super) fn new(items: Vec<TrayItem>) -> Self {
        let mut menu = Self { entries: Vec::new(), next_id: 1, revision: 1 };
        menu.assign(items);
        menu
    }

    pub(super) fn update(&mut self, items: Vec<TrayItem>) -> u32 {
        self.assign(items);
        self.revision = self.revision.wrapping_add(1);
        if self.revision == 0 { self.revision = 1; }
        self.revision
    }

    pub(super) fn revision(&self) -> u32 { self.revision }

    pub(super) fn ids(&self) -> impl Iterator<Item = i32> + '_ {
        self.entries.iter().map(|entry| entry.id)
    }

    pub(super) fn action_id(&self, id: i32) -> Option<&str> {
        if id <= DBUSMENU_ROOT { return None; }
        match &self.entries.iter().find(|entry| entry.id == id)?.item {
            TrayItem::Action { id, enabled: true, .. }
            | TrayItem::Check { id, enabled: true, .. } => Some(id.as_str()),
            _ => None,
        }
    }

    pub(super) fn properties(&self, id: i32) -> Vec<(&'static str, DbusMenuValue)> {
        if id == DBUSMENU_ROOT {
            return vec![("children-display", DbusMenuValue::StaticStr("submenu"))];
        }
        if id < DBUSMENU_ROOT { return Vec::new(); }
        match self.entries.iter().find(|entry| entry.id == id).map(|entry| &entry.item) {
            Some(TrayItem::Action { label, enabled, .. }) => vec![
                ("type", DbusMenuValue::StaticStr("standard")),
                ("label", DbusMenuValue::Str(dbusmenu_label(label))),
                ("enabled", DbusMenuValue::Bool(*enabled)),
            ],
            Some(TrayItem::Check { label, enabled, checked, .. }) => vec![
                ("type", DbusMenuValue::StaticStr("standard")),
                ("label", DbusMenuValue::Str(dbusmenu_label(label))),
                ("enabled", DbusMenuValue::Bool(*enabled)),
                ("toggle-type", DbusMenuValue::StaticStr("checkmark")),
                ("toggle-state", DbusMenuValue::Int(if *checked { 1 } else { 0 })),
            ],
            Some(TrayItem::Separator) => vec![("type", DbusMenuValue::StaticStr("separator"))],
            None => Vec::new(),
        }
    }

    fn assign(&mut self, items: Vec<TrayItem>) {
        let mut previous: std::collections::HashMap<DbusMenuKey, i32> =
            self.entries.drain(..).map(|entry| (entry.key, entry.id)).collect();
        let mut action_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut check_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut separators = 0usize;
        let mut assigned_ids: Vec<i32> = Vec::with_capacity(items.len());
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            let key = match &item {
                TrayItem::Action { id, .. } => {
                    let count = action_counts.entry(id.clone()).or_insert(0);
                    let key = DbusMenuKey::Action(id.clone(), *count);
                    *count += 1;
                    key
                }
                TrayItem::Check { id, .. } => {
                    let count = check_counts.entry(id.clone()).or_insert(0);
                    let key = DbusMenuKey::Check(id.clone(), *count);
                    *count += 1;
                    key
                }
                TrayItem::Separator => {
                    let key = DbusMenuKey::Separator(separators);
                    separators += 1;
                    key
                }
            };
            let id = match previous.remove(&key) {
                Some(existing) => existing,
                None => self.alloc_id(&previous, &assigned_ids),
            };
            assigned_ids.push(id);
            entries.push(DbusMenuEntry { id, key, item });
        }
        self.entries = entries;
    }

    fn alloc_id(&mut self, previous: &std::collections::HashMap<DbusMenuKey, i32>, assigned: &[i32]) -> i32 {
        loop {
            let id = self.next_id;
            self.next_id = if id == i32::MAX { 1 } else { id + 1 };
            if !assigned.contains(&id) && !previous.values().any(|&old| old == id) {
                return id;
            }
        }
    }
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

/// Computes the logical point size `(width, height)` for a macOS status-bar icon of pixel
/// dimensions `width × height` so its height matches `bar_thickness` while preserving aspect
/// ratio. Returns `None` for zero dimensions or non-positive/non-finite bar thickness.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(super) fn macos_icon_point_size(width: u32, height: u32, bar_thickness: f64) -> Option<(f64, f64)> {
    if width == 0 || height == 0 || !bar_thickness.is_finite() || bar_thickness <= 0.0 {
        return None;
    }
    let pt_w = bar_thickness * (f64::from(width) / f64::from(height));
    pt_w.is_finite().then_some((pt_w, bar_thickness))
}

/// Converts a row of straight RGBA pixels into premultiplied RGBA pixels.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(super) fn premultiply_rgba(straight_rgba: &[u8], out: &mut [u8]) {
    for (dst, src) in out.chunks_exact_mut(4).zip(straight_rgba.chunks_exact(4)) {
        let a = u32::from(src[3]);
        dst[0] = ((u32::from(src[0]) * a + 127) / 255) as u8;
        dst[1] = ((u32::from(src[1]) * a + 127) / 255) as u8;
        dst[2] = ((u32::from(src[2]) * a + 127) / 255) as u8;
        dst[3] = src[3];
    }
}

/// What a macOS status-bar click should trigger.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MacClickAction {
    Activate,
    ShowMenu,
}

/// Primary click activates; secondary click or Control-click opens the tray menu.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(super) fn classify_mac_click(is_right_button: bool, control_modifier: bool) -> MacClickAction {
    if is_right_button || control_modifier {
        MacClickAction::ShowMenu
    } else {
        MacClickAction::Activate
    }
}

/// Converts an AppKit bottom-up screen rectangle in logical points to top-down physical desktop
/// pixels `[x, y, width, height]` matching winit's `PhysicalPosition` / `AppHandle::set_position`.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(super) fn macos_screen_rect_to_physical(
    origin_x: f64,
    origin_y: f64,
    width: f64,
    height: f64,
    primary_screen_height: f64,
    scale: f64,
) -> Option<[i32; 4]> {
    if ![origin_x, origin_y, width, height, primary_screen_height, scale]
        .into_iter()
        .all(f64::is_finite)
        || width <= 0.0
        || height <= 0.0
        || primary_screen_height <= 0.0
        || scale <= 0.0
    {
        return None;
    }
    let top_y = primary_screen_height - origin_y - height;
    let x = (origin_x * scale).round();
    let y = (top_y * scale).round();
    let w = (width * scale).round();
    let h = (height * scale).round();
    if x < f64::from(i32::MIN)
        || x > f64::from(i32::MAX)
        || y < f64::from(i32::MIN)
        || y > f64::from(i32::MAX)
        || w <= 0.0
        || w > f64::from(i32::MAX)
        || h <= 0.0
        || h > f64::from(i32::MAX)
    {
        return None;
    }
    Some([x as i32, y as i32, w as i32, h as i32])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Tray>();
    }

    #[test]
    fn tray_icon_template_defaults_false_and_toggles() {
        let plain = TrayIcon::rgba(2, 2, vec![0; 16]);
        assert!(!plain.template);
        let templated = plain.clone().template(true);
        assert!(templated.template);
        assert_ne!(plain, templated);
        assert_eq!(templated.template(false), plain);
    }

    #[test]
    fn macos_icon_point_size_preserves_aspect_ratio_and_rejects_invalid() {
        assert_eq!(macos_icon_point_size(32, 32, 22.0), Some((22.0, 22.0)));
        assert_eq!(macos_icon_point_size(44, 44, 22.0), Some((22.0, 22.0)));
        assert_eq!(macos_icon_point_size(64, 32, 22.0), Some((44.0, 22.0)));
        assert_eq!(macos_icon_point_size(16, 32, 22.0), Some((11.0, 22.0)));
        assert_eq!(macos_icon_point_size(48, 24, 24.0), Some((48.0, 24.0)));
        assert_eq!(macos_icon_point_size(0, 32, 22.0), None);
        assert_eq!(macos_icon_point_size(32, 0, 22.0), None);
        assert_eq!(macos_icon_point_size(0, 0, 22.0), None);
        assert_eq!(macos_icon_point_size(32, 32, 0.0), None);
        assert_eq!(macos_icon_point_size(32, 32, -22.0), None);
        assert_eq!(macos_icon_point_size(32, 32, f64::NAN), None);
        assert_eq!(macos_icon_point_size(32, 32, f64::INFINITY), None);
    }

    #[test]
    fn premultiply_rgba_scales_rgb_and_preserves_alpha() {
        let src = [
            255, 128, 64, 255, // opaque
            255, 128, 64, 0,   // transparent -> RGB zeroed
            200, 100, 50, 128, // ~50% alpha
        ];
        let mut dst = [0u8; 12];
        premultiply_rgba(&src, &mut dst);
        assert_eq!(dst, [
            255, 128, 64, 255,
            0, 0, 0, 0,
            100, 50, 25, 128,
        ]);
    }

    #[test]
    fn mac_click_classification_splits_primary_and_secondary_or_control() {
        assert_eq!(classify_mac_click(false, false), MacClickAction::Activate);
        assert_eq!(classify_mac_click(true, false), MacClickAction::ShowMenu);
        assert_eq!(classify_mac_click(false, true), MacClickAction::ShowMenu);
        assert_eq!(classify_mac_click(true, true), MacClickAction::ShowMenu);
    }

    #[test]
    fn enabled_and_checked_handle_action_check_and_separator() {
        assert_eq!(TrayItem::Separator.enabled(false), TrayItem::Separator);
        assert_eq!(TrayItem::Separator.checked(true), TrayItem::Separator);
        assert_eq!(
            TrayItem::action("q", "Quit").enabled(false),
            TrayItem::Action { id: "q".into(), label: "Quit".into(), enabled: false }
        );
        assert_eq!(
            TrayItem::check("hud", "Show HUD", false),
            TrayItem::Check { id: "hud".into(), label: "Show HUD".into(), enabled: true, checked: false }
        );
        assert_eq!(
            TrayItem::check("hud", "Show HUD", false).checked(true).enabled(false),
            TrayItem::Check { id: "hud".into(), label: "Show HUD".into(), enabled: false, checked: true }
        );
        assert_eq!(
            TrayItem::action("mute", "Mute").enabled(false).checked(true),
            TrayItem::Check { id: "mute".into(), label: "Mute".into(), enabled: false, checked: true }
        );
    }

    #[test]
    fn validate_icon_checks_pixel_count_and_overflow() {
        assert!(validate_icon(&TrayIcon::rgba(2, 2, vec![0; 16])).is_ok());
        assert!(validate_icon(&TrayIcon::rgba(0, 0, Vec::new())).is_ok());
        assert!(matches!(validate_icon(&TrayIcon::rgba(2, 2, vec![0; 15])), Err(TrayError::Backend(_))));
        assert!(matches!(validate_icon(&TrayIcon::rgba(2, 2, vec![0; 20])), Err(TrayError::Backend(_))));
        assert!(matches!(validate_icon(&TrayIcon::rgba(u32::MAX, 2, Vec::new())), Err(TrayError::Backend(_))));
    }

    #[test]
    fn macos_screen_rect_to_physical_flips_y_scales_and_rejects_invalid() {
        // Retina 2x primary display (height 982 pt); menu bar at top: AppKit y = 982 - 24 = 958.
        assert_eq!(
            macos_screen_rect_to_physical(1200.0, 958.0, 28.0, 24.0, 982.0, 2.0),
            Some([2400, 0, 56, 48])
        );
        // Secondary display to the left of primary (negative x and negative top_y when taller).
        assert_eq!(
            macos_screen_rect_to_physical(-200.0, 1056.0, 28.0, 24.0, 982.0, 1.0),
            Some([-200, -98, 28, 24])
        );
        // Invalid / zero / negative / non-finite dimensions return None.
        assert_eq!(macos_screen_rect_to_physical(0.0, 0.0, 0.0, 24.0, 982.0, 2.0), None);
        assert_eq!(macos_screen_rect_to_physical(0.0, 0.0, 28.0, -1.0, 982.0, 2.0), None);
        assert_eq!(macos_screen_rect_to_physical(0.0, 0.0, 28.0, 24.0, 0.0, 2.0), None);
        assert_eq!(macos_screen_rect_to_physical(0.0, 0.0, 28.0, 24.0, 982.0, 0.0), None);
        assert_eq!(macos_screen_rect_to_physical(f64::NAN, 0.0, 28.0, 24.0, 982.0, 2.0), None);
        assert_eq!(macos_screen_rect_to_physical(0.0, f64::INFINITY, 28.0, 24.0, 982.0, 2.0), None);
    }

    #[test]
    fn a_short_icon_is_refused() {
        let spec = TraySpec::new("t", "T", TrayIcon::rgba(2, 2, vec![0; 3]));
        assert!(matches!(Tray::new(spec, |_| {}), Err(TrayError::Backend(_))));
    }

    #[test]
    fn overflowing_icon_dimensions_are_refused() {
        for (width, height) in [(1u32 << 31, 1u32 << 31), (u32::MAX, u32::MAX), (u32::MAX, 2)] {
            let spec = TraySpec::new("t", "T", TrayIcon::rgba(width, height, Vec::new()));
            assert!(matches!(Tray::new(spec, |_| {}), Err(TrayError::Backend(_))));
        }
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

    #[test]
    fn dbusmenu_ids_stay_stable_and_never_switch_kind_across_updates() {
        let mut menu = DbusMenu::new(vec![
            TrayItem::action("show", "Show"),
            TrayItem::Separator,
            TrayItem::action("quit", "Quit"),
        ]);
        assert_eq!(menu.revision(), 1);
        let initial_ids: Vec<i32> = menu.ids().collect();
        assert_eq!(initial_ids.len(), 3);
        let (show_id, sep_id, quit_id) = (initial_ids[0], initial_ids[1], initial_ids[2]);

        // Inserting an action before the separator must not reuse the separator's id for the new
        // action or the quit action's id for the separator.
        let rev = menu.update(vec![
            TrayItem::action("show", "Show"),
            TrayItem::action("disconnect", "Disconnect"),
            TrayItem::Separator,
            TrayItem::action("quit", "Quit"),
        ]);
        assert_eq!(rev, 2);
        assert_eq!(menu.revision(), 2);
        let updated_ids: Vec<i32> = menu.ids().collect();
        assert_eq!(updated_ids.len(), 4);
        let disconnect_id = updated_ids[1];
        assert_eq!(updated_ids[0], show_id);
        assert_eq!(updated_ids[2], sep_id);
        assert_eq!(updated_ids[3], quit_id);
        assert!(!initial_ids.contains(&disconnect_id));
        assert_eq!(menu.action_id(disconnect_id), Some("disconnect"));
        assert_eq!(menu.action_id(sep_id), None);
        assert_eq!(menu.action_id(quit_id), Some("quit"));

        // Removing the action keeps the remaining items' ids and retires the removed id.
        menu.update(vec![
            TrayItem::action("show", "Show"),
            TrayItem::Separator,
            TrayItem::action("quit", "Quit"),
        ]);
        assert_eq!(menu.ids().collect::<Vec<_>>(), vec![show_id, sep_id, quit_id]);
        assert_eq!(menu.action_id(disconnect_id), None);
        assert!(menu.properties(disconnect_id).is_empty());

        // Replacing a separator with an action at the same slot allocates a fresh id.
        menu.update(vec![
            TrayItem::action("show", "Show"),
            TrayItem::action("about", "About"),
            TrayItem::action("quit", "Quit"),
        ]);
        let replaced_ids: Vec<i32> = menu.ids().collect();
        assert_eq!(replaced_ids[0], show_id);
        assert_ne!(replaced_ids[1], sep_id);
        assert_ne!(replaced_ids[1], disconnect_id);
        assert_eq!(replaced_ids[2], quit_id);
    }

    #[test]
    fn dbusmenu_actions_emit_explicit_type_and_enabled_properties() {
        let mut menu = DbusMenu::new(vec![
            TrayItem::action("show", "Show_Now"),
            TrayItem::Separator,
            TrayItem::action("quit", "Quit").enabled(false),
        ]);
        let ids: Vec<i32> = menu.ids().collect();
        let (show_id, sep_id, quit_id) = (ids[0], ids[1], ids[2]);

        assert_eq!(menu.properties(DBUSMENU_ROOT), vec![
            ("children-display", DbusMenuValue::StaticStr("submenu")),
        ]);
        assert_eq!(menu.properties(show_id), vec![
            ("type", DbusMenuValue::StaticStr("standard")),
            ("label", DbusMenuValue::Str("Show__Now".into())),
            ("enabled", DbusMenuValue::Bool(true)),
        ]);
        assert_eq!(menu.properties(sep_id), vec![
            ("type", DbusMenuValue::StaticStr("separator")),
        ]);
        assert_eq!(menu.properties(quit_id), vec![
            ("type", DbusMenuValue::StaticStr("standard")),
            ("label", DbusMenuValue::Str("Quit".into())),
            ("enabled", DbusMenuValue::Bool(false)),
        ]);
        assert_eq!(menu.action_id(quit_id), None);

        // Re-enabling a previously disabled item must explicitly emit enabled = true.
        menu.update(vec![
            TrayItem::action("show", "Show_Now"),
            TrayItem::Separator,
            TrayItem::action("quit", "Quit").enabled(true),
        ]);
        assert_eq!(menu.properties(quit_id), vec![
            ("type", DbusMenuValue::StaticStr("standard")),
            ("label", DbusMenuValue::Str("Quit".into())),
            ("enabled", DbusMenuValue::Bool(true)),
        ]);
        assert_eq!(menu.action_id(quit_id), Some("quit"));
    }

    #[test]
    fn dbusmenu_duplicate_action_ids_and_multiple_separators_get_distinct_ids() {
        let mut menu = DbusMenu::new(vec![
            TrayItem::action("dup", "First"),
            TrayItem::Separator,
            TrayItem::action("dup", "Second"),
            TrayItem::Separator,
        ]);
        let ids: Vec<i32> = menu.ids().collect();
        assert_eq!(ids.len(), 4);
        assert_ne!(ids[0], ids[2]);
        assert_ne!(ids[1], ids[3]);
        assert!(ids.iter().all(|&id| id > DBUSMENU_ROOT));

        // Re-adding a removed action gives it a fresh id instead of reusing a retired one.
        let removed_sep = ids[3];
        menu.update(vec![
            TrayItem::action("dup", "First"),
            TrayItem::Separator,
            TrayItem::action("dup", "Second"),
        ]);
        assert!(menu.properties(removed_sep).is_empty());
        menu.update(vec![
            TrayItem::action("dup", "First"),
            TrayItem::Separator,
            TrayItem::action("dup", "Second"),
            TrayItem::Separator,
        ]);
        let ids_after: Vec<i32> = menu.ids().collect();
        assert_eq!(&ids_after[..3], &ids[..3]);
        assert_ne!(ids_after[3], removed_sep);
    }

    #[test]
    fn dbusmenu_check_items_emit_toggle_properties_keep_ids_and_never_reuse_action_ids() {
        let mut menu = DbusMenu::new(vec![
            TrayItem::action("hud", "HUD"),
            TrayItem::check("mute", "Mute_Audio", false),
            TrayItem::check("locked", "Locked", true).enabled(false),
        ]);
        let ids: Vec<i32> = menu.ids().collect();
        let (hud_action_id, mute_id, locked_id) = (ids[0], ids[1], ids[2]);

        assert_eq!(menu.properties(mute_id), vec![
            ("type", DbusMenuValue::StaticStr("standard")),
            ("label", DbusMenuValue::Str("Mute__Audio".into())),
            ("enabled", DbusMenuValue::Bool(true)),
            ("toggle-type", DbusMenuValue::StaticStr("checkmark")),
            ("toggle-state", DbusMenuValue::Int(0)),
        ]);
        assert_eq!(menu.properties(locked_id), vec![
            ("type", DbusMenuValue::StaticStr("standard")),
            ("label", DbusMenuValue::Str("Locked".into())),
            ("enabled", DbusMenuValue::Bool(false)),
            ("toggle-type", DbusMenuValue::StaticStr("checkmark")),
            ("toggle-state", DbusMenuValue::Int(1)),
        ]);
        assert_eq!(menu.action_id(mute_id), Some("mute"));
        assert_eq!(menu.action_id(locked_id), None);

        // Flipping checked keeps the Check item's id; replacing Action("hud") with Check("hud")
        // allocates a fresh id so libdbusmenu-qt never reuses an Action id as a Check id.
        menu.update(vec![
            TrayItem::check("hud", "HUD", true),
            TrayItem::check("mute", "Mute_Audio", true),
            TrayItem::check("locked", "Locked", true).enabled(false),
        ]);
        let updated: Vec<i32> = menu.ids().collect();
        assert_ne!(updated[0], hud_action_id);
        assert_eq!(updated[1], mute_id);
        assert_eq!(updated[2], locked_id);
        assert_eq!(menu.action_id(updated[0]), Some("hud"));
        assert_eq!(menu.properties(mute_id), vec![
            ("type", DbusMenuValue::StaticStr("standard")),
            ("label", DbusMenuValue::Str("Mute__Audio".into())),
            ("enabled", DbusMenuValue::Bool(true)),
            ("toggle-type", DbusMenuValue::StaticStr("checkmark")),
            ("toggle-state", DbusMenuValue::Int(1)),
        ]);
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
