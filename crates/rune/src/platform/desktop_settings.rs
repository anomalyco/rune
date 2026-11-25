//! Any key of the desktop's settings, for an app to follow (`rune::desktop_setting`): on Linux the
//! XDG Settings portal (`org.freedesktop.portal.Settings`). The appearance portal's worker
//! (`portal_appearance`) reads each followed key between the `SettingChanged` signals it already
//! follows, so a read never lands after a newer signal, and passes every value here; a dispatch
//! thread of its own runs the app's callbacks, so a slow or panicking one doesn't hold up Rune's
//! own keys (`color-scheme`, reduced motion, the double-click interval). This is for the rest,
//! such as `org.gnome.desktop.interface` `text-scaling-factor` or `cursor-blink-time`.

/// A value from the desktop's settings ([`desktop_setting`]). Values of other D-Bus types
/// (structures, arrays) are not delivered.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DesktopSetting {
    Bool(bool),
    /// A signed integer (`n`, `i`, `x`).
    Int(i64),
    /// An unsigned integer (`y`, `q`, `u`, `t`).
    Uint(u64),
    Double(f64),
    Str(String),
}

impl DesktopSetting {
    /// The value as a `bool`, when it is one.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The value as an `i64`, when it is an integer that fits.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(x) => Some(*x),
            Self::Uint(x) => i64::try_from(*x).ok(),
            _ => None,
        }
    }

    /// The value as an `f64`, when it is a number.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(x) => Some(*x as f64),
            Self::Uint(x) => Some(*x as f64),
            Self::Double(x) => Some(*x),
            _ => None,
        }
    }

    /// The value as a string, when it is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }
}

/// Follows one key of the desktop's settings while the returned [`DesktopSettingWatch`] lives:
/// `on_change` gets its value once the desktop answers, then each new value when it changes. On
/// Linux this is the XDG Settings portal's `namespace` and `key` (`"org.gnome.desktop.interface"`,
/// `"text-scaling-factor"`), which sandboxed apps can read too. Call it before or after `App::run`.
///
/// `on_change` runs on a thread of its own, shared by every followed key, in the order the values
/// came, never twice in a row with the same value: hand the value to the UI with
/// `AppHandle::update`. A callback that panics is dropped. It is never called when no portal
/// answers the key, nor on macOS, Windows, the web, or Linux without the `dbus` feature.
///
/// ```no_run
/// use rune::{App, AppSpec, el};
///
/// struct S { text_scale: f64 }
/// let app = App::new(AppSpec::new("Notes", (480.0, 360.0), || S { text_scale: 1.0 }, |s: &S, _| {
///     let _ = s.text_scale;
///     el()
/// }));
/// let handle = app.handle();
/// let _text_scale = rune::desktop_setting("org.gnome.desktop.interface", "text-scaling-factor", move |v| {
///     if let Some(scale) = v.as_f64() { handle.update(move |s, _| s.text_scale = scale); }
/// });
/// app.run();
/// ```
pub fn desktop_setting(
    namespace: &str,
    key: &str,
    on_change: impl FnMut(DesktopSetting) + Send + 'static,
) -> DesktopSettingWatch {
    #[cfg(all(target_os = "linux", feature = "dbus"))]
    let id = dbus::watch(namespace, key, Box::new(on_change));
    #[cfg(not(all(target_os = "linux", feature = "dbus")))]
    let id = {
        let _ = (namespace, key, on_change);
        0
    };
    DesktopSettingWatch { id }
}

/// Following a key of the desktop's settings ([`desktop_setting`]); dropping it stops.
#[must_use = "dropping the watch stops following the key"]
#[derive(Debug)]
pub struct DesktopSettingWatch {
    #[cfg_attr(not(all(target_os = "linux", feature = "dbus")), allow(dead_code))]
    id: u64,
}

impl Drop for DesktopSettingWatch {
    fn drop(&mut self) {
        #[cfg(all(target_os = "linux", feature = "dbus"))]
        dbus::unwatch(self.id);
    }
}

/// One followed key and what its callback was last given.
#[cfg(any(test, all(target_os = "linux", feature = "dbus")))]
struct Watch {
    namespace: String,
    key: String,
    on_change: Box<dyn FnMut(DesktopSetting) + Send>,
    last: Option<DesktopSetting>,
}

#[cfg(any(test, all(target_os = "linux", feature = "dbus")))]
impl Watch {
    /// Gives `on_change` `value` when the watch is for `namespace`/`key` and `value` is new.
    /// Returns false when the callback panicked, and the watch should go.
    fn offer(&mut self, namespace: &str, key: &str, value: &DesktopSetting) -> bool {
        if self.namespace != namespace || self.key != key || self.last.as_ref() == Some(value) { return true; }
        self.last = Some(value.clone());
        let on_change = &mut self.on_change;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| on_change(value.clone()))).is_ok()
    }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
mod dbus {
    use std::collections::HashMap;
    use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}, mpsc};

    use zbus::zvariant::Value;

    use super::{DesktopSetting, Watch};

    /// What the dispatch thread hears, in order.
    enum Dispatch {
        Add(u64, Watch),
        Remove(u64),
        Value { namespace: String, key: String, value: DesktopSetting },
    }

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    /// The dispatch thread, started by the first `desktop_setting`; it owns every watch.
    static DISPATCH: OnceLock<Option<mpsc::Sender<Dispatch>>> = OnceLock::new();
    /// Keys the worker is to read, in the order they were followed.
    static READS: OnceLock<(async_channel::Sender<(String, String)>, async_channel::Receiver<(String, String)>)> =
        OnceLock::new();

    fn reads_channel() -> &'static (async_channel::Sender<(String, String)>, async_channel::Receiver<(String, String)>) {
        READS.get_or_init(async_channel::unbounded)
    }

    /// The keys followed so far and from now on, for the worker to read.
    pub(crate) fn reads() -> async_channel::Receiver<(String, String)> {
        reads_channel().1.clone()
    }

    fn dispatch() -> Option<&'static mpsc::Sender<Dispatch>> {
        DISPATCH.get_or_init(|| {
            let (send, receive) = mpsc::channel();
            std::thread::Builder::new()
                .name("rune-desktop-settings".into())
                .spawn(move || run(&receive))
                .ok()
                .map(|_| send)
        }).as_ref()
    }

    fn run(receive: &mpsc::Receiver<Dispatch>) {
        let mut watches: HashMap<u64, Watch> = HashMap::new();
        while let Ok(message) = receive.recv() {
            match message {
                Dispatch::Add(id, watch) => { watches.insert(id, watch); }
                Dispatch::Remove(id) => { watches.remove(&id); }
                Dispatch::Value { namespace, key, value } => {
                    watches.retain(|_, watch| watch.offer(&namespace, &key, &value));
                }
            }
        }
    }

    /// Decodes a portal value (`Read`, before `ReadOne`, wraps it twice).
    pub(crate) fn decode(value: &Value<'_>) -> Option<DesktopSetting> {
        Some(match value {
            Value::Bool(b) => DesktopSetting::Bool(*b),
            Value::I16(x) => DesktopSetting::Int(i64::from(*x)),
            Value::I32(x) => DesktopSetting::Int(i64::from(*x)),
            Value::I64(x) => DesktopSetting::Int(*x),
            Value::U8(x) => DesktopSetting::Uint(u64::from(*x)),
            Value::U16(x) => DesktopSetting::Uint(u64::from(*x)),
            Value::U32(x) => DesktopSetting::Uint(u64::from(*x)),
            Value::U64(x) => DesktopSetting::Uint(*x),
            Value::F64(x) => DesktopSetting::Double(*x),
            Value::Str(s) => DesktopSetting::Str(s.to_string()),
            Value::Value(inner) => return decode(inner),
            _ => return None,
        })
    }

    pub(super) fn watch(namespace: &str, key: &str, on_change: Box<dyn FnMut(DesktopSetting) + Send>) -> u64 {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let Some(dispatch) = dispatch() else { return id };
        let watch = Watch { namespace: namespace.to_owned(), key: key.to_owned(), on_change, last: None };
        dispatch.send(Dispatch::Add(id, watch)).ok();
        reads_channel().0.try_send((namespace.to_owned(), key.to_owned())).ok();
        crate::platform::portal_appearance::begin();
        id
    }

    pub(super) fn unwatch(id: u64) {
        if let Some(Some(dispatch)) = DISPATCH.get() { dispatch.send(Dispatch::Remove(id)).ok(); }
    }

    /// A value the portal read or signalled, on the worker: queued for the dispatch thread.
    pub(crate) fn changed(namespace: &str, key: &str, value: &Value<'_>) {
        let Some(Some(dispatch)) = DISPATCH.get() else { return };
        let Some(value) = decode(value) else { return };
        dispatch.send(Dispatch::Value { namespace: namespace.to_owned(), key: key.to_owned(), value }).ok();
    }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) use dbus::{changed, reads};

#[cfg(test)]
#[path = "desktop_settings_tests.rs"]
mod tests;
