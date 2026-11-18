//! Canonical single source of truth for named keys, modifier tables, punctuation key names,
//! and keycap formatting. Shared by winit key mapping, browser DOM key events, headless scripts,
//! key chords, global shortcuts, and the `--keys` overlay.

use winit::keyboard::NamedKey;

use super::chord::{ChordKey, ChordMod, KeyPlatform};
use crate::edit::{Key, Mods};

/// One modifier's parse aliases (canonical first) and keycap labels per platform.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ModSpec {
    pub chord: ChordMod,
    /// Lowercase names, canonical first (`Chord`'s `Display` writes `aliases[0]`).
    pub aliases: &'static [&'static str],
    /// macOS and `--keys` overlay glyph (`⌃`, `⌥`, `⇧`, `⌘`).
    pub glyph: &'static str,
    /// Word label off macOS (`"Ctrl"`, `"Alt"`, `"Shift"`, `"Super"`).
    pub word: &'static str,
    /// Windows `Chord` word label (`"Win"` for Super, otherwise `word`).
    pub win_word: &'static str,
}

/// Modifiers in `Chord`'s canonical `Display` order: `mod`, `ctrl`, `alt`, `shift`, `super`.
pub(crate) const MODS: &[ModSpec] = &[
    ModSpec { chord: ChordMod::Primary, aliases: &["mod", "primary"], glyph: "⌘", word: "Ctrl", win_word: "Ctrl" },
    ModSpec { chord: ChordMod::Ctrl, aliases: &["ctrl", "control"], glyph: "⌃", word: "Ctrl", win_word: "Ctrl" },
    ModSpec { chord: ChordMod::Alt, aliases: &["alt", "option", "opt"], glyph: "⌥", word: "Alt", win_word: "Alt" },
    ModSpec { chord: ChordMod::Shift, aliases: &["shift"], glyph: "⇧", word: "Shift", win_word: "Shift" },
    ModSpec { chord: ChordMod::Super, aliases: &["super", "cmd", "command", "meta", "win", "logo"], glyph: "⌘", word: "Super", win_word: "Win" },
];

/// Metadata for one non-character, non-function named key across every input and display layer.
pub(crate) struct NamedKeySpec {
    pub chord: ChordKey,
    pub key: Key,
    pub winit: NamedKey,
    /// DOM `KeyboardEvent.key` name (e.g. `"ArrowLeft"`, `"Enter"`).
    pub dom_name: &'static str,
    /// Headless script key name (e.g. `"Left"`, `"Enter"`).
    pub script_name: &'static str,
    /// Lowercase chord / shortcut aliases, canonical first.
    pub aliases: &'static [&'static str],
    /// macOS keycap label.
    pub mac_label: &'static str,
    /// Non-macOS `Chord` keycap label.
    pub pc_label: &'static str,
    /// Non-macOS `Shortcut` keycap label (`"Return"` for Enter, else `pc_label`).
    pub shortcut_pc_label: &'static str,
    /// Compact `--keys` overlay cap label.
    pub overlay_label: &'static str,
}

impl NamedKeySpec {
    const fn new(
        chord: ChordKey,
        key: Key,
        winit: NamedKey,
        dom_name: &'static str,
        aliases: &'static [&'static str],
        mac_label: &'static str,
        pc_label: &'static str,
    ) -> Self {
        Self {
            chord,
            key,
            winit,
            dom_name,
            script_name: dom_name,
            aliases,
            mac_label,
            pc_label,
            shortcut_pc_label: pc_label,
            overlay_label: mac_label,
        }
    }

    const fn script(mut self, name: &'static str) -> Self { self.script_name = name; self }
    const fn shortcut_pc(mut self, label: &'static str) -> Self { self.shortcut_pc_label = label; self }
    const fn overlay(mut self, label: &'static str) -> Self { self.overlay_label = label; self }
}

/// Every non-function named key Rune handles.
pub(crate) const NAMED_KEYS: &[NamedKeySpec] = &[
    NamedKeySpec::new(ChordKey::Enter, Key::Enter, NamedKey::Enter, "Enter", &["enter", "return"], "↩", "Enter").shortcut_pc("Return"),
    NamedKeySpec::new(ChordKey::Tab, Key::Tab, NamedKey::Tab, "Tab", &["tab"], "⇥", "Tab"),
    NamedKeySpec::new(ChordKey::Escape, Key::Escape, NamedKey::Escape, "Escape", &["escape", "esc"], "⎋", "Esc"),
    NamedKeySpec::new(ChordKey::Backspace, Key::Backspace, NamedKey::Backspace, "Backspace", &["backspace"], "⌫", "Backspace"),
    NamedKeySpec::new(ChordKey::Delete, Key::Delete, NamedKey::Delete, "Delete", &["delete", "del"], "⌦", "Delete"),
    NamedKeySpec::new(ChordKey::Insert, Key::Insert, NamedKey::Insert, "Insert", &["insert", "ins"], "Insert", "Insert").overlay("Ins"),
    NamedKeySpec::new(ChordKey::Home, Key::Home, NamedKey::Home, "Home", &["home"], "↖", "Home"),
    NamedKeySpec::new(ChordKey::End, Key::End, NamedKey::End, "End", &["end"], "↘", "End"),
    NamedKeySpec::new(ChordKey::PageUp, Key::PageUp, NamedKey::PageUp, "PageUp", &["pageup", "pgup"], "⇞", "Page Up"),
    NamedKeySpec::new(ChordKey::PageDown, Key::PageDown, NamedKey::PageDown, "PageDown", &["pagedown", "pgdn"], "⇟", "Page Down"),
    NamedKeySpec::new(ChordKey::Left, Key::Left, NamedKey::ArrowLeft, "ArrowLeft", &["left", "arrowleft"], "←", "←").script("Left"),
    NamedKeySpec::new(ChordKey::Right, Key::Right, NamedKey::ArrowRight, "ArrowRight", &["right", "arrowright"], "→", "→").script("Right"),
    NamedKeySpec::new(ChordKey::Up, Key::Up, NamedKey::ArrowUp, "ArrowUp", &["up", "arrowup"], "↑", "↑").script("Up"),
    NamedKeySpec::new(ChordKey::Down, Key::Down, NamedKey::ArrowDown, "ArrowDown", &["down", "arrowdown"], "↓", "↓").script("Down"),
    NamedKeySpec::new(ChordKey::ContextMenu, Key::ContextMenu, NamedKey::ContextMenu, "ContextMenu", &["contextmenu", "menu"], "Menu", "Menu").overlay("≣"),
    NamedKeySpec::new(ChordKey::PrintScreen, Key::PrintScreen, NamedKey::PrintScreen, "PrintScreen", &["printscreen", "prtsc"], "Print Screen", "Print Screen").overlay("PrtSc"),
    NamedKeySpec::new(ChordKey::ScrollLock, Key::ScrollLock, NamedKey::ScrollLock, "ScrollLock", &["scrolllock"], "Scroll Lock", "Scroll Lock").overlay("ScrLk"),
    NamedKeySpec::new(ChordKey::Pause, Key::Pause, NamedKey::Pause, "Pause", &["pause", "break"], "Pause", "Pause"),
];

const WINIT_FUNCTION_KEYS: &[NamedKey] = &[
    NamedKey::F1, NamedKey::F2, NamedKey::F3, NamedKey::F4, NamedKey::F5, NamedKey::F6,
    NamedKey::F7, NamedKey::F8, NamedKey::F9, NamedKey::F10, NamedKey::F11, NamedKey::F12,
    NamedKey::F13, NamedKey::F14, NamedKey::F15, NamedKey::F16, NamedKey::F17, NamedKey::F18,
    NamedKey::F19, NamedKey::F20, NamedKey::F21, NamedKey::F22, NamedKey::F23, NamedKey::F24,
];

/// Printable punctuation keys whose stable name differs from their symbol.
pub(crate) const PUNCTUATION: &[(char, &str, &str)] = &[
    ('-', "minus", "-"),
    ('=', "equal", "="),
    (',', "comma", ","),
    ('.', "period", "."),
    ('/', "slash", "/"),
    (';', "semicolon", ";"),
    ('\'', "quote", "'"),
    ('[', "bracketleft", "["),
    (']', "bracketright", "]"),
    ('\\', "backslash", "\\"),
    ('`', "backquote", "`"),
];

// ── Modifiers ────────────────────────────────────────────────────────────────

/// Builds `Mods` from raw platform modifier keys: `cmd` follows Super on macOS, Ctrl on
/// Windows/Linux, and either Super or Ctrl on `wasm32`.
pub(crate) const fn mods_from_keys(shift: bool, ctrl: bool, alt: bool, logo: bool) -> Mods {
    let cmd = if cfg!(target_arch = "wasm32") {
        logo || ctrl
    } else if cfg!(target_os = "macos") {
        logo
    } else {
        ctrl
    };
    Mods { shift, cmd, alt, ctrl, logo }
}

/// Builds `Mods` for browser DOM bridge events, where either Meta/Super or Ctrl is `cmd`
/// regardless of the host target running tests.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const fn web_mods(shift: bool, ctrl: bool, alt: bool, logo: bool) -> Mods {
    Mods { shift, cmd: logo || ctrl, alt, ctrl, logo }
}

/// Builds `Mods` for a headless script step: `"cmd"` sets the platform's primary key alongside
/// `cmd`, so `"cmd": true` matches Super on macOS and Ctrl elsewhere.
pub(crate) const fn mods_from_script(shift: bool, ctrl: bool, alt: bool, logo: bool, cmd: bool) -> Mods {
    let mac = cfg!(target_os = "macos");
    Mods { shift, cmd, alt, ctrl: ctrl || (cmd && !mac), logo: logo || (cmd && mac) }
}

pub(crate) const fn mod_spec(m: ChordMod) -> &'static ModSpec {
    match m {
        ChordMod::Primary => &MODS[0],
        ChordMod::Ctrl => &MODS[1],
        ChordMod::Alt => &MODS[2],
        ChordMod::Shift => &MODS[3],
        ChordMod::Super => &MODS[4],
    }
}

/// Parses a lowercased modifier name (`"mod"`, `"ctrl"`, `"option"`, `"cmd"`, ...).
pub(crate) fn modifier_named(name: &str) -> Option<ChordMod> {
    MODS.iter().find(|m| m.aliases.contains(&name)).map(|m| m.chord)
}

/// A modifier's keycap label for `Chord::keycaps_on(platform)`.
pub(crate) fn modifier_label(m: ChordMod, platform: KeyPlatform) -> &'static str {
    let s = mod_spec(m);
    match platform {
        KeyPlatform::Mac => s.glyph,
        KeyPlatform::Windows => s.win_word,
        KeyPlatform::Linux => s.word,
    }
}

fn held_mod_specs(m: Mods) -> impl Iterator<Item = &'static ModSpec> {
    let mac = cfg!(target_os = "macos");
    [
        (m.ctrl || (!mac && m.cmd), ChordMod::Ctrl),
        (m.alt, ChordMod::Alt),
        (m.shift, ChordMod::Shift),
        (m.logo || (mac && m.cmd), ChordMod::Super),
    ]
    .into_iter()
    .filter_map(|(on, cm)| on.then(|| mod_spec(cm)))
}

/// Apple's order: Control, Option, Shift, Command (`⌃⌥⇧⌘`), for the `--keys` overlay.
/// `cmd` folds into Control off macOS and Command on macOS.
pub(crate) fn modifier_glyphs(m: Mods) -> String {
    held_mod_specs(m).map(|s| s.glyph).collect()
}

/// Keycaps for held modifiers (`rune::modifier_keycaps`): glyphs on macOS, words elsewhere.
pub(crate) fn modifier_keycaps(m: Mods) -> Vec<String> {
    let mac = cfg!(target_os = "macos");
    held_mod_specs(m).map(|s| if mac { s.glyph } else { s.word }.to_owned()).collect()
}

// ── Named & Function Keys ────────────────────────────────────────────────────

/// Parses `"F1"`..`"F24"` (with `prefix = 'F'`) or `"f1"`..`"f24"` (with `prefix = 'f'`),
/// rejecting `"F0"`, `"F01"`, `"F+1"`, and `"F25"`.
pub(crate) fn parse_function_key(s: &str, prefix: char) -> Option<u8> {
    let digits = s.strip_prefix(prefix)?;
    if digits.is_empty() || digits.starts_with(['0', '+']) { return None; }
    let n: u8 = digits.parse().ok()?;
    (1..=24).contains(&n).then_some(n)
}

/// Maps a winit `NamedKey` to a Rune `Key`.
pub(crate) fn key_from_winit(n: NamedKey) -> Option<Key> {
    if n == NamedKey::Space { return Some(Key::Text(" ".into())); }
    if let Some(i) = WINIT_FUNCTION_KEYS.iter().position(|&f| f == n) {
        return Some(Key::Function((i + 1) as u8));
    }
    NAMED_KEYS.iter().find(|k| k.winit == n).map(|k| k.key.clone())
}

/// Maps a DOM `KeyboardEvent.key` or headless script key name (including `"F1"`–`"F24"`) to a
/// Rune `Key`. Case-sensitive (`"Enter"`, `"ArrowLeft"` / `"Left"`, `"F1"`..`"F24"`).
pub(crate) fn named_key_from_str(name: &str) -> Option<Key> {
    if let Some(n) = parse_function_key(name, 'F') {
        return Some(Key::Function(n));
    }
    NAMED_KEYS.iter().find(|k| k.dom_name == name || k.script_name == name).map(|k| k.key.clone())
}

fn named_by_alias(alias: &str) -> Option<&'static NamedKeySpec> {
    NAMED_KEYS.iter().find(|k| k.aliases.contains(&alias))
}

/// Lowercase chord / shortcut alias to `ChordKey` (non-function named keys).
pub(crate) fn chord_key_named(name: &str) -> Option<ChordKey> {
    named_by_alias(name).map(|k| k.chord)
}

/// Converts a named or function `Key` to its `ChordKey`.
pub(crate) fn chord_key_from_key(key: &Key) -> Option<ChordKey> {
    if let Key::Function(n) = key { return Some(ChordKey::Function(*n)); }
    NAMED_KEYS.iter().find(|k| k.key == *key).map(|k| k.chord)
}

/// Canonical lowercase name for a named `ChordKey` (`Chord`'s `Display`).
pub(crate) fn chord_key_canonical(key: ChordKey) -> &'static str {
    NAMED_KEYS.iter().find(|k| k.chord == key).map_or("", |k| k.aliases[0])
}

/// Keycap label for a named `ChordKey` on `platform` (`Chord::keycaps_on`).
pub(crate) fn chord_key_label(key: ChordKey, platform: KeyPlatform) -> &'static str {
    let mac = platform == KeyPlatform::Mac;
    NAMED_KEYS.iter().find(|k| k.chord == key).map_or("", |k| if mac { k.mac_label } else { k.pc_label })
}

/// Normalizes a lowercase named-key alias (`"esc"` → `"escape"`, `"arrowleft"` → `"left"`).
pub(crate) fn canonical_named_alias(alias: &str) -> Option<&'static str> {
    named_by_alias(alias).map(|k| k.aliases[0])
}

/// Keycap label for a `Shortcut`'s stable lowercase named/function key (`"space"`, `"f5"`,
/// `"enter"`, `"left"`, ...). Returns `None` for letters, digits, and punctuation.
pub(crate) fn shortcut_named_label(name: &str) -> Option<String> {
    if name == "space" { return Some("Space".into()); }
    if let Some(n) = parse_function_key(name, 'f') { return Some(format!("F{n}")); }
    let spec = named_by_alias(name)?;
    Some(if cfg!(target_os = "macos") { spec.mac_label } else { spec.shortcut_pc_label }.into())
}

/// Compact cap glyph for a named `Key` in the `--keys` overlay.
pub(crate) fn overlay_key_label(key: &Key) -> Option<&'static str> {
    NAMED_KEYS.iter().find(|k| k.key == *key).map(|k| k.overlay_label)
}

/// Compact cap label for a `Shortcut`'s stable lowercase key name in the `--keys` overlay.
pub(crate) fn overlay_named_label(name: &str) -> String {
    if name == "space" { return "␣".to_owned(); }
    if let Some(spec) = named_by_alias(name) { return spec.overlay_label.to_owned(); }
    if let Some(sym) = punctuation_symbol(name) { return sym.to_owned(); }
    title_case(name)
}

// ── Punctuation & Character Names ────────────────────────────────────────────

/// Parses a lowercased character name in a `Chord` string (`"space"`, `"plus"`, `"minus"`, ...).
pub(crate) fn char_named(name: &str) -> Option<char> {
    match name {
        "space" => Some(' '),
        "plus" => Some('+'),
        _ => PUNCTUATION.iter().find(|p| p.1 == name).map(|p| p.0),
    }
}

/// Canonical name for a character in `Chord`'s `Display` (`' '` → `"space"`, `'+'` → `"plus"`,
/// `','` → `"comma"`, ...).
pub(crate) fn char_name(c: char) -> Option<&'static str> {
    match c {
        ' ' => Some("space"),
        '+' => Some("plus"),
        _ => PUNCTUATION.iter().find(|p| p.0 == c).map(|p| p.1),
    }
}

/// Maps a stable punctuation name (`"minus"`, `"bracketleft"`) to its US symbol (`"-"`, `"["`).
pub(crate) fn punctuation_symbol(name: &str) -> Option<&'static str> {
    PUNCTUATION.iter().find(|p| p.1 == name).map(|p| p.2)
}

/// Maps a US punctuation symbol (`"-"`, `"["`) to its stable name (`"minus"`, `"bracketleft"`).
pub(crate) fn punctuation_name(sym: &str) -> Option<&'static str> {
    PUNCTUATION.iter().find(|p| p.2 == sym).map(|p| p.1)
}

// ── Keycap Capitalization ────────────────────────────────────────────────────

/// Capitalizes a single character for a keycap ("k" → "K"), leaving characters whose uppercase
/// isn't one reversible character ("ß", Turkish "ı") untouched.
fn cap_char(c: char) -> char {
    let mut upper = c.to_uppercase();
    let (Some(u), None) = (upper.next(), upper.next()) else { return c };
    let mut lower = u.to_lowercase();
    if (lower.next(), lower.next()) == (Some(c), None) { u } else { c }
}

/// Capitalizes a single-character keycap label ("k" → "K"); multi-character labels stay as given.
pub(crate) fn capital_keycap(label: &str) -> String {
    let mut chars = label.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => cap_char(c).to_string(),
        _ => label.to_owned(),
    }
}

/// Title-cases a keycap label ("k" → "K", "capslock" → "Capslock", "SPACE" → "Space"),
/// leaving characters whose capital doesn't roundtrip ("ß", Turkish "ı") as typed.
pub(crate) fn title_case(name: &str) -> String {
    let mut chars = name.chars();
    let Some(first) = chars.next() else { return String::new() };
    format!("{}{}", cap_char(first), chars.as_str().to_lowercase())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn tables_have_unique_entries_and_consistent_mappings() {
        let mut mod_aliases = HashSet::new();
        for (i, m) in MODS.iter().enumerate() {
            assert_eq!(mod_spec(m.chord).chord, m.chord);
            assert_eq!(MODS[i].chord, m.chord);
            assert!(!m.aliases.is_empty());
            for &a in m.aliases {
                assert!(mod_aliases.insert(a), "duplicate modifier alias: {a}");
                assert_eq!(modifier_named(a), Some(m.chord));
            }
        }

        let mut key_aliases = HashSet::new();
        let mut chords = HashSet::new();
        let mut winit_keys = HashSet::new();
        for k in NAMED_KEYS {
            assert!(chords.insert(k.chord), "duplicate ChordKey: {:?}", k.chord);
            assert!(winit_keys.insert(k.winit), "duplicate NamedKey: {:?}", k.winit);
            assert_eq!(key_from_winit(k.winit), Some(k.key.clone()));
            assert_eq!(named_key_from_str(k.dom_name), Some(k.key.clone()));
            assert_eq!(named_key_from_str(k.script_name), Some(k.key.clone()));
            assert_eq!(chord_key_from_key(&k.key), Some(k.chord));
            assert_eq!(chord_key_canonical(k.chord), k.aliases[0]);
            assert_eq!(overlay_key_label(&k.key), Some(k.overlay_label));
            assert_eq!(overlay_named_label(k.aliases[0]), k.overlay_label);
            for &a in k.aliases {
                assert!(key_aliases.insert(a), "duplicate key alias: {a}");
                assert_eq!(chord_key_named(a), Some(k.chord));
                assert_eq!(canonical_named_alias(a), Some(k.aliases[0]));
            }
        }

        assert_eq!(WINIT_FUNCTION_KEYS.len(), 24);
        for n in 1..=24u8 {
            let f_upper = format!("F{n}");
            let f_lower = format!("f{n}");
            assert_eq!(key_from_winit(WINIT_FUNCTION_KEYS[(n - 1) as usize]), Some(Key::Function(n)));
            assert_eq!(named_key_from_str(&f_upper), Some(Key::Function(n)));
            assert_eq!(parse_function_key(&f_upper, 'F'), Some(n));
            assert_eq!(parse_function_key(&f_lower, 'f'), Some(n));
            assert_eq!(shortcut_named_label(&f_lower), Some(f_upper));
        }
        for bad in ["F", "F0", "F01", "F+1", "F25", "f1"] {
            assert_eq!(named_key_from_str(bad), None, "should reject {bad:?}");
        }
    }

    #[test]
    fn punctuation_and_keycap_formatting_match_every_platform() {
        for &(ch, name, sym) in PUNCTUATION {
            assert_eq!(char_named(name), Some(ch));
            assert_eq!(char_name(ch), Some(name));
            assert_eq!(punctuation_symbol(name), Some(sym));
            assert_eq!(punctuation_name(sym), Some(name));
            assert_eq!(overlay_named_label(name), sym);
        }
        assert_eq!(char_named("space"), Some(' '));
        assert_eq!(char_named("plus"), Some('+'));
        assert_eq!(char_name(' '), Some("space"));
        assert_eq!(char_name('+'), Some("plus"));

        assert_eq!(modifier_label(ChordMod::Super, KeyPlatform::Mac), "⌘");
        assert_eq!(modifier_label(ChordMod::Super, KeyPlatform::Windows), "Win");
        assert_eq!(modifier_label(ChordMod::Super, KeyPlatform::Linux), "Super");
        assert_eq!(chord_key_label(ChordKey::Enter, KeyPlatform::Mac), "↩");
        assert_eq!(chord_key_label(ChordKey::Enter, KeyPlatform::Windows), "Enter");
        assert_eq!(shortcut_named_label("enter").as_deref(), Some(if cfg!(target_os = "macos") { "↩" } else { "Return" }));
        assert_eq!(overlay_named_label("space"), "␣");
        assert_eq!(overlay_named_label("insert"), "Ins");
        assert_eq!(overlay_named_label("contextmenu"), "≣");

        assert_eq!(capital_keycap("k"), "K");
        assert_eq!(capital_keycap("K"), "K");
        assert_eq!(capital_keycap("ß"), "ß");
        assert_eq!(capital_keycap("ı"), "ı");
        assert_eq!(capital_keycap("/"), "/");
        assert_eq!(capital_keycap("Space"), "Space");
        assert_eq!(title_case("k"), "K");
        assert_eq!(title_case("ß"), "ß");
        assert_eq!(title_case("ı"), "ı");
        assert_eq!(title_case("capslock"), "Capslock");
        assert_eq!(title_case("SPACE"), "Space");
        assert_eq!(title_case(""), "");
    }
}
