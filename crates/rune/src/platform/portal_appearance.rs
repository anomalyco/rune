//! The desktop's light/dark, reduced-motion and double-click preferences on Linux, from the XDG
//! desktop portal (`org.freedesktop.portal.Settings`: `org.freedesktop.appearance` `color-scheme`
//! and `reduced-motion`, `org.gnome.desktop.interface` `enable-animations` for portals such as
//! `xdg-desktop-portal-gtk` and older GNOME that expose animations there, and
//! `org.gnome.desktop.peripherals.mouse` `double-click` in milliseconds). winit reports no theme
//! on Wayland unless the app set one, so hosts ask here before falling back to dark. One worker
//! thread, started when the event loop is made (`begin`), so that a portal that is up has
//! answered before the first window asks, reads the settings once and follows `SettingChanged`,
//! falling back once on that worker to `kdeglobals` or GSettings when the portal does not answer
//! motion or double-click keys; any change asks every window to redraw, which rereads the cached
//! atomics without touching disk or D-Bus on the UI thread.

use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use crate::platform::preferences::{DEFAULT_DOUBLE_CLICK_MS, clamp_double_click_ms};

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

/// A run's first frame takes the scheme the last run saw (`last`), unless the portal has answered
/// already: its answer is newer than any run's. Returns whether the seed was stored.
fn seed_scheme(slot: &AtomicU8, dark: bool) -> bool {
    slot.compare_exchange(UNKNOWN, if dark { DARK } else { LIGHT }, Ordering::Relaxed, Ordering::Relaxed).is_ok()
}

/// The last run's reduced motion, where the fallback's answer goes: a portal's motion key, when
/// one answers, decides instead, and the fallback read replaces it when none does.
fn seed_reduced_motion(slot: &AtomicU8, reduced: bool) {
    update_motion(slot, |bits| if has_portal_motion(bits) || !reduced { bits } else { bits | FALLBACK_REDUCED_ON });
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

/// Stored double-click interval in an `AtomicU32`:
/// - `0` (`DOUBLE_CLICK_UNKNOWN`) when neither the portal nor fallback has answered
/// - lower 31 bits: clamped interval in milliseconds (`50..=5000`)
/// - top bit (`PORTAL_DOUBLE_CLICK_BIT`): set once the Settings portal has answered, so the
///   one-shot fallback cannot overwrite an authoritative portal value
const DOUBLE_CLICK_UNKNOWN: u32 = 0;
const PORTAL_DOUBLE_CLICK_BIT: u32 = 1 << 31;
const DOUBLE_CLICK_MS_MASK: u32 = !PORTAL_DOUBLE_CLICK_BIT;

/// Whether the Settings portal has answered `org.gnome.desktop.peripherals.mouse` `double-click`.
fn has_portal_double_click(stored: u32) -> bool {
    stored & PORTAL_DOUBLE_CLICK_BIT != 0
}

/// Decodes the stored double-click interval into seconds, or `None` when unanswered.
fn stored_double_click_secs(stored: u32) -> Option<f64> {
    let ms = stored & DOUBLE_CLICK_MS_MASK;
    (ms > 0).then(|| f64::from(ms) / 1000.0)
}

/// Effective double-click interval in milliseconds (treating unanswered as the 500 ms default).
fn effective_double_click_ms(stored: u32) -> u32 {
    let ms = stored & DOUBLE_CLICK_MS_MASK;
    if ms > 0 { ms } else { DEFAULT_DOUBLE_CLICK_MS }
}

/// Records a portal `double-click` interval (milliseconds, clamped to `50..=5000`). Returns
/// whether the effective interval changed.
fn record_double_click(slot: &AtomicU32, ms: u32) -> bool {
    let Some(clamped) = clamp_double_click_ms(ms) else { return false };
    let next = clamped | PORTAL_DOUBLE_CLICK_BIT;
    let prev = slot.swap(next, Ordering::Relaxed);
    effective_double_click_ms(prev) != clamped
}

/// Records the one-shot fallback `double-click` interval (read off the UI thread when the portal
/// did not answer the key). Does not overwrite a portal answer. Returns whether the effective
/// interval changed.
fn record_fallback_double_click(slot: &AtomicU32, ms: u32) -> bool {
    let Some(clamped) = clamp_double_click_ms(ms) else { return false };
    let mut prev = slot.load(Ordering::Relaxed);
    loop {
        if has_portal_double_click(prev) { return false; }
        match slot.compare_exchange_weak(prev, clamped, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return effective_double_click_ms(prev) != clamped,
            Err(actual) => prev = actual,
        }
    }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
mod dbus {
    use futures_lite::StreamExt;
    use std::{sync::{Arc, Mutex, OnceLock, Weak, atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering}, mpsc}, time::{Duration, Instant}};
    use winit::window::Window;
    use zbus::{Connection, Proxy, zvariant::{OwnedValue, Value}};

    use super::{
        DOUBLE_CLICK_UNKNOWN, UNKNOWN, has_portal_double_click, has_portal_motion,
        record_double_click, record_enable_animations, record_fallback_double_click,
        record_fallback_reduced_motion, record_reduced_motion, record_scheme,
        resolved_reduced_motion, seed_reduced_motion, seed_scheme, stored_dark,
        stored_double_click_secs,
    };
    use crate::platform::preferences::Appearance;
    use crate::platform::portal::{self, DEST, PATH};

    const INTERFACE: &str = "org.freedesktop.portal.Settings";
    const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";
    const COLOR_SCHEME_KEY: &str = "color-scheme";
    const REDUCED_MOTION_KEY: &str = "reduced-motion";
    const GNOME_INTERFACE_NAMESPACE: &str = "org.gnome.desktop.interface";
    const ENABLE_ANIMATIONS_KEY: &str = "enable-animations";
    const MOUSE_NAMESPACE: &str = "org.gnome.desktop.peripherals.mouse";
    const DOUBLE_CLICK_KEY: &str = "double-click";
    /// How long the portal has for its first answer, from when the worker starts, before the
    /// first window stops waiting for it (as sctk-adwaita waits for its frame's); a slower answer
    /// redraws the windows when it arrives.
    const FIRST_ANSWER: Duration = Duration::from_millis(100);
    /// Connecting, D-Bus activation of the portal and the first read.
    const RPC_TIMEOUT: Duration = Duration::from_secs(10);

    static SCHEME: AtomicU8 = AtomicU8::new(UNKNOWN);
    static MOTION: AtomicU8 = AtomicU8::new(0);
    static DOUBLE_CLICK: AtomicU32 = AtomicU32::new(DOUBLE_CLICK_UNKNOWN);
    static WATCH: OnceLock<()> = OnceLock::new();
    /// The worker's first answer on its way, and when the worker started: the first ask takes
    /// it and waits for what is left of `FIRST_ANSWER`; an ask meanwhile waits behind it.
    static FIRST: Mutex<Option<(mpsc::Receiver<()>, Instant)>> = Mutex::new(None);

    /// The app gave what its last run saw (`last`): no ask waits for the portal.
    static SEEDED: AtomicBool = AtomicBool::new(false);
    /// `last`'s keeper, and the appearance it was last given (or the last run's own).
    #[allow(clippy::type_complexity)]
    static KEEP: Mutex<Option<(Box<dyn FnMut(Appearance) + Send>, Option<Appearance>)>> = Mutex::new(None);

    /// `rune::last_appearance`: `last` is what the first frame draws with until the portal
    /// answers, and `keep` gets every answer that differs from what it last had, on the worker.
    pub(crate) fn last(last: Option<Appearance>, keep: Box<dyn FnMut(Appearance) + Send>) {
        *KEEP.lock().unwrap_or_else(|e| e.into_inner()) = Some((keep, last));
        let Some(last) = last else { return };
        if let Some(dark) = last.dark { seed_scheme(&SCHEME, dark); }
        seed_reduced_motion(&MOTION, last.reduced_motion);
        SEEDED.store(true, Ordering::Relaxed);
    }

    /// Gives the keeper the appearance as it stands, when it isn't what the keeper last had.
    fn keep_now() {
        let now = Appearance { dark: stored_dark(SCHEME.load(Ordering::Relaxed)), reduced_motion: resolved_reduced_motion(MOTION.load(Ordering::Relaxed)) };
        let mut keeper = KEEP.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((keep, had)) = keeper.as_mut() && *had != Some(now) {
            keep(now);
            *had = Some(now);
        }
    }

    /// Starts the worker, if it isn't started, without waiting for it. An event loop calls this
    /// before it is built: the worker's connection, its reads and its fallback (which loads
    /// GSettings) then run beside the app's setup, not in the first window's creation.
    pub(crate) fn begin() {
        WATCH.get_or_init(start);
    }

    /// The first ask: waits for the worker's first answer, until `FIRST_ANSWER` after it started.
    /// With the last run's appearance to draw with (`last`) it doesn't wait: the answer, when it
    /// differs, redraws the windows.
    fn first_answer() {
        WATCH.get_or_init(start);
        let mut first = FIRST.lock().unwrap_or_else(|e| e.into_inner());
        let Some((answer, started)) = first.take() else { return };
        if SEEDED.load(Ordering::Relaxed) { return; }
        answer.recv_timeout(FIRST_ANSWER.saturating_sub(started.elapsed())).ok();
    }
    static WINDOWS: Mutex<Vec<Weak<Window>>> = Mutex::new(Vec::new());

    /// Some(true) when the desktop prefers dark, Some(false) light (including portal `0` "no
    /// preference", which GNOME's Default style reports), or None when no portal has answered.
    pub(crate) fn dark() -> Option<bool> {
        first_answer();
        stored_dark(SCHEME.load(Ordering::Relaxed))
    }

    /// Whether the desktop asks for reduced motion, cached from the Settings portal (or a
    /// one-shot off-UI-thread fallback when the portal answers neither motion key).
    pub(crate) fn reduced_motion() -> bool {
        first_answer();
        resolved_reduced_motion(MOTION.load(Ordering::Relaxed))
    }

    /// The desktop's configured double-click interval in seconds, cached from the Settings portal
    /// (`org.gnome.desktop.peripherals.mouse` `double-click`) or a one-shot off-UI-thread
    /// fallback when the portal does not answer that key, or `None` when neither answered.
    pub(crate) fn double_click_interval() -> Option<f64> {
        first_answer();
        stored_double_click_secs(DOUBLE_CLICK.load(Ordering::Relaxed))
    }

    /// Redraws `window` whenever a watched portal preference changes; its redraw rereads
    /// [`dark`], [`reduced_motion`] and [`double_click_interval`].
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

    /// Unwraps a positive millisecond portal value (`org.gnome.desktop.peripherals.mouse`
    /// `double-click`, exposed as `i32` by GSettings portals or `u32`). Returns `None` for
    /// non-positive or non-integer values.
    pub(super) fn millis_value(value: &Value<'_>) -> Option<u32> {
        match value {
            Value::I32(ms) => u32::try_from(*ms).ok().filter(|&ms| ms > 0),
            Value::U32(ms) => (*ms > 0).then_some(*ms),
            Value::Value(inner) => millis_value(inner),
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

    fn store_double_click(value: &Value<'_>) {
        let Some(ms) = millis_value(value) else { return };
        if record_double_click(&DOUBLE_CLICK, ms) { redraw_windows(); }
    }

    fn store_fallback_double_click(ms: u32) {
        if record_fallback_double_click(&DOUBLE_CLICK, ms) { redraw_windows(); }
    }

    fn start() {
        let (ready, first) = mpsc::sync_channel(1);
        let worker = std::thread::Builder::new().name("rune-appearance-portal".into()).spawn(move || {
            async_io::block_on(watch(ready));
        });
        if worker.is_ok() { *FIRST.lock().unwrap_or_else(|e| e.into_inner()) = Some((first, crate::time::now())); }
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
                let dbl_res = read_setting(&portal, MOUSE_NAMESPACE, DOUBLE_CLICK_KEY).await;
                if let Ok(v) = &dbl_res { store_double_click(v); }
                let read_ok = scheme_res.is_ok() || reduced_res.is_ok() || anim_res.is_ok() || dbl_res.is_ok();
                let unavailable = matches!(scheme_res, Err(true))
                    && matches!(reduced_res, Err(true))
                    && matches!(anim_res, Err(true))
                    && matches!(dbl_res, Err(true));
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
        if !has_portal_double_click(DOUBLE_CLICK.load(Ordering::Relaxed))
            && let Some(ms) = crate::platform::preferences::read_double_click_fallback()
        {
            store_fallback_double_click(ms);
        }
        keep_now();
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
                (MOUSE_NAMESPACE, DOUBLE_CLICK_KEY) => {
                    store_double_click(&value);
                    continue;
                }
                _ => continue,
            }
            keep_now();
        }
    }
}

#[cfg(all(target_os = "linux", feature = "dbus"))]
pub(crate) use dbus::{begin, dark, double_click_interval, follow, last, reduced_motion};

#[cfg(test)]
#[path = "portal_appearance_tests.rs"]
mod tests;
