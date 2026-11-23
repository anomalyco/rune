//! System-wide shortcuts. Native registrations belong to the app's event-loop thread.
//! Nothing connects to the desktop until the first registration.
mod parse;
mod keys;
#[cfg(target_os = "macos")]
mod layout_macos;
#[cfg(all(test, target_os = "macos"))]
pub(crate) use layout_macos::force as force_layout;
pub use keys::{PhysicalKeyCode, modifier_keycaps};
pub(crate) use keys::us_char;
mod service;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "linux")]
mod x11;
#[cfg(any(target_os = "linux", test))]
mod x11_repeat;
#[cfg(all(target_os = "linux", feature = "dbus"))]
mod wayland;

pub use parse::Shortcut;
pub(crate) use service::{Guard, Hub, HubCore};
use std::sync::Arc;

/// An opaque registration, unique within this process. It is not an accelerator hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ShortcutId(u32);

/// Registration failures callers can act on without inspecting platform error strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutError {
    Conflict,
    Unsupported,
    InvalidAccelerator(String),
    /// Runtime registration requires a running app. Static builders work before run().
    NotRunning,
    UnknownId,
    /// The user declined the portal's shortcut permission dialog.
    PermissionDenied,
    Backend(String),
}

impl std::fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => f.write_str("shortcut is already registered"),
            Self::Unsupported => f.write_str("global shortcuts are unsupported in this desktop session"),
            Self::InvalidAccelerator(s) => write!(f, "invalid accelerator: {s}"),
            Self::NotRunning => f.write_str("the shortcut's app is not running"),
            Self::UnknownId => f.write_str("unknown shortcut registration"),
            Self::PermissionDenied => f.write_str("shortcut permission was declined"),
            Self::Backend(s) => write!(f, "shortcut backend: {s}"),
        }
    }
}
impl std::error::Error for ShortcutError {}

type Sink = Arc<dyn Fn(ShortcutId, Option<String>) + Send + Sync>;
trait Backend {
    fn register(&mut self, id: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError>;
    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError>;
}

#[cfg(target_os = "linux")]
type NativeBackend = Box<dyn Backend + Send>;
#[cfg(not(target_os = "linux"))]
type NativeBackend = Box<dyn Backend>;

/// Linux session environment used to choose between the GlobalShortcuts portal and X11 grabs.
#[cfg(any(test, target_os = "linux"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinuxEnv<'a> {
    /// Whether the app's own event loop is connected to Wayland (false on X11 and XWayland).
    wayland: bool,
    session_type: Option<&'a str>,
    wayland_display: Option<&'a str>,
    display: Option<&'a str>,
    dbus: bool,
}

#[cfg(any(test, target_os = "linux"))]
impl LinuxEnv<'_> {
    /// True when the desktop session is Wayland, including an XWayland app whose own event loop
    /// is X11. Explicit `XDG_SESSION_TYPE=x11` keeps X11 even if a stale `WAYLAND_DISPLAY` is
    /// present; an unset or other session type falls back to a non-empty `WAYLAND_DISPLAY`.
    fn wayland_session(self) -> bool {
        if self.wayland { return true; }
        match self.session_type.map(str::trim) {
            Some(t) if t.eq_ignore_ascii_case("wayland") => true,
            Some(t) if t.eq_ignore_ascii_case("x11") => false,
            _ => self.wayland_display.is_some_and(|d| !d.trim().is_empty()),
        }
    }

    fn has_x11_display(self) -> bool {
        self.display.is_some_and(|d| !d.trim().is_empty())
    }
}

/// Chooses and opens the Linux global shortcuts backend for `env`. Native Wayland apps require
/// the GlobalShortcuts portal; an XWayland app in a Wayland session tries the portal first so
/// shortcuts fire while native Wayland windows have focus, falling back to X11 grabs when the
/// portal is unavailable or `dbus` is disabled. Real X11 sessions open X11 directly.
#[cfg(any(test, target_os = "linux"))]
fn open_linux_backend<T>(
    env: LinuxEnv<'_>,
    open_portal: impl FnOnce() -> Result<T, ShortcutError>,
    open_x11: impl FnOnce() -> Result<T, ShortcutError>,
) -> Result<T, ShortcutError> {
    if env.wayland_session() {
        if env.dbus {
            match open_portal() {
                Ok(portal) => return Ok(portal),
                Err(error) if env.wayland || !env.has_x11_display() => return Err(error),
                Err(_) => {}
            }
        } else if env.wayland {
            return Err(ShortcutError::Unsupported);
        }
    }
    if env.has_x11_display() { open_x11() } else { Err(ShortcutError::Unsupported) }
}

#[cfg(target_os = "linux")]
fn linux_backend(wayland: bool, sink: Sink) -> Result<NativeBackend, ShortcutError> {
    let session_type = std::env::var("XDG_SESSION_TYPE").ok();
    let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
    let display = std::env::var("DISPLAY").ok();
    let env = LinuxEnv {
        wayland,
        session_type: session_type.as_deref(),
        wayland_display: wayland_display.as_deref(),
        display: display.as_deref(),
        dbus: cfg!(feature = "dbus"),
    };
    open_linux_backend(
        env,
        || {
            #[cfg(feature = "dbus")]
            { Ok(Box::new(wayland::Native::new(sink.clone())?) as NativeBackend) }
            #[cfg(not(feature = "dbus"))]
            { Err(ShortcutError::Unsupported) }
        },
        || Ok(Box::new(x11::Native::new(sink.clone())?) as NativeBackend),
    )
}

fn backend(wayland: bool, sink: Sink) -> Result<NativeBackend, ShortcutError> {
    let _ = wayland;
    #[cfg(target_os = "macos")]
    return Ok(Box::new(macos::Native::new(sink)?));
    #[cfg(target_os = "windows")]
    return Ok(Box::new(windows::Native::new(sink)?));
    #[cfg(target_os = "linux")]
    return linux_backend(wayland, sink);
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    { let _ = sink; Err(ShortcutError::Unsupported) }
}

/// The desktop's own claim on a chord. Only macOS publishes one (its symbolic hotkeys).
fn system_owner(shortcut: &Shortcut) -> Option<String> {
    #[cfg(target_os = "macos")]
    return macos::system_owner(shortcut);
    #[cfg(not(target_os = "macos"))]
    { let _ = shortcut; None }
}

// Keep native status mapping pure so tests can inject failures without grabbing the desktop.
#[cfg(any(test, target_os = "macos", target_os = "windows"))]
fn native_status(code: i64, conflict: i64) -> Result<(), ShortcutError> {
    match code {
        0 => Ok(()),
        c if c == conflict => Err(ShortcutError::Conflict),
        c => Err(ShortcutError::Backend(format!("native status {c}"))),
    }
}

impl<S: 'static> crate::App<S> {
    /// Register when the app starts, on the same thread and state as its windows.
    /// Errors are reported once to stderr and retained by `AppHandle::take_shortcut_errors`.
    /// Headless scripts register virtually, without connecting to the desktop.
    pub fn global_shortcut(self, accelerator: &str, handler: fn(&mut S, &mut crate::Tx)) -> Self {
        self.shared.shortcuts.pending(accelerator.parse(), Arc::new(handler));
        self
    }

    /// Let `global_shortcut` and `register_shortcut` take a chord without ⌘, ⌃ or ⌥ (Ctrl, Alt
    /// or Super elsewhere), such as Space or ⇧A, which then can't be typed in any app. By
    /// default those fail with `InvalidAccelerator`; F1–F24 are always allowed alone.
    pub fn allow_unmodified_shortcuts(self) -> Self {
        self.shared.shortcuts.allow_unmodified();
        self
    }
}

impl<S: 'static> crate::AppHandle<S> {
    /// Register a shortcut and return the actual backend result. On the app thread this is
    /// immediate; on macOS/Windows other threads wait for the loop to process the request.
    /// Do not join such callers from the app thread. Linux registrations can run off-thread;
    /// use a worker for Wayland consent dialogs to keep the UI responsive. Portal consent is
    /// bounded to two minutes and the user may choose a different trigger.
    pub fn register_shortcut(&self, shortcut: Shortcut, handler: impl Fn(&mut S, &mut crate::Tx) + Send + Sync + 'static) -> Result<ShortcutId, ShortcutError> {
        self.shared.shortcuts.register(shortcut, Arc::new(handler))
    }

    /// Release a registration owned by this app. Already queued activations become no-ops.
    pub fn unregister_shortcut(&self, id: ShortcutId) -> Result<(), ShortcutError> {
        self.shared.shortcuts.unregister(id)
    }

    /// Who holds `shortcut`, when Rune or the desktop can say: this App or another Rune App in
    /// the process (its title), or on macOS a system shortcut ("Spotlight", "Mission Control",
    /// "macOS" for others). None when unknown, including other apps' Carbon, Win32, X11 and
    /// portal grabs, before the app runs and for chords headless runs didn't register.
    /// Ask after `ShortcutError::Conflict` to name the owner in a message.
    pub fn shortcut_owner(&self, shortcut: &Shortcut) -> Option<String> {
        self.shared.shortcuts.owner(shortcut)
    }

    /// Drain startup registration failures. Runtime methods return their errors directly.
    pub fn take_shortcut_errors(&self) -> Vec<ShortcutError> {
        self.shared.shortcuts.errors()
    }
}

impl crate::Tx {
    /// Wayland's compositor-issued token for this shortcut activation. Forward it to
    /// `AppHandle::activate(token)` to show and focus the quick panel. None on other backends.
    pub fn activation_token(&self) -> Option<&str> { self.shortcut_activation_token.as_deref() }
}

#[cfg(test)]
mod tests;
