//! Carbon hotkeys do not require Accessibility or Input Monitoring permission.

#![allow(unsafe_code)]

use super::{Backend, Shortcut, ShortcutError, ShortcutId, Sink, native_status, parse::*};
use objc2_core_foundation::{CFArray, CFBoolean, CFDictionary, CFNumber, CFPreferencesCopyAppValue, CFRetained, CFString, CFType};
use std::{cell::RefCell, collections::HashMap, ffi::c_void, ptr};

type Ref = *mut c_void;
#[repr(C, packed(2))]
#[derive(Clone, Copy)]
struct HotKeyId { signature: u32, id: u32 }
#[repr(C, packed(2))]
struct EventType { class: u32, kind: u32 }
const SIGNATURE: u32 = u32::from_be_bytes(*b"rune");
const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
const EXISTS: i64 = -9878;
const NOT_HANDLED: i32 = -9874;
const SHARED: u32 = 0;
// Without this flag Carbon permits another process to register the same combination.
const EXCLUSIVE: u32 = 1;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn GetApplicationEventTarget() -> Ref;
    fn InstallEventHandler(target: Ref, handler: unsafe extern "C" fn(Ref, Ref, Ref) -> i32, count: usize, types: *const EventType, data: Ref, out: *mut Ref) -> i32;
    fn RemoveEventHandler(handler: Ref) -> i32;
    fn RegisterEventHotKey(code: u32, mods: u32, id: HotKeyId, target: Ref, options: u32, out: *mut Ref) -> i32;
    fn UnregisterEventHotKey(hotkey: Ref) -> i32;
    fn GetEventParameter(event: Ref, name: u32, ty: u32, actual: *mut u32, size: usize, actual_size: *mut usize, data: Ref) -> i32;
}

struct Context { sink: Sink, keys: RefCell<HashMap<ShortcutId, Ref>> }
pub(super) struct Native { handler: Ref, context: Box<Context> }

unsafe extern "C" fn activated(_: Ref, event: Ref, data: Ref) -> i32 {
    // SAFETY: Carbon only calls this while the installed handler and its boxed context live.
    // The application event target delivers callbacks on the registration/main thread.
    unsafe {
        let mut id = HotKeyId { signature: 0, id: 0 };
        if GetEventParameter(event, u32::from_be_bytes(*b"----"), u32::from_be_bytes(*b"hkid"), ptr::null_mut(),
            std::mem::size_of::<HotKeyId>(), ptr::null_mut(), (&mut id as *mut HotKeyId).cast()) != 0 || id.signature != SIGNATURE {
            return NOT_HANDLED;
        }
        let context = &*data.cast::<Context>();
        if !context.keys.borrow().contains_key(&ShortcutId(id.id)) { return NOT_HANDLED; }
        // Never unwind across Carbon. Application handlers themselves run later, in Rune.
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (context.sink)(ShortcutId(id.id), None))).ok();
        0
    }
}

impl Native {
    pub(super) fn new(sink: Sink) -> Result<Self, ShortcutError> {
        if objc2::MainThreadMarker::new().is_none() { return Err(ShortcutError::Backend("Carbon requires the main thread".into())); }
        let mut context = Box::new(Context { sink, keys: RefCell::default() });
        let mut handler = ptr::null_mut();
        // SAFETY: Context's address remains stable until after RemoveEventHandler in Drop.
        native_status(unsafe { InstallEventHandler(GetApplicationEventTarget(), activated, 1,
            &EventType { class: KEYBOARD, kind: 5 }, (&mut *context as *mut Context).cast(), &mut handler) }.into(), EXISTS)?;
        Ok(Self { handler, context })
    }
}

/// True when `CopySymbolicHotKeys` holds a turned-off entry for `chord` and no enabled entry on
/// the same chord. macOS can still reject `EXCLUSIVE` with `-9878` for unchecked system shortcuts
/// (such as Spotlight's ⌘Space), while allowing a `SHARED` registration.
fn disabled_only_symbolic(chord: (u32, u32), symbolic: &[((u32, u32), bool)]) -> bool {
    let mut saw_disabled = false;
    for &(entry, enabled) in symbolic {
        if entry != chord { continue; }
        if enabled { return false; }
        saw_disabled = true;
    }
    saw_disabled
}

/// Registers exclusively first so another app's exclusive hold returns `Conflict`. Only when
/// Carbon returns `EXISTS` (`-9878`) for a disabled-only symbolic hotkey does it retry once with
/// `SHARED`, keeping the success path free of `CopySymbolicHotKeys` calls.
fn register_with_fallback<T>(
    chord: (u32, u32),
    mut try_register: impl FnMut(u32) -> (i32, T),
    symbolic: impl FnOnce() -> Vec<((u32, u32), bool)>,
) -> Result<T, ShortcutError> {
    let (status, handle) = try_register(EXCLUSIVE);
    if i64::from(status) == EXISTS && disabled_only_symbolic(chord, &symbolic()) {
        let (status, handle) = try_register(SHARED);
        native_status(status.into(), EXISTS)?;
        return Ok(handle);
    }
    native_status(status.into(), EXISTS)?;
    Ok(handle)
}

fn carbon_keycode(shortcut: &Shortcut) -> Result<u32, ShortcutError> {
    keycode(shortcut.key_name()).ok_or_else(|| ShortcutError::InvalidAccelerator(shortcut.to_string()))
}

impl Backend for Native {
    fn register(&mut self, id: ShortcutId, shortcut: &Shortcut) -> Result<(), ShortcutError> {
        let code = carbon_keycode(shortcut)?;
        let mods = carbon_mods(shortcut);
        let hotkey = register_with_fallback(
            (code, mods),
            |options| {
                let mut hotkey = ptr::null_mut();
                // SAFETY: On the main thread; pointers are valid output slots and target is Carbon-owned.
                let status = unsafe { RegisterEventHotKey(code, mods, HotKeyId { signature: SIGNATURE, id: id.0 },
                    GetApplicationEventTarget(), options, &mut hotkey) };
                (status, hotkey)
            },
            symbolic_hotkeys,
        )?;
        self.context.keys.borrow_mut().insert(id, hotkey);
        Ok(())
    }
    fn unregister(&mut self, id: ShortcutId) -> Result<(), ShortcutError> {
        let hotkey = *self.context.keys.borrow().get(&id).ok_or(ShortcutError::UnknownId)?;
        // SAFETY: This registration is owned by this backend and has not been released.
        native_status(unsafe { UnregisterEventHotKey(hotkey) }.into(), EXISTS)?;
        self.context.keys.borrow_mut().remove(&id);
        Ok(())
    }
}

impl Drop for Native {
    fn drop(&mut self) {
        // SAFETY: Registrations and handler are owned here, on the app thread. Removing the
        // handler before freeing Context prevents callbacks with a dangling user-data pointer.
        unsafe {
            for (_, hotkey) in self.context.keys.get_mut().drain() { UnregisterEventHotKey(hotkey); }
            RemoveEventHandler(self.handler);
        }
    }
}

const CMD: u32 = 1 << 8;
const SHIFT_KEY: u32 = 1 << 9;
const OPTION: u32 = 1 << 11;
const CONTROL: u32 = 1 << 12;
pub(super) fn carbon_mods(shortcut: &Shortcut) -> u32 {
    [(SUPER, CMD), (SHIFT, SHIFT_KEY), (ALT, OPTION), (CTRL, CONTROL)].into_iter()
        .filter(|(bit, _)| shortcut.mods & bit != 0).fold(0, |mods, (_, carbon)| mods | carbon)
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn CopySymbolicHotKeys(out: *mut *const CFArray) -> i32;
}

/// Default system shortcuts that can be named: (com.apple.symbolichotkeys id holding a user's
/// remapping, Carbon key code, Carbon modifiers, owner). The app switcher cannot be remapped.
const SYSTEM: &[(Option<&str>, u32, u32, &str)] = &[
    (Some("64"), 49, CMD, "Spotlight"),
    (Some("65"), 49, CMD | OPTION, "Finder search"),
    (Some("60"), 49, CONTROL, "Input Sources"),
    (Some("61"), 49, CONTROL | OPTION, "Input Sources"),
    (Some("28"), 20, CMD | SHIFT_KEY, "Screenshot"),
    (Some("29"), 20, CONTROL | CMD | SHIFT_KEY, "Screenshot"),
    (Some("30"), 21, CMD | SHIFT_KEY, "Screenshot"),
    (Some("31"), 21, CONTROL | CMD | SHIFT_KEY, "Screenshot"),
    (Some("184"), 23, CMD | SHIFT_KEY, "Screenshot"),
    (Some("32"), 126, CONTROL, "Mission Control"),
    (Some("33"), 125, CONTROL, "Mission Control"),
    (Some("79"), 123, CONTROL, "Mission Control"),
    (Some("81"), 124, CONTROL, "Mission Control"),
    (None, 48, CMD, "App Switcher"),
    (None, 48, CMD | SHIFT_KEY, "App Switcher"),
];
const MODS: u32 = CMD | SHIFT_KEY | OPTION | CONTROL;

/// An enabled macOS symbolic hotkey on this chord: its name when it is a known shortcut assigned
/// there (by default or remapped), otherwise "macOS". None off the main thread (HIToolbox).
pub(super) fn system_owner(shortcut: &Shortcut) -> Option<String> {
    objc2::MainThreadMarker::new()?;
    let chord = (keycode(shortcut.key_name())?, carbon_mods(shortcut));
    if !symbolic_hotkeys().iter().any(|&(entry, enabled)| enabled && entry == chord) { return None; }
    Some(named(chord, &remapped()).unwrap_or("macOS").into())
}

/// The known shortcut whose effective chord is this one: the user's remapping, else its default.
fn named(chord: (u32, u32), remapped: &[(&str, Option<(u32, u32)>)]) -> Option<&'static str> {
    SYSTEM.iter().find(|(id, code, mods, _)| {
        let remap = id.and_then(|id| remapped.iter().find(|(r, _)| *r == id));
        remap.map_or(Some((*code, *mods)), |(_, at)| *at) == Some(chord)
    }).map(|(.., name)| *name)
}

/// Carbon's effective symbolic hotkeys, defaults included, as `((key code, modifiers), enabled)`.
fn symbolic_hotkeys() -> Vec<((u32, u32), bool)> {
    let mut array = ptr::null();
    // SAFETY: Copy rule: a successful call returns a retained array, released by CFRetained.
    let array = unsafe {
        if CopySymbolicHotKeys(&mut array) != 0 { return Vec::new(); }
        let Some(array) = ptr::NonNull::new(array.cast_mut()) else { return Vec::new() };
        CFRetained::from_raw(array)
    };
    // SAFETY: Each element is a CFDictionary keyed by CFStrings, per HIToolbox's documentation.
    let array = unsafe { array.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    let number = |d: &CFDictionary<CFString, CFType>, key: &CFString| d.get(key)
        .and_then(|v| v.downcast::<CFNumber>().ok()).and_then(|n| n.as_i64());
    // HIToolbox defines these keys as CFSTR literals, not exported symbols.
    let [code, mods, enabled] = ["kHISymbolicHotKeyCode", "kHISymbolicHotKeyModifiers", "kHISymbolicHotKeyEnabled"].map(CFString::from_str);
    let (code, mods, enabled) = (&*code, &*mods, &*enabled);
    array.iter().filter_map(|d| {
        let is_enabled = d.get(enabled)?.downcast::<CFBoolean>().ok()?.as_bool();
        let code = u32::try_from(number(&d, code)?).ok().filter(|&c| c != 65535)?;
        let mods = number(&d, mods)? as u32 & MODS;
        Some(((code, mods), is_enabled))
    }).collect()
}

/// The user's remappings of the named ids: id and its chord now, None when unassigned or disabled.
fn remapped() -> Vec<(&'static str, Option<(u32, u32)>)> {
    let Some(all) = CFPreferencesCopyAppValue(&CFString::from_str("AppleSymbolicHotKeys"),
        &CFString::from_str("com.apple.symbolichotkeys")).and_then(|v| v.downcast::<CFDictionary>().ok()) else { return Vec::new() };
    // SAFETY: The preference is a property list: string keys, dictionary or other values.
    let all = unsafe { all.cast_unchecked::<CFString, CFType>() };
    let dict = |v: CFRetained<CFType>| v.downcast::<CFDictionary>().ok();
    SYSTEM.iter().filter_map(|(id, ..)| {
        let id = (*id)?;
        let entry = dict(all.get(&CFString::from_str(id))?)?;
        // SAFETY: As above, a property-list dictionary.
        let entry = unsafe { entry.cast_unchecked::<CFString, CFType>() };
        let enabled = entry.get(&CFString::from_str("enabled")).and_then(|v| v.downcast::<CFBoolean>().ok()).map(|b| b.as_bool());
        let parameters = entry.get(&CFString::from_str("value")).and_then(dict).and_then(|value| {
            // SAFETY: As above, a property-list dictionary.
            unsafe { value.cast_unchecked::<CFString, CFType>() }.get(&CFString::from_str("parameters"))?.downcast::<CFArray>().ok()
        });
        // SAFETY: parameters is (character, key code, NSEvent modifier flags) as CFNumbers.
        let parameters = parameters.as_deref().map(|p| unsafe { p.cast_unchecked::<CFType>() });
        let at = |i| parameters?.get(i).and_then(|v| v.downcast::<CFNumber>().ok()).and_then(|n| n.as_i64());
        Some((id, remap(enabled, at(1), at(2))?))
    }).collect()
}

/// A remap entry's chord: None (unassigned) when disabled or its key code is 65535; no remap
/// (outer None) when an enabled entry carries no chord.
fn remap(enabled: Option<bool>, code: Option<i64>, flags: Option<i64>) -> Option<Option<(u32, u32)>> {
    if enabled == Some(false) { return Some(None); }
    let (code, flags) = (code?, flags?);
    Some((code != 65535).then(|| (code as u32, ns_event_mods(flags as u32))))
}

fn ns_event_mods(flags: u32) -> u32 {
    [(1 << 17, SHIFT_KEY), (1 << 18, CONTROL), (1 << 19, OPTION), (1 << 20, CMD)].into_iter()
        .filter(|(bit, _)| flags & bit != 0).fold(0, |mods, (_, carbon)| mods | carbon)
}

pub(super) fn keycode(key: &str) -> Option<u32> {
    // Carbon virtual key codes (physical ANSI positions, matching winit KeyCode semantics).
    Some(match key {
        "a" => 0, "s" => 1, "d" => 2, "f" => 3, "h" => 4, "g" => 5, "z" => 6, "x" => 7, "c" => 8, "v" => 9,
        "b" => 11, "q" => 12, "w" => 13, "e" => 14, "r" => 15, "y" => 16, "t" => 17,
        "1" => 18, "2" => 19, "3" => 20, "4" => 21, "6" => 22, "5" => 23, "equal" => 24, "9" => 25,
        "7" => 26, "minus" => 27, "8" => 28, "0" => 29, "bracketright" => 30, "o" => 31, "u" => 32,
        "bracketleft" => 33, "i" => 34, "p" => 35, "enter" => 36, "l" => 37, "j" => 38, "quote" => 39,
        "k" => 40, "semicolon" => 41, "backslash" => 42, "comma" => 43, "slash" => 44, "n" => 45,
        "m" => 46, "period" => 47, "tab" => 48, "space" => 49, "backquote" => 50, "backspace" => 51, "escape" => 53,
        "f17" => 64, "f18" => 79, "f19" => 80, "f20" => 90, "f5" => 96, "f6" => 97, "f7" => 98, "f3" => 99,
        "f8" => 100, "f9" => 101, "f11" => 103, "f13" => 105, "f16" => 106, "f14" => 107, "f10" => 109,
        "f12" => 111, "f15" => 113, "insert" => 114, "home" => 115, "pageup" => 116, "delete" => 117,
        "f4" => 118, "end" => 119, "f2" => 120, "pagedown" => 121, "f1" => 122,
        "left" => 123, "right" => 124, "down" => 125, "up" => 126,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn preference_modifiers_map_to_carbon_and_drop_fn() {
        use super::*;
        assert_eq!(ns_event_mods(1_179_648), CMD | SHIFT_KEY);
        assert_eq!(ns_event_mods(8_650_752), CONTROL);
        assert_eq!(ns_event_mods(786_432), CONTROL | OPTION);
    }

    #[test]
    fn a_remapped_system_shortcut_is_named_at_its_new_chord_only() {
        use super::*;
        // Spotlight (id 64) moved from ⌘Space to ⌃⌥⌘K.
        let moved = remap(Some(true), Some(40), Some(1_835_008)).unwrap();
        assert_eq!(moved, Some((40, CMD | OPTION | CONTROL)));
        assert_eq!(named((40, CMD | OPTION | CONTROL), &[("64", moved)]), Some("Spotlight"));
        assert_eq!(named((49, CMD), &[("64", moved)]), None);
        assert_eq!(named((49, CMD), &[]), Some("Spotlight"));
        // Disabled or unassigned: no chord, so the default no longer names it.
        assert_eq!(remap(Some(false), Some(49), Some(1_048_576)), Some(None));
        assert_eq!(remap(None, Some(65535), Some(0)), Some(None));
        assert_eq!(named((49, CMD), &[("64", None)]), None);
        // An enabled entry without parameters is no remap.
        assert_eq!(remap(Some(true), None, None), None);
    }

    #[test]
    fn registration_retries_shared_only_for_disabled_symbolic_hotkeys() {
        use super::*;
        let chord = (49, CMD);

        // Exclusive succeeds: no symbolic lookup and no retry.
        let mut calls = Vec::new();
        let mut symbolic_calls = 0;
        let got = register_with_fallback(
            chord,
            |options| { calls.push(options); (0, 101u32) },
            || { symbolic_calls += 1; vec![(chord, false)] },
        );
        assert_eq!(got, Ok(101));
        assert_eq!(calls, vec![EXCLUSIVE]);
        assert_eq!(symbolic_calls, 0);

        // -9878 with disabled-only symbolic match: retries once with SHARED and succeeds.
        let mut calls = Vec::new();
        let mut symbolic_calls = 0;
        let got = register_with_fallback(
            chord,
            |options| {
                calls.push(options);
                if options == EXCLUSIVE { (EXISTS as i32, 0u32) } else { (0, 202) }
            },
            || { symbolic_calls += 1; vec![((49, CMD | OPTION), true), (chord, false)] },
        );
        assert_eq!(got, Ok(202));
        assert_eq!(calls, vec![EXCLUSIVE, SHARED]);
        assert_eq!(symbolic_calls, 1);

        // -9878 with enabled symbolic match: no retry, Conflict.
        let mut calls = Vec::new();
        let mut symbolic_calls = 0;
        let got = register_with_fallback(
            chord,
            |options| { calls.push(options); (EXISTS as i32, 0u32) },
            || { symbolic_calls += 1; vec![(chord, true)] },
        );
        assert_eq!(got, Err(ShortcutError::Conflict));
        assert_eq!(calls, vec![EXCLUSIVE]);
        assert_eq!(symbolic_calls, 1);

        // -9878 with both enabled and disabled entries on the same chord: no retry, Conflict.
        for entries in [vec![(chord, false), (chord, true)], vec![(chord, true), (chord, false)]] {
            let mut calls = Vec::new();
            let mut symbolic_calls = 0;
            let got = register_with_fallback(
                chord,
                |options| { calls.push(options); (EXISTS as i32, 0u32) },
                || { symbolic_calls += 1; entries },
            );
            assert_eq!(got, Err(ShortcutError::Conflict));
            assert_eq!(calls, vec![EXCLUSIVE]);
            assert_eq!(symbolic_calls, 1);
        }

        // -9878 with no symbolic match (another process holds it exclusively): no retry, Conflict.
        let mut calls = Vec::new();
        let mut symbolic_calls = 0;
        let got = register_with_fallback(
            chord,
            |options| { calls.push(options); (EXISTS as i32, 0u32) },
            || { symbolic_calls += 1; vec![((49, CMD | OPTION), false)] },
        );
        assert_eq!(got, Err(ShortcutError::Conflict));
        assert_eq!(calls, vec![EXCLUSIVE]);
        assert_eq!(symbolic_calls, 1);

        // Non--9878 OSStatus: no symbolic lookup and no retry, Backend error.
        let mut calls = Vec::new();
        let mut symbolic_calls = 0;
        let got = register_with_fallback(
            chord,
            |options| { calls.push(options); (-50, 0u32) },
            || { symbolic_calls += 1; vec![(chord, false)] },
        );
        assert_eq!(got, Err(ShortcutError::Backend("native status -50".into())));
        assert_eq!(calls, vec![EXCLUSIVE]);
        assert_eq!(symbolic_calls, 0);

        // Shared-retry failure propagates its mapped status (Conflict and Backend).
        for (retry_status, expected) in [
            (EXISTS as i32, ShortcutError::Conflict),
            (-50, ShortcutError::Backend("native status -50".into())),
        ] {
            let mut calls = Vec::new();
            let mut symbolic_calls = 0;
            let got = register_with_fallback(
                chord,
                |options| {
                    calls.push(options);
                    if options == EXCLUSIVE { (EXISTS as i32, 0u32) } else { (retry_status, 0u32) }
                },
                || { symbolic_calls += 1; vec![(chord, false)] },
            );
            assert_eq!(got, Err(expected));
            assert_eq!(calls, vec![EXCLUSIVE, SHARED]);
            assert_eq!(symbolic_calls, 1);
        }
    }

    #[test]
    fn printscreen_is_rejected_with_invalid_accelerator() {
        use super::*;
        assert_eq!(keycode("printscreen"), None);
        for accel in ["printscreen", "print", "prtsc", "ctrl+printscreen", "alt+printscreen"] {
            let shortcut: Shortcut = accel.parse().unwrap();
            assert_eq!(carbon_keycode(&shortcut), Err(ShortcutError::InvalidAccelerator(shortcut.to_string())), "{accel}");
        }
    }
}
