//! Input method events: what the platform's input method (dead keys, CJK, the emoji picker,
//! dictation) composes, for the focused `text_input` or an app's own editor (`App::on_ime`);
//! and a field's [`Composition`], stepped by those events.
use zeroize::Zeroize;

use crate::machine::Effects;

/// One input method event. Plain typing arrives as keys (`Key::Text`); these carry what an
/// input method composed.
#[derive(Clone, Debug, PartialEq)]
pub enum Ime {
    /// The composition so far (marked text), drawn at the caret and not yet part of the text,
    /// with the byte range of its cursor or selection in it. Empty ends the composition.
    /// A range that isn't empty is the clause being converted (Windows and Wayland report the
    /// target clause there): it draws with a thick underline and the candidate window opens
    /// under it.
    Preedit(String, Option<(usize, usize)>),
    /// Text to insert at the caret, replacing the preedit.
    Commit(String),
    /// The window's input method turned on for the caret that takes text (a focused
    /// `text_input` or an `El::ime_cursor` surface), or off again. Informational: Rune turns it
    /// on and off.
    Enabled,
    Disabled,
}

impl Drop for Ime {
    fn drop(&mut self) {
        match self {
            Self::Preedit(text, _) | Self::Commit(text) => text.zeroize(),
            Self::Enabled | Self::Disabled => {}
        }
    }
}

/// Whether a field's input method is composing: the marked text it draws at the caret, not
/// yet part of the field's text, and the byte range of its cursor or selection in it.
#[derive(Clone, Default, PartialEq, Hash)]
pub(super) enum Composition {
    #[default]
    Idle,
    Composing { text: String, cursor: Option<(usize, usize)> },
}

/// What steps a [`Composition`]: the input method, or the field ending it on its own.
pub(super) enum Event<'a> {
    Ime(&'a Ime),
    /// The field let go of the composition while it showed (it blurred, the window went
    /// inactive, a click moved the caret): what it shows becomes text, as a browser's field and
    /// an `NSTextView` keep it, and the platform's input method is told to drop its copy.
    Finish,
    /// An edit or undo replaced the text under the composition, or the field turned secure:
    /// the composition goes, unwritten.
    Cancel,
}

/// What a composition step asks of its field.
#[derive(Debug, PartialEq)]
pub(super) enum Effect {
    /// Insert the committed text at the caret.
    Commit(String),
}

impl Composition {
    pub(super) fn preedit(&self) -> Option<(&str, Option<(usize, usize)>)> {
        match self {
            Composition::Idle => None,
            Composition::Composing { text, cursor } => Some((text.as_str(), *cursor)),
        }
    }

    /// The clause being converted, a byte range of the preedit, when the input method marks
    /// one (a cursor range that isn't empty).
    pub(super) fn target(&self) -> Option<(usize, usize)> {
        match self {
            Composition::Composing { text, cursor: Some((a, b)) } if a < b && *b <= text.len() => Some((*a, *b)),
            _ => None,
        }
    }
}

/// Steps `composition`.
pub(super) fn step(composition: &mut Composition, event: Event<'_>) -> Effects<Effect> {
    let previous = std::mem::take(composition);
    match (previous, event) {
        (mut old, Event::Ime(Ime::Preedit(text, cursor))) => {
            wipe(&mut old);
            if !text.is_empty() {
                *composition = Composition::Composing { text: text.clone(), cursor: *cursor };
            }
            Effects::none()
        }
        (mut old, Event::Ime(Ime::Commit(text))) => {
            wipe(&mut old);
            Effects::one(Effect::Commit(text.clone()))
        }
        (old, Event::Ime(Ime::Enabled | Ime::Disabled)) => {
            *composition = old;
            Effects::none()
        }
        (Composition::Composing { text, .. }, Event::Finish) => Effects::one(Effect::Commit(text)),
        (Composition::Idle, Event::Finish) => Effects::none(),
        (mut old, Event::Cancel) => {
            wipe(&mut old);
            Effects::none()
        }
    }
}

fn wipe(composition: &mut Composition) {
    if let Composition::Composing { text, .. } = composition { text.zeroize(); }
}
