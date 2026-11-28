//! Pure Android key-event, editor-action, and `InputConnection` translation.
//!
//! Compiles on every host target so Android keycode mapping, modifier translation,
//! dead-key handling, editor actions, and `InputConnection` edit dispatch are
//! verified by unit tests off-device.

use crate::edit::Ime;
use crate::runtime::RuntimeCore;
use crate::{Key, KeyEvent, MediaKey, Mods};

/// Android `KeyEvent.META_SHIFT_ON | META_SHIFT_LEFT_ON | META_SHIFT_RIGHT_ON`.
pub(crate) const META_SHIFT_MASK: i32 = 0x01 | 0x40 | 0x80;
/// Android `KeyEvent.META_ALT_ON | META_ALT_LEFT_ON | META_ALT_RIGHT_ON`.
pub(crate) const META_ALT_MASK: i32 = 0x02 | 0x10 | 0x20;
/// Android `KeyEvent.META_CTRL_ON | META_CTRL_LEFT_ON | META_CTRL_RIGHT_ON`.
pub(crate) const META_CTRL_MASK: i32 = 0x1000 | 0x2000 | 0x4000;
/// Android `KeyEvent.META_META_ON | META_META_LEFT_ON | META_META_RIGHT_ON`.
pub(crate) const META_META_MASK: i32 = 0x10000 | 0x20000 | 0x40000;

/// Android `KeyCharacterMap.COMBINING_ACCENT` high bit marking a dead key.
pub(crate) const COMBINING_ACCENT: u32 = 0x8000_0000;

/// Upper bound on synthetic Backspace/Delete key events queued by one
/// `InputConnection.deleteSurroundingText` call so a malformed IME request
/// cannot stall the UI thread.
pub(crate) const MAX_DELETE_SURROUNDING: usize = 4096;

// Android `KeyEvent.KEYCODE_*` constants (identical to NDK `AKEYCODE_*`).
pub(crate) const KEYCODE_0: i32 = 7;
pub(crate) const KEYCODE_9: i32 = 16;
pub(crate) const KEYCODE_DPAD_UP: i32 = 19;
pub(crate) const KEYCODE_DPAD_DOWN: i32 = 20;
pub(crate) const KEYCODE_DPAD_LEFT: i32 = 21;
pub(crate) const KEYCODE_DPAD_RIGHT: i32 = 22;
pub(crate) const KEYCODE_A: i32 = 29;
pub(crate) const KEYCODE_Z: i32 = 54;
pub(crate) const KEYCODE_COMMA: i32 = 55;
pub(crate) const KEYCODE_PERIOD: i32 = 56;
pub(crate) const KEYCODE_TAB: i32 = 61;
pub(crate) const KEYCODE_SPACE: i32 = 62;
pub(crate) const KEYCODE_ENTER: i32 = 66;
pub(crate) const KEYCODE_DEL: i32 = 67;
pub(crate) const KEYCODE_GRAVE: i32 = 68;
pub(crate) const KEYCODE_MINUS: i32 = 69;
pub(crate) const KEYCODE_EQUALS: i32 = 70;
pub(crate) const KEYCODE_LEFT_BRACKET: i32 = 71;
pub(crate) const KEYCODE_RIGHT_BRACKET: i32 = 72;
pub(crate) const KEYCODE_BACKSLASH: i32 = 73;
pub(crate) const KEYCODE_SEMICOLON: i32 = 74;
pub(crate) const KEYCODE_APOSTROPHE: i32 = 75;
pub(crate) const KEYCODE_SLASH: i32 = 76;
pub(crate) const KEYCODE_MENU: i32 = 82;
pub(crate) const KEYCODE_MEDIA_PLAY_PAUSE: i32 = 85;
pub(crate) const KEYCODE_MEDIA_STOP: i32 = 86;
pub(crate) const KEYCODE_MEDIA_NEXT: i32 = 87;
pub(crate) const KEYCODE_MEDIA_PREVIOUS: i32 = 88;
pub(crate) const KEYCODE_MEDIA_REWIND: i32 = 89;
pub(crate) const KEYCODE_MEDIA_FAST_FORWARD: i32 = 90;
pub(crate) const KEYCODE_PAGE_UP: i32 = 92;
pub(crate) const KEYCODE_PAGE_DOWN: i32 = 93;
pub(crate) const KEYCODE_ESCAPE: i32 = 111;
pub(crate) const KEYCODE_FORWARD_DEL: i32 = 112;
pub(crate) const KEYCODE_SCROLL_LOCK: i32 = 116;
pub(crate) const KEYCODE_SYSRQ: i32 = 120;
pub(crate) const KEYCODE_BREAK: i32 = 121;
pub(crate) const KEYCODE_MOVE_HOME: i32 = 122;
pub(crate) const KEYCODE_MOVE_END: i32 = 123;
pub(crate) const KEYCODE_INSERT: i32 = 124;
pub(crate) const KEYCODE_MEDIA_PLAY: i32 = 126;
pub(crate) const KEYCODE_MEDIA_PAUSE: i32 = 127;
pub(crate) const KEYCODE_F1: i32 = 131;
pub(crate) const KEYCODE_F12: i32 = 142;
pub(crate) const KEYCODE_NUMPAD_0: i32 = 144;
pub(crate) const KEYCODE_NUMPAD_9: i32 = 153;
pub(crate) const KEYCODE_NUMPAD_DIVIDE: i32 = 154;
pub(crate) const KEYCODE_NUMPAD_MULTIPLY: i32 = 155;
pub(crate) const KEYCODE_NUMPAD_SUBTRACT: i32 = 156;
pub(crate) const KEYCODE_NUMPAD_ADD: i32 = 157;
pub(crate) const KEYCODE_NUMPAD_DOT: i32 = 158;
pub(crate) const KEYCODE_NUMPAD_COMMA: i32 = 159;
pub(crate) const KEYCODE_NUMPAD_ENTER: i32 = 160;
pub(crate) const KEYCODE_NUMPAD_EQUALS: i32 = 161;
pub(crate) const KEYCODE_NUMPAD_LEFT_PAREN: i32 = 162;
pub(crate) const KEYCODE_NUMPAD_RIGHT_PAREN: i32 = 163;

// Android `EditorInfo.IME_ACTION_*` constants.
pub(crate) const IME_ACTION_UNSPECIFIED: i32 = 0;
pub(crate) const IME_ACTION_NONE: i32 = 1;
pub(crate) const IME_ACTION_GO: i32 = 2;
pub(crate) const IME_ACTION_SEARCH: i32 = 3;
pub(crate) const IME_ACTION_SEND: i32 = 4;
pub(crate) const IME_ACTION_NEXT: i32 = 5;
pub(crate) const IME_ACTION_DONE: i32 = 6;
pub(crate) const IME_ACTION_PREVIOUS: i32 = 7;

/// Translated text or key action ready to apply to a Rune `RuntimeCore`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TextInputAction {
    Ime(Ime),
    FinishCompositionFromIme,
    DeleteSurrounding { before: usize, after: usize },
    KeyDown { event: KeyEvent, repeat: bool },
}

/// Maps Android `KeyEvent.getMetaState()` bits to Rune `Mods`.
///
/// Android uses Ctrl as its primary shortcut modifier (`cmd = ctrl`), matching
/// Rune's non-Apple convention regardless of which host OS runs unit tests.
pub(crate) const fn map_mods(meta_state: i32) -> Mods {
    let shift = (meta_state & META_SHIFT_MASK) != 0;
    let alt = (meta_state & META_ALT_MASK) != 0;
    let ctrl = (meta_state & META_CTRL_MASK) != 0;
    let logo = (meta_state & META_META_MASK) != 0;
    Mods {
        shift,
        cmd: ctrl,
        alt,
        ctrl,
        logo,
    }
}

fn decode_unicode_char(code: i32) -> Option<char> {
    if code <= 0 || ((code as u32) & COMBINING_ACCENT) != 0 {
        return None;
    }
    let ch = char::from_u32(code as u32)?;
    (!ch.is_control()).then_some(ch)
}

fn named_key(key_code: i32) -> Option<Key> {
    Some(match key_code {
        KEYCODE_DPAD_UP => Key::Up,
        KEYCODE_DPAD_DOWN => Key::Down,
        KEYCODE_DPAD_LEFT => Key::Left,
        KEYCODE_DPAD_RIGHT => Key::Right,
        KEYCODE_TAB => Key::Tab,
        KEYCODE_ENTER | KEYCODE_NUMPAD_ENTER => Key::Enter,
        KEYCODE_DEL => Key::Backspace,
        KEYCODE_FORWARD_DEL => Key::Delete,
        KEYCODE_MENU => Key::ContextMenu,
        KEYCODE_MEDIA_PLAY_PAUSE => Key::Media(MediaKey::PlayPause),
        KEYCODE_MEDIA_PLAY => Key::Media(MediaKey::Play),
        KEYCODE_MEDIA_PAUSE => Key::Media(MediaKey::Pause),
        KEYCODE_MEDIA_NEXT => Key::Media(MediaKey::Next),
        KEYCODE_MEDIA_PREVIOUS => Key::Media(MediaKey::Previous),
        KEYCODE_MEDIA_STOP => Key::Media(MediaKey::Stop),
        KEYCODE_MEDIA_FAST_FORWARD => Key::Media(MediaKey::FastForward),
        KEYCODE_MEDIA_REWIND => Key::Media(MediaKey::Rewind),
        KEYCODE_PAGE_UP => Key::PageUp,
        KEYCODE_PAGE_DOWN => Key::PageDown,
        KEYCODE_ESCAPE => Key::Escape,
        KEYCODE_SCROLL_LOCK => Key::ScrollLock,
        KEYCODE_SYSRQ => Key::PrintScreen,
        KEYCODE_BREAK => Key::Pause,
        KEYCODE_MOVE_HOME => Key::Home,
        KEYCODE_MOVE_END => Key::End,
        KEYCODE_INSERT => Key::Insert,
        KEYCODE_F1..=KEYCODE_F12 => Key::Function((key_code - KEYCODE_F1 + 1) as u8),
        _ => return None,
    })
}

/// US-layout base ASCII character for an Android key code, used for Ctrl chords
/// (including non-ASCII hardware layouts) and synthetic `KeyEvent`s that carry
/// `unicodeChar == 0`.
fn us_base_char(key_code: i32) -> Option<char> {
    Some(match key_code {
        KEYCODE_0..=KEYCODE_9 => (b'0' + (key_code - KEYCODE_0) as u8) as char,
        KEYCODE_A..=KEYCODE_Z => (b'a' + (key_code - KEYCODE_A) as u8) as char,
        KEYCODE_COMMA => ',',
        KEYCODE_PERIOD => '.',
        KEYCODE_GRAVE => '`',
        KEYCODE_MINUS => '-',
        KEYCODE_EQUALS => '=',
        KEYCODE_LEFT_BRACKET => '[',
        KEYCODE_RIGHT_BRACKET => ']',
        KEYCODE_BACKSLASH => '\\',
        KEYCODE_SEMICOLON => ';',
        KEYCODE_APOSTROPHE => '\'',
        KEYCODE_SLASH => '/',
        KEYCODE_NUMPAD_0..=KEYCODE_NUMPAD_9 => (b'0' + (key_code - KEYCODE_NUMPAD_0) as u8) as char,
        KEYCODE_NUMPAD_DIVIDE => '/',
        KEYCODE_NUMPAD_MULTIPLY => '*',
        KEYCODE_NUMPAD_SUBTRACT => '-',
        KEYCODE_NUMPAD_ADD => '+',
        KEYCODE_NUMPAD_DOT => '.',
        KEYCODE_NUMPAD_COMMA => ',',
        KEYCODE_NUMPAD_EQUALS => '=',
        KEYCODE_NUMPAD_LEFT_PAREN => '(',
        KEYCODE_NUMPAD_RIGHT_PAREN => ')',
        _ => return None,
    })
}

fn us_shifted_char(key_code: i32) -> Option<char> {
    let base = us_base_char(key_code)?;
    Some(match key_code {
        KEYCODE_A..=KEYCODE_Z => base.to_ascii_uppercase(),
        KEYCODE_0 => ')',
        8 => '!',
        9 => '@',
        10 => '#',
        11 => '$',
        12 => '%',
        13 => '^',
        14 => '&',
        15 => '*',
        KEYCODE_9 => '(',
        KEYCODE_COMMA => '<',
        KEYCODE_PERIOD => '>',
        KEYCODE_GRAVE => '~',
        KEYCODE_MINUS => '_',
        KEYCODE_EQUALS => '+',
        KEYCODE_LEFT_BRACKET => '{',
        KEYCODE_RIGHT_BRACKET => '}',
        KEYCODE_BACKSLASH => '|',
        KEYCODE_SEMICOLON => ':',
        KEYCODE_APOSTROPHE => '"',
        KEYCODE_SLASH => '?',
        _ => base,
    })
}

/// Translates an Android `KeyEvent`'s fields into a Rune `KeyEvent`.
///
/// - `key_code`: `event.getKeyCode()`
/// - `meta_state`: `event.getMetaState()`
/// - `unicode_char`: `event.getUnicodeChar()`
/// - `unmodified_char`: `event.getUnicodeChar(0)`
///
/// Returns `None` for system keys Rune does not intercept (Back, Home, Volume,
/// Power), standalone modifier presses, and dead keys (`COMBINING_ACCENT`).
pub(crate) fn map_key_event(
    key_code: i32,
    meta_state: i32,
    unicode_char: i32,
    unmodified_char: i32,
) -> Option<KeyEvent> {
    let mods = map_mods(meta_state);

    if let Some(key) = named_key(key_code) {
        return Some(KeyEvent { key, mods });
    }

    if key_code == KEYCODE_SPACE {
        let ch = if !mods.cmd && !mods.alt && !mods.logo {
            decode_unicode_char(unicode_char).unwrap_or(' ')
        } else {
            ' '
        };
        return Some(KeyEvent {
            key: Key::Text(ch.to_string()),
            mods,
        });
    }

    if mods.cmd {
        let ch = decode_unicode_char(unmodified_char)
            .or_else(|| decode_unicode_char(unicode_char))
            .filter(|c| c.is_ascii() && *c != ' ')
            .map(|c| c.to_ascii_lowercase())
            .or_else(|| us_base_char(key_code))?;
        return Some(KeyEvent {
            key: Key::Command(ch),
            mods,
        });
    }

    // Combining accent (dead key): wait for the composed character instead of emitting text.
    if ((unicode_char as u32) & COMBINING_ACCENT) != 0 {
        return None;
    }

    if let Some(ch) = decode_unicode_char(unicode_char) {
        return Some(KeyEvent {
            key: Key::Text(ch.to_string()),
            mods,
        });
    }

    if let Some(ch) = decode_unicode_char(unmodified_char) {
        return Some(KeyEvent {
            key: Key::Text(ch.to_string()),
            mods,
        });
    }

    let ch = if mods.shift {
        us_shifted_char(key_code)
    } else {
        us_base_char(key_code)
    }?;
    Some(KeyEvent {
        key: Key::Text(ch.to_string()),
        mods,
    })
}

/// Maps an Android `EditorInfo.IME_ACTION_*` code from `InputConnection.performEditorAction`
/// to a Rune `KeyEvent`.
pub(crate) fn map_editor_action(action_code: i32) -> Option<KeyEvent> {
    match action_code {
        IME_ACTION_NONE => None,
        IME_ACTION_UNSPECIFIED
        | IME_ACTION_GO
        | IME_ACTION_SEARCH
        | IME_ACTION_SEND
        | IME_ACTION_DONE => Some(KeyEvent::from(Key::Enter)),
        IME_ACTION_NEXT => Some(KeyEvent::from(Key::Tab)),
        IME_ACTION_PREVIOUS => Some(KeyEvent {
            key: Key::Tab,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
        }),
        _ => None,
    }
}

/// Converts `InputConnection.setComposingText(text, newCursorPosition)` into a Rune `Ime::Preedit`.
///
/// Android defines `newCursorPosition <= 0` as relative to the start of the composing text and
/// `> 0` as relative to the end of the text minus one (so `1` places the caret after the full text).
pub(crate) fn composing_text_event(text: String, new_cursor_position: i32) -> Ime {
    if text.is_empty() {
        return Ime::Preedit(String::new(), None, Vec::new());
    }
    let pos = if new_cursor_position <= 0 { 0 } else { text.len() };
    Ime::Preedit(text, Some((pos, pos)), Vec::new())
}

/// Converts `InputConnection.commitText(text, newCursorPosition)` into a Rune `Ime::Commit`.
pub(crate) fn commit_text_event(text: String) -> Ime {
    Ime::Commit(text)
}

/// Clamps a `deleteSurroundingText` character count to a safe non-negative bound.
pub(crate) fn clamp_delete_count(count: i32) -> usize {
    count.clamp(0, MAX_DELETE_SURROUNDING as i32) as usize
}

/// Applies one translated Android text or key action to `rt`, returning whether the runtime
/// accepted work and needs a frame.
pub(crate) fn apply_text_input(rt: &mut RuntimeCore, action: TextInputAction) -> bool {
    match action {
        TextInputAction::Ime(ev) => {
            rt.ime(ev);
            true
        }
        TextInputAction::FinishCompositionFromIme => {
            rt.finish_composition_from_ime();
            true
        }
        TextInputAction::DeleteSurrounding { before, after } => {
            if before == 0 && after == 0 {
                return false;
            }
            for _ in 0..before {
                rt.key(KeyEvent::from(Key::Backspace));
            }
            for _ in 0..after {
                rt.key(KeyEvent::from(Key::Delete));
            }
            true
        }
        TextInputAction::KeyDown { event, repeat } => {
            if matches!(event.key, Key::Media(_)) && !rt.media_keys() {
                return false;
            }
            if repeat {
                rt.repeat_key(event);
            } else {
                rt.key(event);
            }
            true
        }
    }
}

#[cfg(target_os = "android")]
#[allow(unsafe_code)]
pub(super) mod jni {
    use jni_sys::{JNI_FALSE, JNI_TRUE, JNIEnv, jboolean, jint, jobject, jstring};

    use super::{
        TextInputAction, clamp_delete_count, commit_text_event, composing_text_event,
        map_editor_action, map_key_event,
    };
    use crate::platform::android::{bridge, host};

    pub(in crate::platform::android) unsafe extern "system" fn native_commit_text(
        env: *mut JNIEnv,
        _this: jobject,
        text: jstring,
    ) {
        // SAFETY: `env` and `text` are valid JNI handles on the UI thread.
        let text = unsafe { bridge::from_jstring(env, text) }.unwrap_or_default();
        let _ = host::deliver_text_input(env, TextInputAction::Ime(commit_text_event(text)));
    }

    pub(in crate::platform::android) unsafe extern "system" fn native_set_composing_text(
        env: *mut JNIEnv,
        _this: jobject,
        text: jstring,
        new_cursor_position: jint,
    ) {
        // SAFETY: `env` and `text` are valid JNI handles on the UI thread.
        let text = unsafe { bridge::from_jstring(env, text) }.unwrap_or_default();
        let _ = host::deliver_text_input(
            env,
            TextInputAction::Ime(composing_text_event(text, new_cursor_position)),
        );
    }

    pub(in crate::platform::android) unsafe extern "system" fn native_finish_composing_text(
        env: *mut JNIEnv,
        _this: jobject,
    ) {
        let _ = host::deliver_text_input(env, TextInputAction::FinishCompositionFromIme);
    }

    pub(in crate::platform::android) unsafe extern "system" fn native_delete_surrounding_text(
        env: *mut JNIEnv,
        _this: jobject,
        before: jint,
        after: jint,
    ) {
        let before = clamp_delete_count(before);
        let after = clamp_delete_count(after);
        if before > 0 || after > 0 {
            let _ = host::deliver_text_input(env, TextInputAction::DeleteSurrounding { before, after });
        }
    }

    pub(in crate::platform::android) unsafe extern "system" fn native_editor_action(
        env: *mut JNIEnv,
        _this: jobject,
        action_code: jint,
    ) -> jboolean {
        let Some(event) = map_editor_action(action_code) else {
            return JNI_FALSE;
        };
        if host::deliver_text_input(env, TextInputAction::KeyDown { event, repeat: false }) {
            JNI_TRUE
        } else {
            JNI_FALSE
        }
    }

    pub(in crate::platform::android) unsafe extern "system" fn native_key_down(
        env: *mut JNIEnv,
        _this: jobject,
        key_code: jint,
        meta_state: jint,
        unicode_char: jint,
        unmodified_char: jint,
        repeat: jboolean,
    ) -> jboolean {
        let Some(event) = map_key_event(key_code, meta_state, unicode_char, unmodified_char) else {
            return JNI_FALSE;
        };
        if host::deliver_text_input(
            env,
            TextInputAction::KeyDown {
                event,
                repeat: repeat != JNI_FALSE,
            },
        ) {
            JNI_TRUE
        } else {
            JNI_FALSE
        }
    }

    pub(in crate::platform::android) unsafe extern "system" fn native_key_up(
        env: *mut JNIEnv,
        _this: jobject,
        key_code: jint,
        meta_state: jint,
        unicode_char: jint,
        unmodified_char: jint,
    ) -> jboolean {
        let Some(event) = map_key_event(key_code, meta_state, unicode_char, unmodified_char) else {
            return JNI_FALSE;
        };
        if host::deliver_key_up(env, event) {
            JNI_TRUE
        } else {
            JNI_FALSE
        }
    }
}
