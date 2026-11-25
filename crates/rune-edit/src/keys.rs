//! Key events as the shell hands them to handlers and the focused field.
use zeroize::Zeroize;

#[doc(hidden)]
pub const PRIMARY: u8 = 1;
#[doc(hidden)]
pub const CTRL: u8 = 2;
#[doc(hidden)]
pub const ALT: u8 = 4;
#[doc(hidden)]
pub const SHIFT: u8 = 8;
#[doc(hidden)]
pub const SUPER: u8 = 16;

/// The platform whose keyboard conventions a chord resolves and draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyPlatform {
    /// ⌘ is the primary modifier; Control is a separate key. Glyph labels: "⇧⌘T".
    Mac,
    /// Ctrl is the primary modifier. Word labels: "Ctrl+Shift+T", the logo key is "Win".
    Windows,
    /// Ctrl is the primary modifier. Word labels, the logo key is "Super". The web uses this
    /// in a browser that isn't on an Apple device.
    Linux,
}

impl KeyPlatform {
    /// The platform this build runs on.
    #[cfg(not(target_arch = "wasm32"))]
    pub const fn current() -> Self {
        if cfg!(any(target_os = "macos", target_os = "ios")) { return Self::Mac; }
        if cfg!(target_os = "windows") { return Self::Windows; }
        Self::Linux
    }

    /// The platform the browser runs on: `Mac` when `navigator.platform` names an Apple
    /// device (as the browser's own shortcuts do), else `Linux`.
    #[cfg(target_arch = "wasm32")]
    pub fn current() -> Self {
        static PLATFORM: std::sync::OnceLock<KeyPlatform> = std::sync::OnceLock::new();
        *PLATFORM.get_or_init(|| {
            let platform = js_sys::Reflect::get(&js_sys::global(), &"navigator".into())
                .and_then(|navigator| js_sys::Reflect::get(&navigator, &"platform".into()))
                .ok().and_then(|p| p.as_string()).unwrap_or_default();
            if ["Mac", "iPod", "iPhone", "iPad"].iter().any(|apple| platform.contains(apple)) { Self::Mac } else { Self::Linux }
        })
    }

    fn mac(self) -> bool { matches!(self, Self::Mac) }
}

/// The modifiers held with a key. `shift`, `alt`, `ctrl` and `logo` are keys; `cmd` is the
/// platform's primary shortcut modifier, reported alongside its key: ⌘ on macOS (with `logo`)
/// and Ctrl elsewhere (with `ctrl`), so Ctrl+W on Windows is `{ cmd, ctrl }`. A test of
/// `cmd && !ctrl` therefore never passes off macOS. Match shortcuts with a
/// `Chord` ("mod+w", "ctrl+tab") rather than by comparing flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    /// The primary shortcut modifier: ⌘ on macOS, Ctrl elsewhere (the web reports either).
    pub cmd: bool,
    pub alt: bool,
    /// The Control key, on every platform; off macOS `cmd` is set with it.
    pub ctrl: bool,
    /// The Super/Windows key (Command on macOS, where `cmd` also reports it).
    pub logo: bool,
}

impl Mods {
    /// Whether the platform's primary shortcut modifier is held: `cmd`.
    pub fn primary(self) -> bool { self.cmd }
}

/// `bits` with the modifiers that are one key on `platform` merged: Ctrl into the primary
/// off macOS, the logo key into it on macOS.
#[doc(hidden)]
pub fn fold(bits: u8, platform: KeyPlatform) -> u8 {
    let same = if platform.mac() { SUPER } else { CTRL };
    if bits & same != 0 { bits & !same | PRIMARY } else { bits }
}

/// The keys `m` says are held, as chord bits for `platform`. `cmd` repeats the key this build
/// reports as primary: ⌘ (`logo`) on macOS, Ctrl elsewhere (iOS too), Ctrl or ⌘ on the web. A `Mods`
/// with `cmd` and neither key (a script, a test) holds `platform`'s primary.
#[doc(hidden)]
pub fn held(m: Mods, platform: KeyPlatform) -> u8 {
    let bare = m.cmd && !m.ctrl && !m.logo;
    let (ctrl, logo) = if bare || cfg!(target_arch = "wasm32") {
        (m.ctrl, m.logo)
    } else if cfg!(target_os = "macos") {
        (m.ctrl, m.logo || m.cmd)
    } else {
        (m.ctrl || m.cmd, m.logo)
    };
    let (ctrl_bit, logo_bit) = if platform.mac() { (CTRL, PRIMARY) } else { (PRIMARY, SUPER) };
    [(bare, PRIMARY), (ctrl, ctrl_bit), (logo, logo_bit), (m.alt, ALT), (m.shift, SHIFT)]
        .into_iter().filter(|(on, _)| *on).fold(0, |bits, (_, bit)| bits | bit)
}

/// Whether `m` holds `platform`'s primary modifier and no other but Shift, or none at all (a
/// script's `Key::Command`): ⌘ alone on macOS, Ctrl alone elsewhere. For the edit commands a
/// text field takes (copy, paste, undo), which another modifier turns into an app's chord.
pub(crate) fn primary_alone_on(m: Mods, platform: KeyPlatform) -> bool {
    matches!(held(m, platform) & !SHIFT, 0 | PRIMARY)
}

/// [`primary_alone_on`] for [`KeyPlatform::current`].
#[cfg(test)]
pub(crate) fn primary_alone(m: Mods) -> bool {
    primary_alone_on(m, KeyPlatform::current())
}

/// A logical key. More keys may come, so a `match` needs a `_` arm.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Text(String),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    /// Page Up. Unclaimed, it scrolls the nearest scroll view up by most of a viewport.
    /// Text fields leave it alone, as AppKit's do: the view scrolls and the caret stays.
    PageUp,
    /// Page Down, as `PageUp` the other way.
    PageDown,
    /// Insert (Help on older Mac keyboards). Off macOS, `Shift+Insert` pastes into a focused text
    /// field or root `on_paste` handler; bare `Insert` and other chords are left for apps to bind.
    Insert,
    /// Print Screen (SysRq). Text fields ignore it; apps can bind it. Windows sends it only
    /// on release, so its press and release arrive together.
    PrintScreen,
    /// Scroll Lock. Text fields ignore it; apps can bind it.
    ScrollLock,
    /// Pause (Break). Text fields ignore it; apps can bind it.
    Pause,
    Escape,
    Tab,
    /// Any character typed with Cmd (Ctrl off macOS), lowercased: a letter ("a", "c", "v"), a
    /// digit (⌘1) or punctuation (⌘,). Off macOS, a key that types a non-ASCII character
    /// arrives as the US key in its place, so Ctrl+V on a Russian layout is `Command('v')`
    /// (`Tx::key_base` still says "м"). `mods` still says whether Shift, Option or Ctrl were
    /// held too. Space is the exception: Cmd-Space arrives natively as `Text(" ")` with
    /// `mods.cmd` (scripts send `Command(' ')`). The other native `Text` with `mods.cmd` is a
    /// character typed with Left Ctrl + Left Alt on Windows, which is AltGr there ("@" on a
    /// German layout's Q): it inserts, and `Chord::matches_tx` still finds "ctrl+alt+q".
    /// [`KeyEvent::is_command`] matches either form.
    Command(char),
    /// A function key, F1 through F24. Text fields ignore it; apps can bind it.
    Function(u8),
    /// The Menu (Application) key. Unclaimed, it opens the focused element's context menu, as
    /// Shift+F10 does: see `El::on_secondary_press`.
    ContextMenu,
    /// A modifier key going down (`pressed`) or up on its own, sent only where the app opts in
    /// with `App::modifier_keys` (or `Runtime::set_modifier_keys`). `mods` holds the modifiers
    /// after the change, as AppKit's flagsChanged does: Shift's press has `shift` set and its
    /// release doesn't, unless the other Shift is still down. A held modifier never repeats.
    /// Text fields, selection, focus traversal and shortcuts ignore it.
    Modifier { modifier: Modifier, pressed: bool },
    /// A hardware media transport key (`PlayPause`, `Next`, `Previous`, ...), sent only where
    /// the app opts in with `App::media_keys` (or `WindowSpec::media_keys`,
    /// `ConfiguredOverlay::media_keys`, `Runtime::set_media_keys`). Off by default so apps
    /// written without it receive no new keys and never intercept system media keys on macOS.
    /// Text fields ignore it; apps can bind it in `on_key`, `El::on_key`, `El::on_key_preview`,
    /// `El::on_key_up`, and `Chord`.
    Media(MediaKey),
}

/// A hardware media transport key carried by [`Key::Media`].
///
/// Mirrors the W3C UI Events `KeyboardEvent.key` media values (`MediaPlayPause`, `MediaPlay`,
/// `MediaPause`, `MediaTrackNext`, `MediaTrackPrevious`, `MediaStop`, `MediaFastForward`,
/// `MediaRewind`), winit's `NamedKey::Media*` variants, Linux XKB `XF86Audio*` keysyms, and
/// macOS `NSEventTypeSystemDefined` `NX_KEYTYPE_*` events.
///
/// ```
/// use rune_edit::{Key, KeyEvent, MediaKey};
///
/// let ev = KeyEvent::from(Key::Media(MediaKey::PlayPause));
/// assert_eq!(ev.key, Key::Media(MediaKey::PlayPause));
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MediaKey {
    /// Combined Play/Pause toggle (the ⏯ key on Mac, Windows, and web keyboards).
    PlayPause,
    /// Dedicated Play key (for example Linux XKB `XF86AudioPlay` or media remotes).
    Play,
    /// Dedicated Pause key (for example Linux XKB `XF86AudioPause` or media remotes).
    Pause,
    /// Next track (⏭).
    Next,
    /// Previous track (⏮).
    Previous,
    /// Stop playback (⏹).
    Stop,
    /// Fast forward (⏩).
    FastForward,
    /// Rewind (⏪).
    Rewind,
}

/// A modifier key, by side. Alt is Option on macOS; Super is Command on macOS and the Windows
/// key elsewhere.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Modifier {
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    SuperLeft,
    SuperRight,
    /// Fn, or Globe on recent Mac keyboards. Other platforms rarely report it: their keyboards
    /// handle Fn in firmware.
    Fn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeySide {
    Left,
    Right,
}

impl Modifier {
    /// Which side of the keyboard the key is on; None for Fn.
    pub fn side(self) -> Option<KeySide> {
        use Modifier::*;
        match self {
            ShiftLeft | ControlLeft | AltLeft | SuperLeft => Some(KeySide::Left),
            ShiftRight | ControlRight | AltRight | SuperRight => Some(KeySide::Right),
            Fn => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Mods,
}

impl KeyEvent {
    /// Whether this is Cmd (Ctrl off macOS) plus `c`, whatever else is held: `Key::Command(c)`,
    /// or for `' '` the `Key::Text(" ")` a native Cmd-Space sends. Letters match either case;
    /// check `mods.shift` for ⇧⌘ chords.
    pub fn is_command(&self, c: char) -> bool {
        let c = c.to_ascii_lowercase();
        match &self.key {
            Key::Command(k) => *k == c,
            Key::Text(t) => c == ' ' && t == " " && self.mods.cmd,
            _ => false,
        }
    }

    /// Text typed with Cmd (macOS), Ctrl, or (off macOS) Alt alone held, such as an unclaimed
    /// Shift-Cmd-Space or Alt+D. It never inserts, matching browsers, AppKit and GTK, but still
    /// reaches key handlers. Ctrl+Alt is AltGr on Windows and Linux, so text typed with both
    /// still inserts there; Option alone types on macOS.
    #[doc(hidden)]
    pub fn is_command_chord(&self) -> bool {
        self.is_command_chord_on(KeyPlatform::current())
    }

    pub(crate) fn is_command_chord_on(&self, platform: KeyPlatform) -> bool {
        let m = self.mods;
        matches!(self.key, Key::Text(_))
            && if platform.mac() { m.cmd || m.ctrl } else { (m.cmd || m.ctrl) ^ m.alt }
    }

    /// The standard edit command this is (paste, copy, cut, select all, undo, redo, format):
    /// `Key::Command(c)` with the primary modifier alone, ⌘ on macOS and Ctrl elsewhere, or
    /// Shift too for ⇧⌘Z, plus `Shift+Insert` for paste off macOS. Another modifier
    /// (Ctrl+Alt+V, ⌃⌘C, Ctrl+Shift+V, Ctrl+Shift+Insert) makes it an app's chord, which
    /// reaches `on_key` instead.
    #[doc(hidden)]
    pub fn edit_command(&self) -> Option<char> {
        self.edit_command_on(KeyPlatform::current())
    }

    pub(crate) fn edit_command_on(&self, platform: KeyPlatform) -> Option<char> {
        match self.key {
            Key::Command(c) => (primary_alone_on(self.mods, platform) && (c == 'z' || !self.mods.shift)).then_some(c),
            Key::Insert => (!platform.mac() && held(self.mods, platform) == SHIFT).then_some('v'),
            _ => None,
        }
    }

    /// Alt+Space off macOS: the window menu's key. It never types a space, and on Windows opens
    /// the window menu when no handler takes it. (Option+Space types on macOS.)
    #[doc(hidden)]
    pub fn is_window_menu_chord(&self) -> bool {
        let m = self.mods;
        !cfg!(target_os = "macos") && matches!(&self.key, Key::Text(t) if t == " ") && m.alt && !m.ctrl && !m.cmd && !m.logo
    }

    /// A macOS Control-letter text binding (Ctrl-A, Ctrl-E, Ctrl-K, …), as in every AppKit field.
    #[doc(hidden)]
    pub fn control_binding(&self) -> Option<char> {
        let m = self.mods;
        let Key::Text(s) = &self.key else { return None };
        let c = s.chars().next().filter(|_| s.chars().count() == 1)?.to_ascii_lowercase();
        (cfg!(target_os = "macos") && m.ctrl && !m.cmd && !m.alt && "abefpndhk".contains(c)).then_some(c)
    }
}

/// The key pressed with no modifiers.
impl From<Key> for KeyEvent {
    fn from(key: Key) -> Self {
        KeyEvent { key, mods: Mods::default() }
    }
}

impl Drop for KeyEvent {
    fn drop(&mut self) {
        if let Key::Text(text) = &mut self.key { text.zeroize(); }
    }
}

#[cfg(test)]
#[path = "keys_tests.rs"]
mod tests;
