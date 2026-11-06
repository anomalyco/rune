//! How a `Chord` reads: canonical text for settings files, and keycaps per platform.
use std::fmt;

use super::chord::{Chord, ChordKey, ChordMod, KeyPlatform};

impl Chord {
    /// How the chord reads here: "⇧⌘T" on macOS, "Ctrl+Shift+T" on Windows and Linux.
    pub fn label(&self) -> String { self.label_on(KeyPlatform::current()) }

    /// How the chord reads on `platform`: macOS glyphs run together in Apple's order
    /// (⌃⌥⇧⌘), elsewhere words join with "+" in Ctrl, Alt, Shift, Win (Super) order.
    pub fn label_on(&self, platform: KeyPlatform) -> String {
        self.keycaps_on(platform).join(if platform == KeyPlatform::Mac { "" } else { "+" })
    }

    /// One label per key, modifiers first, for drawing each as a keycap.
    pub fn keycaps_on(&self, platform: KeyPlatform) -> Vec<String> {
        let mac = platform == KeyPlatform::Mac;
        let mut caps: Vec<String> = self.mods_on(platform).into_iter().map(|m| match (m, platform) {
            (ChordMod::Ctrl, KeyPlatform::Mac) => "⌃",
            (ChordMod::Alt, KeyPlatform::Mac) => "⌥",
            (ChordMod::Shift, KeyPlatform::Mac) => "⇧",
            (ChordMod::Primary | ChordMod::Super, KeyPlatform::Mac) => "⌘",
            (ChordMod::Primary | ChordMod::Ctrl, _) => "Ctrl",
            (ChordMod::Alt, _) => "Alt",
            (ChordMod::Shift, _) => "Shift",
            (ChordMod::Super, KeyPlatform::Windows) => "Win",
            (ChordMod::Super, _) => "Super",
        }.to_owned()).collect();
        caps.push(key_label(self.key(), mac));
        caps
    }
}

fn key_label(key: ChordKey, mac: bool) -> String {
    let word = |glyph: &str, word: &str| if mac { glyph.to_owned() } else { word.to_owned() };
    match key {
        ChordKey::Char(' ') => "Space".into(),
        ChordKey::Char(c) => c.to_uppercase().collect(),
        ChordKey::Enter => word("↩", "Enter"),
        ChordKey::Tab => word("⇥", "Tab"),
        ChordKey::Escape => word("⎋", "Esc"),
        ChordKey::Backspace => word("⌫", "Backspace"),
        ChordKey::Delete => word("⌦", "Delete"),
        ChordKey::Insert => "Insert".into(),
        ChordKey::Home => word("↖", "Home"),
        ChordKey::End => word("↘", "End"),
        ChordKey::PageUp => word("⇞", "Page Up"),
        ChordKey::PageDown => word("⇟", "Page Down"),
        ChordKey::Left => "←".into(),
        ChordKey::Right => "→".into(),
        ChordKey::Up => "↑".into(),
        ChordKey::Down => "↓".into(),
        ChordKey::Function(n) => format!("F{n}"),
        ChordKey::ContextMenu => "Menu".into(),
    }
}

/// The canonical form `parse` reads back: modifiers in mod, ctrl, alt, shift, super order,
/// then the key ("mod+shift+t", "ctrl+tab", "mod+comma").
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (m, name) in [(ChordMod::Primary, "mod"), (ChordMod::Ctrl, "ctrl"), (ChordMod::Alt, "alt"), (ChordMod::Shift, "shift"), (ChordMod::Super, "super")] {
            if self.has(m) { write!(f, "{name}+")?; }
        }
        match self.key() {
            ChordKey::Char(' ') => f.write_str("space"),
            ChordKey::Char('+') => f.write_str("plus"),
            ChordKey::Char(',') => f.write_str("comma"),
            ChordKey::Char(c) => write!(f, "{c}"),
            ChordKey::Enter => f.write_str("enter"),
            ChordKey::Tab => f.write_str("tab"),
            ChordKey::Escape => f.write_str("escape"),
            ChordKey::Backspace => f.write_str("backspace"),
            ChordKey::Delete => f.write_str("delete"),
            ChordKey::Insert => f.write_str("insert"),
            ChordKey::Home => f.write_str("home"),
            ChordKey::End => f.write_str("end"),
            ChordKey::PageUp => f.write_str("pageup"),
            ChordKey::PageDown => f.write_str("pagedown"),
            ChordKey::Left => f.write_str("left"),
            ChordKey::Right => f.write_str("right"),
            ChordKey::Up => f.write_str("up"),
            ChordKey::Down => f.write_str("down"),
            ChordKey::Function(n) => write!(f, "f{n}"),
            ChordKey::ContextMenu => f.write_str("contextmenu"),
        }
    }
}
