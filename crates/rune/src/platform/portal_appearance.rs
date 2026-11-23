//! The desktop's light/dark and reduced-motion preferences on Linux, from the XDG desktop portal
//! (`org.freedesktop.portal.Settings`: `org.freedesktop.appearance` `color-scheme` and
//! `reduced-motion`, plus `org.gnome.desktop.interface` `enable-animations` for portals such as
//! `xdg-desktop-portal-gtk` and older GNOME that expose animations there). winit reports no theme
//! on Wayland unless the app set one, so hosts ask here before falling back to dark. One worker
//! thread reads the settings once and follows `SettingChanged`, falling back once on that worker
//! to `kdeglobals` or GSettings when the portal answers neither motion key; any change asks every
//! window to redraw, which rereads the cached atomics without touching disk or D-Bus on the UI
//! thread.

use std::sync::atomic::{AtomicU8, Ordering};

/// Stored state: `UNKNOWN` before the portal answers (or when none is reachable); once the
/// portal answers, `1` (prefer dark) stores `DARK`, while `0` (no preference), `2` (prefer light)
/// and unknown values (treated as `0` per the XDG Settings portal spec) store `LIGHT`.
const UNKNOWN: u8 = 0;
const DARK: u8 = 1;
const LIGHT: u8 = 2;

/// Reduced-motion bitfield in an `AtomicU8`:
/// - `PORTAL_REDUCED_KNOWN` / `PORTAL_REDUCED_ON`: `org.freedesktop.appearance` `reduced-motion` (`1` = reduced)
/// - `PORTAL_ANIM_KNOWN` / `PORTAL_ANIM_DISABLED`: `org.gnome.desktop.interface` `enable-animations` (`false` = reduced)
/// - `FALLBACK_REDUCED_ON`: one-shot fallback read on the worker thread when neither portal key answered
const PORTAL_REDUCED_KNOWN: u8 = 1 << 0;
const PORTAL_REDUCED_ON: u8 = 1 << 1;
const PORTAL_ANIM_KNOWN: u8 = 1 << 2;
const PORTAL_ANIM_DISABLED: u8 = 1 << 3;
const FALLBACK_REDUCED_ON: u8 = 1 << 4;

/// Encodes a portal `color-scheme` value into the stored state. Once the portal answers, only `1`
/// resolves to dark; `0`, `2` and unknown values resolve to light (matching Chromium, Firefox and
/// libadwaita, and GNOME's Default style which reports `0`).
fn encode_scheme(scheme: u32) -> u8 {
    if scheme == 1 { DARK } else { LIGHT }
}

/// Decodes the stored state into `Some(true)` dark, `Some(false)` light, or `None` when no
/// portal has answered.
fn stored_dark(stored: u8) -> Option<bool> {
    match stored {
        DARK => Some(true),
        LIGHT => Some(false),
        _ => None,
    }
}

/// Records a portal `color-scheme` answer in `slot` and returns whether the stored state changed
/// (including the first answer leaving `UNKNOWN`).
fn record_scheme(slot: &AtomicU8, scheme: u32) -> bool {
    let next = encode_scheme(scheme);
    slot.swap(next, Ordering::Relaxed) != next
}

/// Whether at least one Settings portal reduced-motion key has answered.
fn has_portal_motion(bits: u8) -> bool {
    bits & (PORTAL_REDUCED_KNOWN | PORTAL_ANIM_KNOWN) != 0
}

/// Resolves the stored motion bits to the effective reduced-motion preference.
/// Once either portal key answers, the portal is authoritative (either `reduced-motion == 1`
/// or `enable-animations == false` requests reduced motion); otherwise the one-shot fallback
/// value applies.
fn resolved_reduced_motion(bits: u8) -> bool {
    if has_portal_motion(bits) {
        bits & (PORTAL_REDUCED_ON | PORTAL_ANIM_DISABLED) != 0
    } else {
        bits & FALLBACK_REDUCED_ON != 0
    }
}

fn update_motion(slot: &AtomicU8, update: impl Fn(u8) -> u8) -> bool {
    let mut prev = slot.load(Ordering::Relaxed);
    loop {
        let next = update(prev);
        match slot.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return resolved_reduced_motion(prev) != resolved_reduced_motion(next),
            Err(actual) => prev = actual,
        }
    }
}

/// Records `org.freedesktop.appearance` `reduced-motion` (`1` = reduced motion, `0` and unknown
/// values = no preference per the XDG Settings portal spec). Returns whether the effective
/// reduced-motion state changed.
fn record_reduced_motion(slot: &AtomicU8, value: u32) -> bool {
    update_motion(slot, |bits| {
        let bits = (bits | PORTAL_REDUCED_KNOWN) & !PORTAL_REDUCED_ON;
        if value == 1 { bits | PORTAL_REDUCED_ON } else { bits }
    })
}

/// Records `org.gnome.desktop.interface` `enable-animations` (`false` = reduced motion).
/// Returns whether the effective reduced-motion state changed.
fn record_enable_animations(slot: &AtomicU8, enabled: bool) -> bool {
    update_motion(slot, |bits| {
        let bits = (bits | PORTAL_ANIM_KNOWN) & !PORTAL_ANIM_DISABLED;
        if !enabled { bits | PORTAL_ANIM_DISABLED } else { bits }
    })
}

/// Records the one-shot fallback result (read off the UI thread when the portal answered neither
/// motion key). Returns whether the effective reduced-motion state changed.
fn record_fallback_reduced_motion(slot: &AtomicU8, reduced: bool) -> bool {
    update_motion(slot, |bits| {
        let bits = bits & !FALLBACK_REDUCED_ON;
        if reduced { bits | FALLBACK_REDUCED_ON } else { bits }
    })
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
mod dbus {
    use futures_lite::StreamExt;
    use std::{sync::{Arc, Mutex, OnceLock, Weak, atomic::{AtomicU8, Ordering}, mpsc}, time::Duration};
    use winit::window::Window;
    use zbus::{Connection, Proxy, zvariant::{OwnedValue, Value}};

    use super::{
        UNKNOWN, has_portal_motion, record_enable_animations, record_fallback_reduced_motion,
        record_reduced_motion, record_scheme, resolved_reduced_motion, stored_dark,
    };
    use crate::platform::portal::{self, DEST, PATH};

    const INTERFACE: &str = "org.freedesktop.portal.Settings";
    const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
    const COLOR_SCHEME_KEY: &str = "color-scheme";
    const REDUCED_MOTION_KEY: &str = "reduced-motion";
    const GNOME_INTERFACE_NAMESPACE: &str = "org.gnome.desktop.interface";
    const ENABLE_ANIMATIONS_KEY: &str = "enable-animations";
    /// How long the first window waits for the portal's first answer, as sctk-adwaita does for its
    /// frame; a slower answer redraws the windows when it arrives.
    const FIRST_ANSWER: Duration = Duration::from_millis(100);
    /// Connecting, D-Bus activation of the portal and the first read.
    const RPC_TIMEOUT: Duration = Duration::from_secs(10);

    static SCHEME: AtomicU8 = AtomicU8::new(UNKNOWN);
    static MOTION: AtomicU8 = AtomicU8::new(0);
    static WATCH: OnceLock<()> = OnceLock::new();
    static WINDOWS: Mutex<Vec<Weak<Window>>> = Mutex::new(Vec::new());

    /// Some(true) when the desktop prefers dark, Some(false) light (including portal `0` "no
    /// preference", which GNOME's Default style reports), or None when no portal has answered.
    pub(crate) fn dark() -> Option<bool> {
        WATCH.get_or_init(start);
        stored_dark(SCHEME.load(Ordering::Relaxed))
    }

    /// Whether the desktop asks for reduced motion, cached from the Settings portal (or a
    /// one-shot off-UI-thread fallback when the portal answers neither motion key).
    pub(crate) fn reduced_motion() -> bool {
        WATCH.get_or_init(start);
        resolved_reduced_motion(MOTION.load(Ordering::Relaxed))
    }

    /// Redraws `window` whenever a watched portal preference changes; its redraw rereads
    /// [`dark`] and [`reduced_motion`].
    pub(crate) fn follow(window: &Arc<Window>) {
        let mut windows = WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
        windows.retain(|w| w.strong_count() > 0);
        windows.push(Arc::downgrade(window));
    }

    /// Unwraps a `u32` portal value (`color-scheme`, `reduced-motion`). Portals answer `v`, and
    /// `Read` (before `ReadOne`) wraps it twice.
    pub(super) fn scheme(value: &Value<'_>) -> Option<u32> {
        match value {
            Value::U32(x) => Some(*x),
            Value::Value(inner) => scheme(inner),
            _ => None,
        }
    }

    /// Unwraps a `bool` portal value (`org.gnome.desktop.interface` `enable-animations`).
    pub(super) fn bool_value(value: &Value<'_>) -> Option<bool> {
        match value {
            Value::Bool(b) => Some(*b),
            Value::Value(inner) => bool_value(inner),
            _ => None,
        }
    }

    fn redraw_windows() {
        for window in WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade) {
            window.request_redraw();
        }
    }

    fn store_scheme(value: &Value<'_>) {
        let Some(scheme) = scheme(value) else { return };
        if record_scheme(&SCHEME, scheme) { redraw_windows(); }
    }

    fn store_reduced_motion(value: &Value<'_>) {
        let Some(reduced) = scheme(value) else { return };
        if record_reduced_motion(&MOTION, reduced) { redraw_windows(); }
    }

    fn store_enable_animations(value: &Value<'_>) {
        let Some(enabled) = bool_value(value) else { return };
        if record_enable_animations(&MOTION, enabled) { redraw_windows(); }
    }

    fn store_fallback_reduced_motion(reduced: bool) {
        if record_fallback_reduced_motion(&MOTION, reduced) { redraw_windows(); }
    }

    fn start() {
        let (ready, first) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("rune-appearance-portal".into()).spawn(move || {
            async_io::block_on(watch(ready));
        });
        if worker.is_ok() { first.recv_timeout(FIRST_ANSWER).ok(); }
    }

    async fn read_setting(portal: &Proxy<'_>, namespace: &str, key: &str) -> Result<OwnedValue, bool> {
        match portal.call::<_, _, OwnedValue>("ReadOne", &(namespace, key)).await {
            Ok(v) => Ok(v),
            Err(first) => match portal.call::<_, _, OwnedValue>("Read", &(namespace, key)).await {
                Ok(v) => Ok(v),
                Err(second) => Err(portal::is_unavailable(&first) && portal::is_unavailable(&second)),
            },
        }
    }

    async fn watch(ready: mpsc::SyncSender<()>) {
        let setup = portal::timeout(RPC_TIMEOUT, async {
            let conn = Connection::session().await.ok()?;
            let portal = Proxy::new(&conn, DEST, PATH, INTERFACE).await.ok()?;
            // Subscribe first, so a change between the read and the subscription isn't lost.
            let changes = portal.receive_signal("SettingChanged").await.ok()?;
            let scheme_res = read_setting(&portal, APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY).await;
            if let Ok(v) = &scheme_res { store_scheme(v); }
            let (read_ok, unavailable) = if matches!(scheme_res, Err(true)) {
                (false, true)
            } else {
                let reduced_res = read_setting(&portal, APPEARANCE_NAMESPACE, REDUCED_MOTION_KEY).await;
                if let Ok(v) = &reduced_res { store_reduced_motion(v); }
                let anim_res = read_setting(&portal, GNOME_INTERFACE_NAMESPACE, ENABLE_ANIMATIONS_KEY).await;
                if let Ok(v) = &anim_res { store_enable_animations(v); }
                let read_ok = scheme_res.is_ok() || reduced_res.is_ok() || anim_res.is_ok();
                let unavailable = matches!(scheme_res, Err(true))
                    && matches!(reduced_res, Err(true))
                    && matches!(anim_res, Err(true));
                (read_ok, unavailable)
            };
            if !portal::should_watch_settings(read_ok, unavailable) {
                conn.close().await.ok();
                return None;
            }
            Some((conn, changes))
        }).await.flatten();
        if !has_portal_motion(MOTION.load(Ordering::Relaxed)) {
            store_fallback_reduced_motion(crate::platform::preferences::read_reduced_motion_fallback());
        }
        ready.send(()).ok();
        let Some((_conn, mut changes)) = setup else { return };
        while let Some(message) = changes.next().await {
            let Ok((namespace, key, value)) = message.body().deserialize::<(String, String, OwnedValue)>() else {
                continue;
            };
            match (namespace.as_str(), key.as_str()) {
                (APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY) => store_scheme(&value),
                (APPEARANCE_NAMESPACE, REDUCED_MOTION_KEY) => store_reduced_motion(&value),
                (GNOME_INTERFACE_NAMESPACE, ENABLE_ANIMATIONS_KEY) => store_enable_animations(&value),
                _ => {}
            }
        }
    }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) use dbus::{dark, follow, reduced_motion};

#[cfg(test)]
#[path = "portal_appearance_tests.rs"]
mod tests;
