//! How a `Chord` reads: canonical text for settings files, and keycaps per platform.
//! Driven by the canonical modifier, key and punctuation tables in `input::key_table`.
use std::fmt;

use super::chord::{Chord, ChordKey, KeyPlatform};
use super::key_table;

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
        let mut caps: Vec<String> = self.mods_on(platform).into_iter()
            .map(|m| key_table::modifier_label(m, platform).to_owned())
            .collect();
        caps.push(match self.key() {
            ChordKey::Char(' ') => "Space".into(),
            ChordKey::Char(c) => key_table::capital_keycap(&c.to_string()),
            ChordKey::Function(n) => format!("F{n}"),
            key => key_table::chord_key_label(key, platform).to_owned(),
        });
        caps
    }
}

/// The canonical form `parse` reads back: modifiers in mod, ctrl, alt, shift, super order,
/// then the key ("mod+shift+t", "ctrl+tab", "mod+comma").
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for m in key_table::MODS {
            if self.has(m.chord) { write!(f, "{}+", m.aliases[0])?; }
        }
        match self.key() {
            ChordKey::Char(c @ (' ' | '+' | ',')) => f.write_str(key_table::char_name(c).unwrap_or("")),
            ChordKey::Char(c) => write!(f, "{c}"),
            ChordKey::Function(n) => write!(f, "f{n}"),
            key => f.write_str(key_table::chord_key_canonical(key)),
        }
    }
}
