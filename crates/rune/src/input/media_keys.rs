//! Hardware media keys on macOS (`NSEventTypeSystemDefined`, subtype 8).
//!
//! AppKit delivers hardware media keys (Play/Pause, Next, Previous, Fast Forward, Rewind) as
//! `NSEventTypeSystemDefined` events rather than `keyDown`, so winit never surfaces them.
//! The `IOKit/hidsystem/ev_keymap.h` `NX_KEYTYPE_*` decoder and runtime delivery helper live in
//! portable Rust so every platform tests them; on macOS a thin local `NSEvent` monitor is
//! installed only when the app opts in via `App::media_keys`, `WindowSpec::media_keys`, or
//! `ConfiguredOverlay::media_keys`.

use crate::edit::{MediaKey, Mods};

/// `NX_SUBTYPE_AUX_CONTROL_BUTTONS` (media, volume, brightness, eject).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const SUBTYPE_AUX_CONTROL_BUTTONS: i16 = 8;

/// `IOKit/hidsystem/ev_keymap.h` hardware media key codes.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const NX_KEYTYPE_PLAY: u32 = 16;
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const NX_KEYTYPE_NEXT: u32 = 17;
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const NX_KEYTYPE_PREVIOUS: u32 = 18;
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const NX_KEYTYPE_FAST: u32 = 19;
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const NX_KEYTYPE_REWIND: u32 = 20;

/// Key state inside `data1`'s low 16 bits (`(data1 & 0xFF00) >> 8`).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const NX_KEYDOWN: u32 = 0x0A;
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const NX_KEYUP: u32 = 0x0B;

/// A decoded macOS `NSEventTypeSystemDefined` media key transition.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecodedMediaKey {
    pub(crate) key: MediaKey,
    pub(crate) pressed: bool,
    pub(crate) repeat: bool,
}

/// Decodes one `NSEventTypeSystemDefined` `(subtype, data1)` pair into a media key transition.
///
/// Returns `None` for other subtypes, non-media aux keys (volume, brightness, backlight, eject),
/// or key states other than `NX_KEYDOWN` (`0x0A`) and `NX_KEYUP` (`0x0B`).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const fn decode_system_defined(subtype: i16, data1: isize) -> Option<DecodedMediaKey> {
    if subtype != SUBTYPE_AUX_CONTROL_BUTTONS {
        return None;
    }
    let raw = data1 as u64;
    let key_code = ((raw >> 16) & 0xFFFF) as u32;
    let key_flags = (raw & 0xFFFF) as u32;
    let key_state = (key_flags & 0xFF00) >> 8;
    let pressed = match key_state {
        NX_KEYDOWN => true,
        NX_KEYUP => false,
        _ => return None,
    };
    let repeat = pressed && (key_flags & 0x01) != 0;
    let key = match key_code {
        NX_KEYTYPE_PLAY => MediaKey::PlayPause,
        NX_KEYTYPE_NEXT => MediaKey::Next,
        NX_KEYTYPE_PREVIOUS => MediaKey::Previous,
        NX_KEYTYPE_FAST => MediaKey::FastForward,
        NX_KEYTYPE_REWIND => MediaKey::Rewind,
        _ => return None,
    };
    Some(DecodedMediaKey { key, pressed, repeat })
}

/// Decodes macOS `NSEventModifierFlags` into Rune `Mods` (where Command sets both `cmd` and `logo`).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) const fn mods_from_macos_flags(flags: usize) -> Mods {
    const SHIFT: usize = 1 << 17;
    const CONTROL: usize = 1 << 18;
    const OPTION: usize = 1 << 19;
    const COMMAND: usize = 1 << 20;
    let logo = flags & COMMAND != 0;
    Mods {
        shift: flags & SHIFT != 0,
        cmd: logo,
        alt: flags & OPTION != 0,
        ctrl: flags & CONTROL != 0,
        logo,
    }
}

/// Delivers one decoded media key transition into `RuntimeCore`. Returns whether a redraw is
/// needed. Does nothing when `rt.media_keys()` is false.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(crate) fn deliver_decoded(
    rt: &mut crate::runtime::RuntimeCore,
    app: &mut dyn crate::runtime::AppDriver,
    decoded: DecodedMediaKey,
    mods: Mods,
) -> bool {
    if !rt.media_keys() {
        return false;
    }
    let ev = crate::KeyEvent { key: crate::Key::Media(decoded.key), mods };
    if decoded.pressed {
        if decoded.repeat {
            rt.repeat_key(ev);
        } else {
            rt.key(ev);
        }
        true
    } else {
        rt.key_up(app, ev)
    }
}

#[cfg(target_os = "macos")]
mod macos {
    #![allow(unsafe_code)]

    use objc2::rc::Retained;
    use objc2::{ClassType, msg_send};
    use objc2_app_kit::{NSEvent, NSView};
    use objc2_foundation::NSObject;
    use std::{cell::RefCell, rc::Rc};

    use super::{DecodedMediaKey, decode_system_defined, mods_from_macos_flags};
    use crate::edit::Mods;

    /// `NSEventMaskSystemDefined` (`1 << NSEventTypeSystemDefined`, where `NSEventTypeSystemDefined = 14`).
    const SYSTEM_DEFINED_MASK: usize = 1 << 14;

    pub(crate) struct MediaKeys {
        token: Retained<NSObject>,
        /// Decoded media key transitions, oldest first, since the last `take`.
        events: Rc<RefCell<Vec<(DecodedMediaKey, Mods)>>>,
    }

    impl MediaKeys {
        /// Watch hardware media keys while the window showing winit's content view is key.
        /// Consumes (`nil`) only recognized media key events while key, leaving volume,
        /// brightness and unfocused apps untouched.
        pub(crate) fn new(window: &winit::window::Window, wake: crate::Waker) -> Option<Self> {
            let view: Retained<NSView> = crate::platform::macos::content_view(window)?;
            let events = Rc::new(RefCell::new(Vec::new()));
            let seen = events.clone();
            let handler = block2::RcBlock::new(move |event: *mut NSEvent| -> *mut NSEvent {
                if event.is_null() {
                    return event;
                }
                if !view.window().is_some_and(|w| w.isKeyWindow()) {
                    return event;
                }
                // SAFETY: AppKit passes a live NSEventTypeSystemDefined event on the main thread.
                let (subtype, data1, flags): (i16, isize, usize) = unsafe {
                    (
                        msg_send![event, subtype],
                        msg_send![event, data1],
                        msg_send![event, modifierFlags],
                    )
                };
                let Some(decoded) = decode_system_defined(subtype, data1) else {
                    return event;
                };
                seen.borrow_mut().push((decoded, mods_from_macos_flags(flags)));
                wake();
                std::ptr::null_mut()
            });
            // SAFETY: the handler outlives the monitor, which `Drop` removes.
            let token: Option<Retained<NSObject>> = unsafe {
                msg_send![NSEvent::class(), addLocalMonitorForEventsMatchingMask: SYSTEM_DEFINED_MASK, handler: &*handler]
            };
            Some(Self { token: token?, events })
        }

        pub(crate) fn take(&self) -> Vec<(DecodedMediaKey, Mods)> {
            std::mem::take(&mut *self.events.borrow_mut())
        }
    }

    impl Drop for MediaKeys {
        fn drop(&mut self) {
            // SAFETY: the token came from addLocalMonitorForEventsMatchingMask.
            unsafe { let _: () = msg_send![NSEvent::class(), removeMonitor: &*self.token]; }
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::MediaKeys;

#[cfg(test)]
#[path = "media_keys_tests.rs"]
mod tests;
