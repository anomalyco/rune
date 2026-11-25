//! The words Rune itself puts on screen: a text field's menu (Cut, Copy, Paste, Select All and
//! the spelling items), selectable text's Copy, the Windows custom-titlebar caption buttons
//! (Minimize, Maximize, Restore, Close), and on macOS the menu bar's standard items and menus.
//! They are English until the app names them in its own language with [`set_labels`], and
//! [`set_locale`] says which language that is.
//!
//! Rune asks each time it shows one, so after the app's language changes the next menu opened
//! speaks it and the Windows caption buttons take the new names on the next view commit. The
//! macOS menu bar takes the new names at the next [`AppHandle::set_menu`].
//!
//! [`AppHandle::set_menu`]: crate::AppHandle::set_menu
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// Something Rune names on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Label<'a> {
    Cut,
    Copy,
    Paste,
    SelectAll,
    Undo,
    Redo,
    /// A misspelled word's item that adds the word to the user's dictionary.
    LearnSpelling,
    /// Shown, disabled, in place of the suggestions when the checker has none.
    NoGuesses,
    /// The field menu's checkbox for the app's spellcheck setting (`El::on_spellcheck`).
    Spellcheck,
    /// The field menu's submenu of dictionaries (`El::on_spellcheck_languages`).
    SpellcheckLanguages,
    /// A dictionary in that submenu, by its language tag (`en-US`). Without a name from the app
    /// it's named in the app's language ([`set_locale`]) where the platform can, else in
    /// English. The submenu lists them in the order of their names.
    Language(&'a str),
    /// A top-level menu of the macOS menu bar, by the name its items give (`MenuItem::menu`):
    /// Rune's own "Edit" and "Window", or one of the app's.
    Menu(&'a str),
    /// The Window menu's Minimize item (macOS) and the Windows custom-titlebar minimize button.
    Minimize,
    /// The Windows custom-titlebar caption button that maximizes a restored window
    /// (English: `"Maximize"`).
    ///
    /// ```
    /// rune::set_labels(|label| match label {
    ///     rune::Label::Maximize => Some("Maximieren".into()),
    ///     _ => None,
    /// });
    /// assert_eq!(rune::labels::label(rune::Label::Maximize), "Maximieren");
    /// ```
    Maximize,
    /// The Windows custom-titlebar caption button that restores a maximized window
    /// (English: `"Restore"`).
    ///
    /// ```
    /// rune::set_labels(|label| match label {
    ///     rune::Label::Restore => Some("Wiederherstellen".into()),
    ///     _ => None,
    /// });
    /// assert_eq!(rune::labels::label(rune::Label::Restore), "Wiederherstellen");
    /// ```
    Restore,
    /// The Window menu's Zoom item (macOS).
    Zoom,
    /// The Window menu's Close item (macOS) and the Windows custom-titlebar close button.
    Close,
    /// The Window menu's Bring All to Front item (macOS).
    BringAllToFront,
    /// The Edit menu's system items (macOS).
    StartDictation,
    EmojiAndSymbols,
    /// The app menu's items; some carry the app's name.
    About(&'a str),
    Services,
    Hide(&'a str),
    HideOthers,
    ShowAll,
    Quit(&'a str),
    /// A Markdown block's marker before `n` lines nested past the container bound.
    MoreNestedLines(usize),
    /// A link's default context-menu item that activates the link (English: `"Open Link"`).
    ///
    /// ```
    /// rune::set_labels(|label| match label {
    ///     rune::Label::OpenLink => Some("Link öffnen".into()),
    ///     _ => None,
    /// });
    /// assert_eq!(rune::labels::label(rune::Label::OpenLink), "Link öffnen");
    /// ```
    OpenLink,
    /// A link's default context-menu item that copies its address (English: `"Copy Link"`).
    ///
    /// ```
    /// rune::set_labels(|label| match label {
    ///     rune::Label::CopyLink => Some("Link kopieren".into()),
    ///     _ => None,
    /// });
    /// assert_eq!(rune::labels::label(rune::Label::CopyLink), "Link kopieren");
    /// ```
    CopyLink,
    /// An opted-in image's context-menu item that saves its original source bytes or file
    /// (English: `"Save Image As…"`).
    ///
    /// ```
    /// rune::set_labels(|label| match label {
    ///     rune::Label::SaveImageAs => Some("Bild speichern unter…".into()),
    ///     _ => None,
    /// });
    /// assert_eq!(rune::labels::label(rune::Label::SaveImageAs), "Bild speichern unter…");
    /// ```
    SaveImageAs,
    /// An opted-in image's context-menu item that copies the image to the clipboard
    /// (English: `"Copy Image"`).
    ///
    /// ```
    /// rune::set_labels(|label| match label {
    ///     rune::Label::CopyImage => Some("Bild kopieren".into()),
    ///     _ => None,
    /// });
    /// assert_eq!(rune::labels::label(rune::Label::CopyImage), "Bild kopieren");
    /// ```
    CopyImage,
    /// An opted-in image's context-menu item that copies its URL (English: `"Copy Image Address"`).
    ///
    /// ```
    /// rune::set_labels(|label| match label {
    ///     rune::Label::CopyImageAddress => Some("Bildadresse kopieren".into()),
    ///     _ => None,
    /// });
    /// assert_eq!(rune::labels::label(rune::Label::CopyImageAddress), "Bildadresse kopieren");
    /// ```
    CopyImageAddress,
}

type Namer = Arc<dyn Fn(Label<'_>) -> Option<String> + Send + Sync>;

static NAMER: RwLock<Option<Namer>> = RwLock::new(None);
static LOCALE: RwLock<Option<String>> = RwLock::new(None);
static RTL: AtomicBool = AtomicBool::new(false);

/// Names Rune's own labels with `names`, for every window. `None` keeps a label's English.
pub fn set_labels(names: impl Fn(Label<'_>) -> Option<String> + Send + Sync + 'static) {
    if let Ok(mut namer) = NAMER.write() {
        *namer = Some(Arc::new(names));
    }
}

/// The language the app's labels are in, as a BCP 47 tag (`de`, `zh-Hant`). Rune names the
/// spellcheck dictionaries in it when the app doesn't (`Label::Language`), as a browser's
/// `Intl.DisplayNames(locale)` would, and sorts them as it sorts text (`localeCompare`).
pub fn set_locale(tag: &str) {
    if let Ok(mut locale) = LOCALE.write() {
        *locale = Some(tag.to_owned());
    }
}

/// The way the app's language reads, which Rune's native menus take, as a browser's menus take
/// its interface language's: right to left, a Windows pop-up, window or tray menu lays out right
/// to left (its checkmarks and submenu arrows on the left, shortcuts at the left edge) and opens
/// leftward from its point or under its anchor's right edge, and a macOS pop-up menu's items
/// align right. Left to right (`Ltr` or `Auto`) until the app says.
pub fn set_direction(direction: crate::Direction) {
    RTL.store(direction == crate::Direction::Rtl, Ordering::Relaxed);
}

/// Whether the app's language reads right to left ([`set_direction`]).
pub(crate) fn rtl() -> bool {
    RTL.load(Ordering::Relaxed)
}

/// What `label` reads now: the app's name for it, else Rune's English.
pub fn label(label: Label<'_>) -> String {
    named(label).unwrap_or_else(|| english(label))
}

/// The app's name for `label`, if it gave one.
pub(crate) fn named(label: Label<'_>) -> Option<String> {
    let namer = NAMER.read().ok().and_then(|namer| namer.clone());
    namer.and_then(|names| names(label))
}

/// The app's language ([`set_locale`]), if it said.
pub(crate) fn locale() -> Option<String> {
    LOCALE.read().ok().and_then(|locale| locale.clone())
}

pub(crate) fn english(label: Label<'_>) -> String {
    match label {
        Label::Cut => "Cut".into(),
        Label::Copy => "Copy".into(),
        Label::Paste => "Paste".into(),
        Label::SelectAll => "Select All".into(),
        Label::Undo => "Undo".into(),
        Label::Redo => "Redo".into(),
        Label::LearnSpelling => "Learn Spelling".into(),
        Label::NoGuesses => "No Guesses Found".into(),
        Label::Spellcheck => "Spellcheck".into(),
        Label::SpellcheckLanguages => "Spellcheck Languages".into(),
        // The field menu names a dictionary itself when the app doesn't (`spell::language_name`).
        Label::Language(tag) => tag.into(),
        Label::Menu(name) => name.into(),
        Label::Minimize => "Minimize".into(),
        Label::Maximize => "Maximize".into(),
        Label::Restore => "Restore".into(),
        Label::Zoom => "Zoom".into(),
        Label::Close => "Close".into(),
        Label::BringAllToFront => "Bring All to Front".into(),
        Label::StartDictation => "Start Dictation…".into(),
        Label::EmojiAndSymbols => "Emoji & Symbols".into(),
        Label::About(app) => format!("About {app}"),
        Label::Services => "Services".into(),
        Label::Hide(app) => format!("Hide {app}"),
        Label::HideOthers => "Hide Others".into(),
        Label::ShowAll => "Show All".into(),
        Label::Quit(app) => format!("Quit {app}"),
        Label::MoreNestedLines(1) => "1 more nested line".into(),
        Label::MoreNestedLines(n) => format!("{n} more nested lines"),
        Label::OpenLink => "Open Link".into(),
        Label::CopyLink => "Copy Link".into(),
        Label::SaveImageAs => "Save Image As…".into(),
        Label::CopyImage => "Copy Image".into(),
        Label::CopyImageAddress => "Copy Image Address".into(),
    }
}

#[cfg(test)]
pub(crate) struct LabelsGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prev: Option<Namer>,
}

#[cfg(test)]
impl Drop for LabelsGuard {
    fn drop(&mut self) {
        if let Ok(mut namer) = NAMER.write() {
            *namer = self.prev.take();
        }
    }
}

/// Serializes tests that call [`set_labels`] and restores the previous namer on drop.
#[cfg(test)]
pub(crate) fn test_lock() -> LabelsGuard {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev = NAMER.read().ok().and_then(|n| n.clone());
    LabelsGuard { _lock: lock, prev }
}
