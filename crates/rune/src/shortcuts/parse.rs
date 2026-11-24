use super::ShortcutError;
use std::{fmt, str::FromStr};

pub(super) const CTRL: u8 = 1;
pub(super) const ALT: u8 = 2;
pub(super) const SHIFT: u8 = 4;
pub(super) const SUPER: u8 = 8;
const PRIMARY_NAME: u8 = 16;

/// A validated, layout-independent accelerator. `cmd`/`command`/`logo`/`meta`/`win` are aliases
/// for `super`; `option`/`opt` is `alt`; `mod` (primary) is `super` on macOS and `ctrl`
/// elsewhere, as in a [`Chord`](crate::Chord), and is stored as that key; `plus` is the key
/// that types `+` on a US layout (`equal`); `print` and `prtsc` alias `printscreen` (supported
/// on Windows and Linux; rejected on macOS with `InvalidAccelerator`). Stored keys are physical
/// US positions. Windows resolves these to virtual keys at registration; Wayland's portal may
/// rebind the preferred trigger. Display uses `ctrl+alt+shift+super` order, lowercase keys and
/// normalized aliases.
///
/// ```
/// use rune::Shortcut;
///
/// let capture: Shortcut = "print".parse().unwrap();
/// assert_eq!(capture.to_string(), "printscreen");
/// assert!(capture.require_modifier().is_ok());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub(super) mods: u8,
    pub(super) key: super::PhysicalKeyCode,
}

impl FromStr for Shortcut {
    type Err = ShortcutError;
    fn from_str(input: &str) -> Result<Self, Self::Err> { Self::parse_on(input, crate::KeyPlatform::current()) }
}

impl Shortcut {
    /// `str::parse` with `mod` resolved for `platform`: super on macOS, ctrl elsewhere.
    pub fn parse_on(input: &str, platform: crate::KeyPlatform) -> Result<Self, ShortcutError> {
        let invalid = || ShortcutError::InvalidAccelerator(input.into());
        let mut mods = 0;
        // Names given, to refuse a repeat; "mod" is its own name, so "mod+ctrl" is ⌃⌘ on
        // macOS and Ctrl elsewhere rather than an error on one platform.
        let mut named = 0u8;
        let mut key = None;
        for part in input.split('+') {
            let part = part.trim().to_ascii_lowercase();
            if let Some(m) = crate::input::key_table::modifier_named(&part) {
                let (name, modifier) = match m {
                    crate::ChordMod::Ctrl => (CTRL, CTRL),
                    crate::ChordMod::Alt => (ALT, ALT),
                    crate::ChordMod::Shift => (SHIFT, SHIFT),
                    crate::ChordMod::Super => (SUPER, SUPER),
                    crate::ChordMod::Primary => (PRIMARY_NAME, if platform == crate::KeyPlatform::Mac { SUPER } else { CTRL }),
                };
                if key.is_some() || named & name != 0 { return Err(invalid()); }
                named |= name;
                mods |= modifier;
            } else {
                if key.is_some() { return Err(invalid()); }
                let name = match part.as_str() {
                    "numpadenter" => "enter",
                    "spacebar" => "space",
                    "plus" => "equal",
                    "print" => "printscreen",
                    s => crate::input::key_table::canonical_named_alias(s)
                        .or_else(|| crate::input::key_table::punctuation_name(s))
                        .or_else(|| s.strip_prefix("key").filter(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_lowercase()))
                        .or_else(|| s.strip_prefix("digit").filter(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_digit()))
                        .unwrap_or(s),
                };
                key = Some(super::keys::named(name).ok_or_else(invalid)?);
            }
        }
        Ok(Self { mods, key: key.ok_or_else(invalid)? })
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (bit, name) in [(CTRL, "ctrl"), (ALT, "alt"), (SHIFT, "shift"), (SUPER, "super")] {
            if self.mods & bit != 0 { write!(f, "{name}+")?; }
        }
        f.write_str(self.key_name())
    }
}

#[cfg(any(all(target_os = "linux", feature = "dbus"), test))]
impl Shortcut {
    pub(super) fn keysym_name(&self) -> String {
        match self.key_name() {
            "escape" => "Escape", "enter" => "Return", "tab" => "Tab", "backspace" => "BackSpace",
            "delete" => "Delete", "insert" => "Insert", "home" => "Home", "end" => "End", "pageup" => "Prior",
            "pagedown" => "Next", "left" => "Left", "right" => "Right", "up" => "Up", "down" => "Down",
            "printscreen" => "Print", "quote" => "apostrophe", "backquote" => "grave",
            k if k.starts_with('f') && k.len() > 1 => return k.to_uppercase(),
            k => k,
        }.into()
    }

    pub(super) fn portal_trigger(&self) -> String {
        let mut s = String::new();
        for (bit, name) in [(CTRL, "CTRL"), (ALT, "ALT"), (SHIFT, "SHIFT"), (SUPER, "LOGO")] {
            if self.mods & bit != 0 { s.push_str(name); s.push('+'); }
        }
        s.push_str(&self.keysym_name());
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_aliases_round_trip() {
        for (input, expected) in [
            ("CMD+Shift+Space", "shift+super+space"),
            ("OPTION+Control+Return", "ctrl+alt+enter"),
            ("super+SHIFT+ctrl+ALT+F24", "ctrl+alt+shift+super+f24"),
            (" ctrl + A ", "ctrl+a"), ("esc", "escape"), ("cmd+/", "super+slash"),
            ("KeyK", "k"), ("Digit7", "7"), ("BracketLeft", "bracketleft"), ("Semicolon", "semicolon"),
            ("printscreen", "printscreen"), ("PrintScreen", "printscreen"), ("print", "printscreen"),
            ("PRINT", "printscreen"), ("prtsc", "printscreen"), ("PrtSc", "printscreen"),
            ("ctrl+printscreen", "ctrl+printscreen"), ("alt+print", "alt+printscreen"),
            ("shift+prtsc", "shift+printscreen"), ("super+printscreen", "super+printscreen"),
        ] {
            let shortcut: Shortcut = input.parse().unwrap();
            assert_eq!(shortcut.to_string(), expected);
            assert_eq!(shortcut, expected.parse().unwrap());
        }
    }

    #[test]
    fn printscreen_emits_xkb_print_keysym_and_portal_triggers() {
        for (accel, trigger) in [
            ("printscreen", "Print"),
            ("print", "Print"),
            ("prtsc", "Print"),
            ("ctrl+printscreen", "CTRL+Print"),
            ("alt+printscreen", "ALT+Print"),
            ("shift+print", "SHIFT+Print"),
            ("super+prtsc", "LOGO+Print"),
        ] {
            let shortcut: Shortcut = accel.parse().unwrap();
            assert_eq!(shortcut.keysym_name(), "Print", "{accel}");
            assert_eq!(shortcut.portal_trigger(), trigger, "{accel}");
        }
    }
    #[test]
    fn reject_ambiguous_and_incomplete_accelerators() {
        for input in ["", "cmd", "cmd+", "cmd++a", "cmd+super+a", "a+b", "a+shift", "ctrl+unknown", "F0", "f25", "f01", "é"] {
            assert!(matches!(input.parse::<Shortcut>(), Err(ShortcutError::InvalidAccelerator(_))), "{input}");
        }
    }
}
