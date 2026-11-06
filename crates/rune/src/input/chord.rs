//! In-app key chords: "mod+shift+t" as data, matched against a `KeyEvent` and drawn per
//! platform. `mod` is the platform's primary modifier (⌘ on macOS, Ctrl elsewhere); `ctrl`,
//! `alt`, `shift` and `super` name keys. Both sides of a match resolve to the keys held on
//! the platform, so "mod+b" and "ctrl+b" are the same chord on Windows and Linux, and
//! different ones (⌘B and ⌃B) on macOS. `Keybind` adds alternatives and per-platform chords.
use std::{fmt, str::FromStr};

use crate::{Key, KeyEvent, Mods};

const PRIMARY: u8 = 1;
const CTRL: u8 = 2;
const ALT: u8 = 4;
const SHIFT: u8 = 8;
const SUPER: u8 = 16;

/// The platform whose keyboard conventions a chord resolves and draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyPlatform {
    /// ⌘ is the primary modifier; Control is a separate key. Glyph labels: "⇧⌘T".
    Mac,
    /// Ctrl is the primary modifier. Word labels: "Ctrl+Shift+T", the logo key is "Win".
    Windows,
    /// Ctrl is the primary modifier. Word labels, the logo key is "Super". The web uses this
    /// too: there `Mods::cmd` reports Ctrl or ⌘, so "mod" chords answer to both.
    Linux,
}

impl KeyPlatform {
    /// The platform this build runs on.
    pub const fn current() -> Self {
        if cfg!(any(target_os = "macos", target_os = "ios")) { return Self::Mac; }
        if cfg!(target_os = "windows") { return Self::Windows; }
        Self::Linux
    }

    fn mac(self) -> bool { matches!(self, Self::Mac) }
}

/// One modifier as a chord names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChordMod {
    /// "mod": ⌘ on macOS, Ctrl elsewhere.
    Primary,
    /// "ctrl": the Control key. Off macOS it is the primary modifier, so it equals `Primary`.
    Ctrl,
    /// "alt": Option on macOS.
    Alt,
    Shift,
    /// "super": the logo key (Windows key, Super); ⌘ on macOS, where it equals `Primary`.
    Super,
}

impl ChordMod {
    const fn bit(self) -> u8 {
        match self { Self::Primary => PRIMARY, Self::Ctrl => CTRL, Self::Alt => ALT, Self::Shift => SHIFT, Self::Super => SUPER }
    }
}

/// The key a chord ends in. A character is the one the key types without Shift ('t', '[',
/// '1', ','), lowercase; name the unshifted key and add `shift` ("mod+shift+[", not "mod+{").
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChordKey {
    Char(char),
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    /// F1 through F24.
    Function(u8),
    ContextMenu,
}

/// A key with modifiers, as an app binds it: `Chord::parse("mod+shift+t")`. Parsing is
/// case-insensitive; modifiers are `mod` (alias `primary`), `ctrl` (`control`), `alt`
/// (`option`, `opt`), `shift` and `super` (`cmd`, `command`, `meta`, `win`, `logo`: the
/// literal ⌘ or logo key, which is rarely what a cross-platform binding wants). Keys are a
/// character or a name: `enter` (`return`), `tab`, `escape` (`esc`), `backspace`, `delete`
/// (`del`), `insert`, `home`, `end`, `pageup`, `pagedown`, `left`/`right`/`up`/`down`
/// (`arrowleft`, …), `space`, `plus`, `comma`, `f1`–`f24`, `contextmenu` (`menu`).
/// `Display` writes the canonical form back ("mod+shift+t"), for settings files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    mods: u8,
    key: ChordKey,
}

/// A chord or keybind string that doesn't parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordError(String);

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "invalid key chord: {}", self.0) }
}

impl std::error::Error for ChordError {}

impl Chord {
    /// The key alone; add modifiers with `with`.
    pub const fn new(key: ChordKey) -> Self { Self { mods: 0, key } }

    /// This chord with `m` held too.
    pub const fn with(self, m: ChordMod) -> Self { Self { mods: self.mods | m.bit(), key: self.key } }

    pub fn parse(s: &str) -> Result<Self, ChordError> { s.parse() }

    pub fn key(&self) -> ChordKey { self.key }

    /// Whether the chord names `m`, as written (before platform resolution).
    pub fn has(&self, m: ChordMod) -> bool { self.mods & m.bit() != 0 }

    /// Whether `ev` is this chord on this platform: the same keys held, no more and no fewer,
    /// and the same key. Letters match either case. A shifted character also matches the key
    /// that types it unshifted on a US layout ('{' for "shift+["); use `matches_tx` in a key
    /// handler to follow the user's layout instead.
    pub fn matches(&self, ev: &KeyEvent) -> bool { self.matches_with(ev, None, KeyPlatform::current()) }

    /// `matches`, also comparing a character chord with `tx.key_base()`, the character the
    /// key types with no modifiers under the current layout (⌥B types "∫" on a Mac, ⇧[ types
    /// "{"), so the binding follows the layout as the menu bar's does.
    pub fn matches_tx(&self, ev: &KeyEvent, tx: &crate::Tx) -> bool {
        self.matches_with(ev, tx.key_base(), KeyPlatform::current())
    }

    /// Whether `self` and `other` are the same keys on `platform`: "mod+b" and "ctrl+b" are
    /// on Windows and Linux, not on macOS. For conflict checks in a shortcut table or editor.
    pub fn same_keys(&self, other: &Chord, platform: KeyPlatform) -> bool {
        self.key == other.key && fold(self.mods, platform) == fold(other.mods, platform)
    }

    /// The modifiers this chord needs on `platform`, in label order: ⌃⌥⇧⌘ on macOS, where
    /// `Super` folds into `Primary`; Ctrl, Alt, Shift, Super elsewhere, where `Ctrl` does.
    pub fn mods_on(&self, platform: KeyPlatform) -> Vec<ChordMod> {
        let bits = fold(self.mods, platform);
        let order = if platform.mac() {
            [ChordMod::Ctrl, ChordMod::Alt, ChordMod::Shift, ChordMod::Primary]
        } else {
            [ChordMod::Primary, ChordMod::Alt, ChordMod::Shift, ChordMod::Super]
        };
        order.into_iter().filter(|m| bits & m.bit() != 0).collect()
    }

    pub(crate) fn matches_with(&self, ev: &KeyEvent, base: Option<char>, platform: KeyPlatform) -> bool {
        fold(self.mods, platform) == held(ev.mods, platform) && self.key_matches(ev, base)
    }

    fn key_matches(&self, ev: &KeyEvent, base: Option<char>) -> bool {
        let ChordKey::Char(want) = self.key else { return named(&ev.key) == Some(self.key) };
        let typed = match &ev.key {
            Key::Command(c) => *c,
            Key::Text(t) => {
                let mut chars = t.chars();
                match (chars.next(), chars.next()) { (Some(c), None) => c, _ => return false }
            }
            _ => return false,
        };
        let typed = lower(typed);
        typed == want || base.map(lower) == Some(want) || (ev.mods.shift && us_unshifted(typed) == Some(want))
    }
}

impl FromStr for Chord {
    type Err = ChordError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let invalid = || ChordError(input.into());
        let spec = input.trim();
        // A trailing "++" is the plus key: "mod++".
        let (spec, plus) = match spec.strip_suffix("++") {
            Some(head) => (head, true),
            None if spec == "+" => ("", true),
            None => (spec, false),
        };
        let mut mods = 0;
        let mut key = plus.then_some(ChordKey::Char('+'));
        for part in spec.split('+').filter(|_| !spec.is_empty()) {
            let part = part.trim().to_lowercase();
            let bit = match part.as_str() {
                "mod" | "primary" => PRIMARY,
                "ctrl" | "control" => CTRL,
                "alt" | "option" | "opt" => ALT,
                "shift" => SHIFT,
                "super" | "cmd" | "command" | "meta" | "win" | "logo" => SUPER,
                _ => 0,
            };
            if bit != 0 {
                if key.is_some() || mods & bit != 0 { return Err(invalid()); }
                mods |= bit;
                continue;
            }
            if key.is_some() { return Err(invalid()); }
            key = Some(key_named(&part).ok_or_else(invalid)?);
        }
        Ok(Self { mods, key: key.ok_or_else(invalid)? })
    }
}

/// A command's chords: any of several alternatives ("mod+t, mod+n"), with other chords on
/// some platforms. `Keybind::new("mod+b").on(KeyPlatform::Windows, "alt+home")` is ⌘B on
/// macOS, Ctrl+B on Linux and Alt+Home on Windows. "" and "none" bind nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keybind {
    chords: Vec<Chord>,
    platforms: Vec<(KeyPlatform, Vec<Chord>)>,
}

impl Keybind {
    /// A keybind from a literal in the app's code. Panics if it doesn't parse; parse user
    /// input (a settings file) with `str::parse` instead.
    #[track_caller]
    pub fn new(spec: &str) -> Self {
        spec.parse().unwrap_or_else(|e| panic!("{e}"))
    }

    /// These chords instead on `platform`. Panics if `spec` doesn't parse, as `new` does.
    #[track_caller]
    pub fn on(mut self, platform: KeyPlatform, spec: &str) -> Self {
        let chords = parse_list(spec).unwrap_or_else(|e| panic!("{e}"));
        self.platforms.retain(|(p, _)| *p != platform);
        self.platforms.push((platform, chords));
        self
    }

    /// The chords that apply on `platform`.
    pub fn chords_on(&self, platform: KeyPlatform) -> &[Chord] {
        self.platforms.iter().find(|(p, _)| *p == platform).map_or(&self.chords, |(_, c)| c)
    }

    /// The chords that apply here.
    pub fn chords(&self) -> &[Chord] { self.chords_on(KeyPlatform::current()) }

    /// Whether `ev` is any of this platform's chords; see `Chord::matches`.
    pub fn matches(&self, ev: &KeyEvent) -> bool { self.chords().iter().any(|c| c.matches(ev)) }

    /// Whether `ev` is any of this platform's chords; see `Chord::matches_tx`.
    pub fn matches_tx(&self, ev: &KeyEvent, tx: &crate::Tx) -> bool { self.chords().iter().any(|c| c.matches_tx(ev, tx)) }

    /// The first chord's label here ("⇧⌘T", "Ctrl+Shift+T"), or "" when nothing is bound.
    pub fn label(&self) -> String { self.label_on(KeyPlatform::current()) }

    /// The first chord's label on `platform`, or "" when nothing is bound there.
    pub fn label_on(&self, platform: KeyPlatform) -> String {
        self.chords_on(platform).first().map(|c| c.label_on(platform)).unwrap_or_default()
    }
}

impl FromStr for Keybind {
    type Err = ChordError;
    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        Ok(Self { chords: parse_list(spec)?, platforms: Vec::new() })
    }
}

/// The default chords, comma-separated; per-platform ones are code, not part of the string.
impl fmt::Display for Keybind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.chords.is_empty() { return f.write_str("none"); }
        for (i, chord) in self.chords.iter().enumerate() {
            if i > 0 { f.write_str(", ")?; }
            write!(f, "{chord}")?;
        }
        Ok(())
    }
}

fn parse_list(spec: &str) -> Result<Vec<Chord>, ChordError> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("none") { return Ok(Vec::new()); }
    spec.split(',').map(str::parse).collect()
}

/// `bits` with the modifiers that are one key on `platform` merged: Ctrl into the primary
/// off macOS, the logo key into it on macOS.
fn fold(bits: u8, platform: KeyPlatform) -> u8 {
    let same = if platform.mac() { SUPER } else { CTRL };
    if bits & same != 0 { bits & !same | PRIMARY } else { bits }
}

/// The keys `m` says are held, as chord bits for `platform`. `cmd` reports the primary key
/// (⌘ on macOS, Ctrl elsewhere) alongside that key's own flag, so it isn't counted twice;
/// a `Mods` built with `cmd` alone (a script, a test) is the primary key. The web reports
/// `cmd` for either Ctrl or ⌘, so there it stands for ⌘ when that is held.
fn held(m: Mods, platform: KeyPlatform) -> u8 {
    let from_super = m.cmd && if cfg!(target_arch = "wasm32") { m.logo } else { platform.mac() };
    let from_ctrl = m.cmd && !from_super;
    let bits = [(m.cmd, PRIMARY), (m.ctrl && !from_ctrl, CTRL), (m.alt, ALT), (m.shift, SHIFT), (m.logo && !from_super, SUPER)]
        .into_iter().filter(|(on, _)| *on).fold(0, |bits, (_, bit)| bits | bit);
    fold(bits, platform)
}

fn named(key: &Key) -> Option<ChordKey> {
    Some(match key {
        Key::Enter => ChordKey::Enter,
        Key::Tab => ChordKey::Tab,
        Key::Escape => ChordKey::Escape,
        Key::Backspace => ChordKey::Backspace,
        Key::Delete => ChordKey::Delete,
        Key::Insert => ChordKey::Insert,
        Key::Home => ChordKey::Home,
        Key::End => ChordKey::End,
        Key::PageUp => ChordKey::PageUp,
        Key::PageDown => ChordKey::PageDown,
        Key::Left => ChordKey::Left,
        Key::Right => ChordKey::Right,
        Key::Up => ChordKey::Up,
        Key::Down => ChordKey::Down,
        Key::Function(n) => ChordKey::Function(*n),
        Key::ContextMenu => ChordKey::ContextMenu,
        _ => return None,
    })
}

fn key_named(name: &str) -> Option<ChordKey> {
    Some(match name {
        "enter" | "return" => ChordKey::Enter,
        "tab" => ChordKey::Tab,
        "escape" | "esc" => ChordKey::Escape,
        "backspace" => ChordKey::Backspace,
        "delete" | "del" => ChordKey::Delete,
        "insert" | "ins" => ChordKey::Insert,
        "home" => ChordKey::Home,
        "end" => ChordKey::End,
        "pageup" | "pgup" => ChordKey::PageUp,
        "pagedown" | "pgdn" => ChordKey::PageDown,
        "left" | "arrowleft" => ChordKey::Left,
        "right" | "arrowright" => ChordKey::Right,
        "up" | "arrowup" => ChordKey::Up,
        "down" | "arrowdown" => ChordKey::Down,
        "contextmenu" | "menu" => ChordKey::ContextMenu,
        "space" => ChordKey::Char(' '),
        "plus" => ChordKey::Char('+'),
        "comma" => ChordKey::Char(','),
        "minus" => ChordKey::Char('-'),
        "equal" => ChordKey::Char('='),
        "period" => ChordKey::Char('.'),
        "slash" => ChordKey::Char('/'),
        "backslash" => ChordKey::Char('\\'),
        "backquote" => ChordKey::Char('`'),
        "semicolon" => ChordKey::Char(';'),
        "quote" => ChordKey::Char('\''),
        "bracketleft" => ChordKey::Char('['),
        "bracketright" => ChordKey::Char(']'),
        f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) && !f[1..].starts_with('0') => {
            ChordKey::Function(f[1..].parse().ok()?)
        }
        c => {
            let mut chars = c.chars();
            match (chars.next(), chars.next()) { (Some(c), None) => ChordKey::Char(lower(c)), _ => return None }
        }
    })
}

/// `c` lowercased when that is one character.
fn lower(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) { (Some(l), None) => l, _ => c }
}

/// The key that types `c` with Shift on a US layout.
fn us_unshifted(c: char) -> Option<char> {
    let (shifted, plain) = ("~!@#$%^&*()_+{}|:\"<>?", "`1234567890-=[]\\;',./");
    shifted.chars().position(|s| s == c).and_then(|i| plain.chars().nth(i))
}
