//! winit keys and modifiers as Rune's.
use winit::keyboard::{Key as WKey, ModifiersState, NativeKeyCode, PhysicalKey};

use crate::edit::{Key, KeyEvent, Mods};

pub(crate) fn map_mods(m: ModifiersState) -> Mods {
    crate::input::key_table::mods_from_keys(m.shift_key(), m.control_key(), m.alt_key(), m.super_key())
}

#[cfg(test)]
pub(crate) fn map_key(key: &WKey, m: ModifiersState) -> Option<KeyEvent> {
    map_key_with_mods(cfg!(target_os = "windows"), key, map_mods(m))
}

fn map_key_with_mods(windows: bool, key: &WKey, mods: Mods) -> Option<KeyEvent> {
    let key = match key {
        WKey::Named(n) => crate::input::key_table::key_from_winit(*n)?,
        // Windows treats Left Ctrl + Left Alt as AltGr: a character typed with both is text
        // ("@" on a German layout's Q), not a chord. A chord that types nothing comes through
        // `map_key_event`'s fallback instead.
        WKey::Character(c) if mods.cmd && c != " " && c.chars().count() == 1 && !(windows && mods.ctrl && mods.alt) => {
            Key::Command(c.chars().next()?.to_ascii_lowercase())
        }
        WKey::Character(c) => Key::Text(c.to_string()),
        _ => return None,
    };
    Some(KeyEvent { key, mods })
}

/// Rewrites a non-ASCII command chord (`Ctrl+М`, `Ctrl+Ö`) to the US character at `physical`,
/// matching Windows, Linux and web shortcut conventions.
fn remap_non_ascii_command(ev: &mut KeyEvent, physical: PhysicalKey) {
    if let Key::Command(c) = ev.key
        && !c.is_ascii()
        && let winit::keyboard::PhysicalKey::Code(code) = physical
        && let Some(us) = crate::shortcuts::us_char(code)
    {
        ev.key = Key::Command(us);
    }
}

/// `map_key` for a window's key event. Windows names no logical key for a chord whose
/// modifiers type nothing on the layout, such as Ctrl+Alt+B on US English (Ctrl+Alt is AltGr
/// there); it still arrives as a chord on the character the key types unmodified. Off macOS, a
/// chord on a key that types a non-ASCII character (Ctrl+М on a Russian layout, Ctrl+Ü on a
/// German one) arrives as the US key in its place, `Command('v')`, as Windows' own shortcuts
/// follow the key's Latin virtual key and macOS's switch to a Latin layout under ⌘: Ctrl+V
/// pastes on every layout.
pub(crate) fn map_key_event(event: &winit::event::KeyEvent, m: ModifiersState) -> Option<KeyEvent> {
    let mut ev = map_parts(cfg!(target_os = "windows"), event.physical_key, &event.logical_key, typed_text(event), m)
        .or_else(|| unidentified(event, m))?;
    if !cfg!(target_os = "macos") {
        remap_non_ascii_command(&mut ev, event.physical_key);
    }
    Some(ev)
}

fn unidentified(event: &winit::event::KeyEvent, m: ModifiersState) -> Option<KeyEvent> {
    let WKey::Unidentified(_) = event.logical_key else { return None };
    let mods = map_mods(m);
    // Ctrl+Pause is Break, which Windows names `VK_CANCEL` and winit leaves unnamed: it arrives as
    // Ctrl+Pause, so a terminal can send Ctrl+Break.
    if event.physical_key == winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::Pause) {
        return Some(KeyEvent { key: Key::Pause, mods });
    }
    if !(mods.cmd || mods.ctrl) { return None; }
    let c = key_base(event, m).filter(|c| !c.is_control() && *c != ' ')?;
    let key = if mods.cmd { Key::Command(c.to_ascii_lowercase()) } else { Key::Text(c.to_string()) };
    Some(KeyEvent { key, mods })
}

/// The physical key Rune's winit patch gives text that arrived with no key behind it on Windows:
/// a bare `WM_CHAR`, or a `VK_PACKET` from `SendInput(KEYEVENTF_UNICODE)` (the emoji panel, voice
/// typing, the on-screen keyboard, text expanders, password managers). Windows never names a real
/// key this way: one with an unknown scan code is `NativeKeyCode::Windows(_)`.
const NO_KEY: PhysicalKey = PhysicalKey::Unidentified(NativeKeyCode::Unidentified);

/// Picks the typed text from winit's `text_with_all_modifiers()` and `text`: prefers the layout's
/// modifier-supplement text when it is non-empty and carries no control characters, falling back
/// to `text` so Ctrl+letter chords (whose supplement is a control byte such as `"\x01"`) keep the
/// key they name.
pub(crate) fn pick_typed_text<'a>(with_all_mods: Option<&'a str>, text: Option<&'a str>) -> Option<&'a str> {
    with_all_mods
        .filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
        .or(text)
}

/// The text `event`'s key typed under the current layout and modifiers.
///
/// `KeyEvent::text` is the logical key's text (`logical_key.to_text()` in winit), not what the
/// layout produced: a key winit names from its keycode reports the name's character. Shift+Space
/// on a Persian layout types ZWNJ (U+200C), while `text` stays `" "` and AppKit's character sits
/// in `text_with_all_modifiers()`.
fn typed_text(event: &winit::event::KeyEvent) -> Option<&str> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    let with_all_mods = {
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        event.text_with_all_modifiers()
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let with_all_mods = None;
    pick_typed_text(with_all_mods, event.text.as_deref())
}

/// `map_key`, with three text-carrying adjustments:
/// - On Windows, text with no key behind it (`NO_KEY`) types as itself with no modifiers, even
///   while a hotkey's modifier is still held.
/// - A second dead key (`WKey::Dead(_)`) carrying printable text (such as `'` then `'` -> `''`
///   on Windows) types that text unless a chord modifier is held; a first dead key carries no
///   text and still returns `None`.
/// - When a non-chord key maps to `Key::Text` (such as `NamedKey::Space`) and the layout typed
///   different non-control text (such as ZWNJ U+200C for Shift+Space on a Persian layout, or
///   NBSP U+00A0 for Option+Space on macOS), keep the typed text instead of the named key's
///   default character.
fn map_parts(windows: bool, physical: PhysicalKey, logical: &WKey, text: Option<&str>, m: ModifiersState) -> Option<KeyEvent> {
    map_parts_with_mods(windows, physical, logical, text, map_mods(m), m.super_key())
}

fn map_parts_with_mods(windows: bool, physical: PhysicalKey, logical: &WKey, text: Option<&str>, mods: Mods, super_key: bool) -> Option<KeyEvent> {
    let text = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control));
    if windows
        && physical == NO_KEY
        && matches!(logical, WKey::Character(_) | WKey::Unidentified(_))
        && let Some(text) = text
    {
        return Some(KeyEvent { key: Key::Text(text.to_owned()), mods: Mods::default() });
    }
    if matches!(logical, WKey::Dead(_))
        && !super_key
        && let Some(text) = text
    {
        let ev = KeyEvent { key: Key::Text(text.to_owned()), mods };
        return (!ev.is_command_chord()).then_some(ev);
    }
    let mut ev = map_key_with_mods(windows, logical, mods)?;
    if !super_key
        && !ev.is_command_chord()
        && let Some(text) = text
        && let Key::Text(t) = &mut ev.key
        && text != t.as_str()
    {
        crate::el::wipe_string(t);
        *t = text.to_owned();
    }
    Some(ev)
}

/// The platform's text for a key press, for `PhysicalInput::Key::text`: the text the layout
/// typed ([`typed_text`], so Shift+Space on a Persian layout is ZWNJ, not the logical key's
/// `" "`), `None` when it is empty and for a release. A Ctrl chord keeps the key's own text
/// (`"c"` for Ctrl+C, not `"\x03"`); Enter's `"\r"` and other keys' control text stay.
pub(crate) fn platform_text(event: &winit::event::KeyEvent) -> Option<String> {
    if event.state != winit::event::ElementState::Pressed { return None; }
    typed_text(event).filter(|t| !t.is_empty()).map(str::to_owned)
}

/// The modifiers the platform spent producing `text` ([`platform_text`]), for
/// `PhysicalInput::Key::consumed_mods`, measured against the key's unmodified text.
pub(crate) fn consumed_mods(event: &winit::event::KeyEvent, text: Option<&str>, mods: Mods) -> Mods {
    consumed_from(cfg!(target_os = "windows"), mods, text, unmodified_text(event).as_deref())
}

/// `unmodified` is the key's text with no modifiers held, `None` when the host doesn't know it;
/// then any text counts as differing. Alt is spent when the text differs from it, Shift when it
/// does and Ctrl is not held. On Windows, Ctrl+Alt that typed text is AltGr: Ctrl (with `cmd`,
/// which is Ctrl there) and Alt are spent, and Shift too when it is held and the text differs.
/// `logo` is never spent, and nothing is without text or for control text (Enter's `"\r"`,
/// Tab's `"\t"`, Backspace, Escape, Delete), so Alt+Enter keeps Alt and Shift+Tab keeps Shift.
pub(crate) fn consumed_from(windows: bool, mods: Mods, text: Option<&str>, unmodified: Option<&str>) -> Mods {
    let Some(text) = text.filter(|t| !t.chars().any(char::is_control)) else { return Mods::default() };
    let differs = unmodified != Some(text);
    if windows && mods.ctrl && mods.alt {
        return Mods { ctrl: true, cmd: mods.cmd, alt: true, shift: mods.shift && differs, logo: false };
    }
    Mods { alt: mods.alt && differs, shift: mods.shift && !mods.ctrl && differs, ..Mods::default() }
}

/// The text `event`'s key types with no modifiers, as [`key_base`] reads it.
fn unmodified_text(event: &winit::event::KeyEvent) -> Option<String> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        use winit::keyboard::NamedKey;
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        match event.key_without_modifiers() {
            WKey::Character(s) => Some(s.to_string()),
            WKey::Named(NamedKey::Space) => Some(" ".to_owned()),
            _ => None,
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = event;
        None
    }
}

/// The character `event`'s key types under the current layout with no modifiers, for
/// `Tx::key_base`.
pub(crate) fn key_base(event: &winit::event::KeyEvent, m: ModifiersState) -> Option<char> {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        let _ = m;
        use winit::keyboard::NamedKey;
        use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
        match event.key_without_modifiers() {
            WKey::Character(s) => { let mut c = s.chars(); c.next().filter(|_| c.next().is_none()) }
            WKey::Named(NamedKey::Space) => Some(' '),
            _ => None,
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        web_key_base(&event.logical_key, map_mods(m))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_arch = "wasm32")))]
    {
        let _ = (event, m);
        None
    }
}

/// Derives `Tx::key_base()` from a browser `KeyboardEvent.key` (`WKey`) and its held modifiers.
///
/// Browsers expose no synchronous `key_without_modifiers`:
/// - `Space` always maps to `' '` (a composed Space such as macOS Option+Space arrives as
///   `"\u{00a0}"`, not `NamedKey::Space`).
/// - With Alt/Option/AltGr held, `key` may be a composed character (`"å"`, `"@"`, `"€"`), so
///   return `None`.
/// - Without Alt, an unshifted single non-control character is the key's unmodified character,
///   lowercased so CapsLock still yields `'a'` / `'ö'` while unshifted punctuation stays intact.
/// - With Shift (and no Alt), only a single alphabetic character is accepted and lowercased
///   (`'A'` -> `'a'`, `'Ö'` -> `'ö'`, `'М'` -> `'м'`), leaving shifted punctuation and digits
///   (`'+'` from `Shift+=`, `'{'` from `Shift+[`, `'/'` from German `Shift+7`, `'¿'` from
///   Spanish `Shift+¡`) as `None` so `Chord::matches_tx` never mistakes them for unmodified keys.
/// - Dead keys, named keys, control characters, and multi-character strings return `None`.
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn web_key_base(logical: &WKey, mods: Mods) -> Option<char> {
    match logical {
        WKey::Named(winit::keyboard::NamedKey::Space) => Some(' '),
        WKey::Character(s) => web_char_base(s, mods),
        _ => None,
    }
}

/// Same rule as [`web_key_base`] for a raw DOM `KeyboardEvent.key` string (`bridge.js` -> `web_key`).
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) fn web_key_base_str(name: &str, mods: Mods) -> Option<char> {
    if name == " " {
        return Some(' ');
    }
    web_char_base(name, mods)
}

#[cfg(any(test, target_arch = "wasm32"))]
fn web_char_base(s: &str, mods: Mods) -> Option<char> {
    if mods.alt {
        return None;
    }
    let mut chars = s.chars();
    let c = chars.next().filter(|_| chars.next().is_none())?;
    if c.is_control() {
        return None;
    }
    if mods.shift && !c.is_alphabetic() {
        return None;
    }
    Some(crate::input::chord::lower(c))
}

#[cfg(test)]
pub(super) fn map_web_winit_key(
    physical: PhysicalKey,
    logical: &WKey,
    text: Option<&str>,
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
) -> (Option<KeyEvent>, Option<char>) {
    let mods = crate::input::key_table::web_mods(shift, ctrl, alt, meta);
    let mut ev = map_parts_with_mods(false, physical, logical, text, mods, meta);
    if let Some(ev) = &mut ev {
        remap_non_ascii_command(ev, physical);
    }
    (ev, web_key_base(logical, mods))
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod layout_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::NamedKey;

    #[test]
    fn keys_that_type_nothing_reach_handlers() {
        for (named, key) in [
            (NamedKey::ScrollLock, Key::ScrollLock), (NamedKey::Pause, Key::Pause), (NamedKey::ContextMenu, Key::ContextMenu),
            (NamedKey::PrintScreen, Key::PrintScreen), (NamedKey::Insert, Key::Insert),
            (NamedKey::MediaPlayPause, Key::Media(crate::MediaKey::PlayPause)),
            (NamedKey::MediaPlay, Key::Media(crate::MediaKey::Play)),
            (NamedKey::MediaPause, Key::Media(crate::MediaKey::Pause)),
            (NamedKey::MediaTrackNext, Key::Media(crate::MediaKey::Next)),
            (NamedKey::MediaTrackPrevious, Key::Media(crate::MediaKey::Previous)),
            (NamedKey::MediaStop, Key::Media(crate::MediaKey::Stop)),
            (NamedKey::MediaFastForward, Key::Media(crate::MediaKey::FastForward)),
            (NamedKey::MediaRewind, Key::Media(crate::MediaKey::Rewind)),
        ] {
            assert_eq!(map_key(&WKey::Named(named), ModifiersState::empty()).map(|ev| ev.key.clone()), Some(key));
        }
    }

    /// A real key Windows sent with scan code 0 that winit could not name (an automation script's
    /// `keybd_event(vk, 0, …)`, some remote-control and accessibility tools).
    const SCAN_ZERO: PhysicalKey = PhysicalKey::Unidentified(NativeKeyCode::Windows(0));

    fn ch(s: &str) -> WKey {
        WKey::Character(s.into())
    }

    #[test]
    fn text_with_no_key_types_itself_without_modifiers() {
        for text in ["a", "é", "日本", "😀", "e\u{301}"] {
            for m in [ModifiersState::empty(), ModifiersState::CONTROL, ModifiersState::ALT, ModifiersState::CONTROL | ModifiersState::SHIFT] {
                let ev = map_parts(true, NO_KEY, &ch(text), Some(text), m);
                assert_eq!(ev, Some(KeyEvent { key: Key::Text(text.into()), mods: Mods::default() }), "{text:?} {m:?}");
            }
        }
    }

    #[test]
    fn a_control_character_with_no_key_falls_through_to_the_old_path() {
        // winit names such a VK_PACKET's key `Unidentified`, as before this change; a bare WM_CHAR
        // control character makes no event at all.
        for text in ["\u{8}", "\t", "\n", "\r", "\u{1b}", "\u{7f}", "\u{85}", "a\r"] {
            for logical in [WKey::Unidentified(winit::keyboard::NativeKey::Windows(0xe7)), ch(text)] {
                for m in [ModifiersState::empty(), ModifiersState::CONTROL] {
                    assert_eq!(map_parts(true, NO_KEY, &logical, Some(text), m), map_key(&logical, m), "{text:?} {logical:?} {m:?}");
                }
            }
        }
        assert_eq!(map_parts(true, NO_KEY, &ch("a"), None, ModifiersState::empty()), map_key(&ch("a"), ModifiersState::empty()));
    }

    #[test]
    fn real_keys_with_scan_code_zero_keep_the_old_path() {
        // Ctrl+C: the logical key is the character, the text is ETX.
        let copy = map_parts(true, SCAN_ZERO, &ch("c"), Some("\u{3}"), ModifiersState::CONTROL);
        assert_eq!(copy, map_key(&ch("c"), ModifiersState::CONTROL));
        // Alt+F: WM_SYSKEYDOWN + WM_SYSCHAR, the text is "f".
        let alt_f = map_parts(true, SCAN_ZERO, &ch("f"), Some("f"), ModifiersState::ALT);
        assert_eq!(alt_f, map_key(&ch("f"), ModifiersState::ALT));
        // A plain "a".
        let a = map_parts(true, SCAN_ZERO, &ch("a"), Some("a"), ModifiersState::empty());
        assert_eq!(a, map_key(&ch("a"), ModifiersState::empty()));
        if cfg!(target_os = "windows") {
            assert_eq!(copy, Some(KeyEvent { key: Key::Command('c'), mods: map_mods(ModifiersState::CONTROL) }));
            assert_eq!(alt_f, Some(KeyEvent { key: Key::Text("f".into()), mods: map_mods(ModifiersState::ALT) }));
            assert_eq!(a, Some(KeyEvent { key: Key::Text("a".into()), mods: Mods::default() }));
        }
    }

    #[test]
    fn text_with_no_key_is_only_special_on_windows() {
        let m = ModifiersState::CONTROL;
        assert_eq!(map_parts(false, NO_KEY, &ch("a"), Some("a"), m), map_key(&ch("a"), m));
    }
}
