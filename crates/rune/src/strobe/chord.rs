//! Keys as a keyboard sends them: `press` chords (`"Control+Shift+D"`, `"ControlOrMeta+a"`)
//! and `type` characters, each as the physical key, the logical key a US layout gives it, the
//! character it types unmodified (`Tx::key_base`) and the modifiers held.

use winit::keyboard::{Key as WKey, KeyCode, ModifiersState, NamedKey};

/// One key with the modifiers held while it goes down.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stroke {
    /// None for a character no key types: it arrives as text, as an IME or a paste-like
    /// keyboard sends it.
    pub code: Option<KeyCode>,
    pub key: WKey,
    pub base: Option<char>,
    pub mods: ModifiersState,
}

/// `press`'s chord. `ControlOrMeta` is Cmd on macOS and Ctrl elsewhere; `Meta` is the logo key
/// (Cmd on macOS). Names and aliases are `Shortcut`'s.
pub(crate) fn chord(input: &str) -> Result<Stroke, String> {
    let primary = if cfg!(target_os = "macos") { "cmd" } else { "ctrl" };
    let parts: Vec<&str> = input.split('+').map(|p| if p.trim().eq_ignore_ascii_case("controlormeta") { primary } else { p }).collect();
    // "Control+Shift++" names the plus key.
    let joined = if input.ends_with("++") { format!("{}+equal", parts[..parts.len() - 2].join("+")) } else { parts.join("+") };
    let shortcut: crate::Shortcut = joined.parse().map_err(|e: crate::ShortcutError| e.to_string())?;
    let m = shortcut.modifiers();
    let mut mods = ModifiersState::empty();
    mods.set(ModifiersState::SHIFT, m.shift);
    mods.set(ModifiersState::CONTROL, m.ctrl);
    mods.set(ModifiersState::ALT, m.alt);
    mods.set(ModifiersState::SUPER, m.logo);
    let code = shortcut.physical_key();
    let (key, base) = logical(code, m.shift);
    Ok(Stroke { code: Some(code), key, base, mods })
}

/// The keystroke a US keyboard types `c` with.
pub(crate) fn typed(c: char) -> Stroke {
    let shifted = |code, base| Stroke { code: Some(code), key: WKey::Character(c.to_string().into()), base: Some(base), mods: ModifiersState::SHIFT };
    let plain = |code| Stroke { code: Some(code), key: WKey::Character(c.to_string().into()), base: Some(c), mods: ModifiersState::empty() };
    let named = |code, named| Stroke { code: Some(code), key: WKey::Named(named), base: None, mods: ModifiersState::empty() };
    match c {
        '\n' | '\r' => named(KeyCode::Enter, NamedKey::Enter),
        '\t' => named(KeyCode::Tab, NamedKey::Tab),
        ' ' => Stroke { code: Some(KeyCode::Space), key: WKey::Named(NamedKey::Space), base: Some(' '), mods: ModifiersState::empty() },
        _ => {
            if let Some((code, base, shift)) = us_key(c) {
                if shift { shifted(code, base) } else { plain(code) }
            } else {
                Stroke { code: None, key: WKey::Character(c.to_string().into()), base: None, mods: ModifiersState::empty() }
            }
        }
    }
}

/// The logical key `code` gives on a US layout, and the character it types unmodified.
fn logical(code: KeyCode, shift: bool) -> (WKey, Option<char>) {
    use KeyCode as K;
    let named = match code {
        K::Enter | K::NumpadEnter => Some(NamedKey::Enter),
        K::Escape => Some(NamedKey::Escape),
        K::Tab => Some(NamedKey::Tab),
        K::Backspace => Some(NamedKey::Backspace),
        K::Delete => Some(NamedKey::Delete),
        K::Insert => Some(NamedKey::Insert),
        K::Home => Some(NamedKey::Home),
        K::End => Some(NamedKey::End),
        K::PageUp => Some(NamedKey::PageUp),
        K::PageDown => Some(NamedKey::PageDown),
        K::ArrowLeft => Some(NamedKey::ArrowLeft),
        K::ArrowRight => Some(NamedKey::ArrowRight),
        K::ArrowUp => Some(NamedKey::ArrowUp),
        K::ArrowDown => Some(NamedKey::ArrowDown),
        K::ContextMenu => Some(NamedKey::ContextMenu),
        K::F1 => Some(NamedKey::F1), K::F2 => Some(NamedKey::F2), K::F3 => Some(NamedKey::F3), K::F4 => Some(NamedKey::F4),
        K::F5 => Some(NamedKey::F5), K::F6 => Some(NamedKey::F6), K::F7 => Some(NamedKey::F7), K::F8 => Some(NamedKey::F8),
        K::F9 => Some(NamedKey::F9), K::F10 => Some(NamedKey::F10), K::F11 => Some(NamedKey::F11), K::F12 => Some(NamedKey::F12),
        K::F13 => Some(NamedKey::F13), K::F14 => Some(NamedKey::F14), K::F15 => Some(NamedKey::F15), K::F16 => Some(NamedKey::F16),
        K::F17 => Some(NamedKey::F17), K::F18 => Some(NamedKey::F18), K::F19 => Some(NamedKey::F19), K::F20 => Some(NamedKey::F20),
        K::F21 => Some(NamedKey::F21), K::F22 => Some(NamedKey::F22), K::F23 => Some(NamedKey::F23), K::F24 => Some(NamedKey::F24),
        _ => None,
    };
    if let Some(named) = named { return (WKey::Named(named), None); }
    if code == K::Space { return (WKey::Named(NamedKey::Space), Some(' ')); }
    match US.iter().find(|(k, ..)| *k == code) {
        Some(&(_, base, upper)) => {
            let c = if shift { upper } else { base };
            (WKey::Character(c.to_string().into()), Some(base))
        }
        None => (WKey::Unidentified(winit::keyboard::NativeKey::Unidentified), None),
    }
}

/// A character's US key: the code, the character it types unshifted, and whether Shift types `c`.
fn us_key(c: char) -> Option<(KeyCode, char, bool)> {
    US.iter().find_map(|&(code, base, upper)| {
        if c == base { Some((code, base, false)) } else if c == upper { Some((code, base, true)) } else { None }
    })
}

/// US keys: code, unshifted and shifted characters.
const US: &[(KeyCode, char, char)] = {
    use KeyCode as K;
    &[
        (K::KeyA, 'a', 'A'), (K::KeyB, 'b', 'B'), (K::KeyC, 'c', 'C'), (K::KeyD, 'd', 'D'), (K::KeyE, 'e', 'E'),
        (K::KeyF, 'f', 'F'), (K::KeyG, 'g', 'G'), (K::KeyH, 'h', 'H'), (K::KeyI, 'i', 'I'), (K::KeyJ, 'j', 'J'),
        (K::KeyK, 'k', 'K'), (K::KeyL, 'l', 'L'), (K::KeyM, 'm', 'M'), (K::KeyN, 'n', 'N'), (K::KeyO, 'o', 'O'),
        (K::KeyP, 'p', 'P'), (K::KeyQ, 'q', 'Q'), (K::KeyR, 'r', 'R'), (K::KeyS, 's', 'S'), (K::KeyT, 't', 'T'),
        (K::KeyU, 'u', 'U'), (K::KeyV, 'v', 'V'), (K::KeyW, 'w', 'W'), (K::KeyX, 'x', 'X'), (K::KeyY, 'y', 'Y'),
        (K::KeyZ, 'z', 'Z'),
        (K::Digit1, '1', '!'), (K::Digit2, '2', '@'), (K::Digit3, '3', '#'), (K::Digit4, '4', '$'), (K::Digit5, '5', '%'),
        (K::Digit6, '6', '^'), (K::Digit7, '7', '&'), (K::Digit8, '8', '*'), (K::Digit9, '9', '('), (K::Digit0, '0', ')'),
        (K::Minus, '-', '_'), (K::Equal, '=', '+'), (K::BracketLeft, '[', '{'), (K::BracketRight, ']', '}'),
        (K::Backslash, '\\', '|'), (K::Semicolon, ';', ':'), (K::Quote, '\'', '"'), (K::Backquote, '`', '~'),
        (K::Comma, ',', '<'), (K::Period, '.', '>'), (K::Slash, '/', '?'),
    ]
};
