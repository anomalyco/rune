//! Input method events: what the platform's input method (dead keys, CJK, the emoji picker,
//! dictation) composes, for the focused `text_input` or an app's own editor (`App::on_ime`);
//! and a field's [`Composition`], stepped by those events.
use std::ops::Range;

use zeroize::Zeroize;

use crate::machine::Effects;

/// One input method event. Plain typing arrives as keys (`Key::Text`); these carry what an
/// input method composed.
#[derive(Clone, Debug, PartialEq)]
pub enum Ime {
    /// The composition so far (marked text), drawn at the caret and not yet part of the text,
    /// with the byte range of its cursor or selection in it, and its clauses as the input
    /// method splits it (a Japanese sentence's phrases, each converted on its own). Empty text
    /// ends the composition. With no clauses (an input method or platform that doesn't say), a
    /// cursor range that isn't empty is taken as the clause being converted.
    Preedit(String, Option<(usize, usize)>, Vec<Clause>),
    /// Text to insert at the caret, replacing the preedit.
    Commit(String),
    /// The input method takes this byte range of the text back to convert it again
    /// (reconversion: Japanese 変換 on committed text, macOS ⌃⇧R): the field selects it, and
    /// the composition that follows replaces it.
    Select(Range<usize>),
    /// The window's input method turned on for the caret that takes text (a focused
    /// `text_input` or an `El::ime_cursor` surface), or off again. Informational: Rune turns it
    /// on and off.
    Enabled,
    Disabled,
}

impl Drop for Ime {
    fn drop(&mut self) {
        match self {
            Self::Preedit(text, ..) | Self::Commit(text) => text.zeroize(),
            Self::Select(_) | Self::Enabled | Self::Disabled => {}
        }
    }
}

/// One clause of a composition: a byte range of the preedit, and whether it's the one being
/// converted (its candidates show; it draws with a thick underline).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Clause {
    range: Range<usize>,
    target: bool,
}

impl Clause {
    /// A clause over byte range `range` of the preedit, not the one being converted.
    pub fn new(range: Range<usize>) -> Clause {
        Clause { range, target: false }
    }

    /// The clause being converted.
    pub fn target(mut self) -> Clause {
        self.target = true;
        self
    }

    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn is_target(&self) -> bool {
        self.target
    }
}

/// A field's text around its selection, which the platform's input method reads to convert
/// committed text again (reconversion) and to convert with the context around the caret
/// (Windows' document feed). One paragraph, at most a KiB either side of the selection.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ImeText {
    pub(crate) text: String,
    /// Where `text` starts in the field's text, in bytes.
    pub(crate) start: usize,
    /// The selection, a byte range of `text`.
    pub(crate) selection: Range<usize>,
}

impl ImeText {
    /// The word (UAX #29) around byte `at` of `text`, for reconversion with no selection.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn word_at(&self, at: usize) -> Range<usize> {
        use unicode_segmentation::UnicodeSegmentation;
        self.text.split_word_bound_indices().map(|(i, w)| i..i + w.len()).find(|r| r.start <= at && at < r.end && !self.text[r.clone()].trim().is_empty())
            .unwrap_or(at..at)
    }
}

/// Whether a field's input method is composing: the marked text it draws at the caret, not
/// yet part of the field's text, the byte range of its cursor or selection in it, and its
/// clauses.
#[derive(Clone, Default, PartialEq, Hash)]
pub(super) enum Composition {
    #[default]
    Idle,
    Composing { text: String, cursor: Option<(usize, usize)>, clauses: Vec<Clause> },
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
    /// Select this byte range of the text, for the input method to convert again.
    Select(Range<usize>),
}

impl Composition {
    pub(super) fn preedit(&self) -> Option<(&str, Option<(usize, usize)>)> {
        match self {
            Composition::Idle => None,
            Composition::Composing { text, cursor, .. } => Some((text.as_str(), *cursor)),
        }
    }

    /// The composition's clauses, each a byte range of the preedit, the one being converted
    /// marked: the input method's, else one clause over the whole preedit with a cursor range
    /// that isn't empty as the target inside it. Empty when idle.
    pub(super) fn clauses(&self) -> Vec<Clause> {
        match self {
            Composition::Idle => Vec::new(),
            Composition::Composing { clauses, .. } if !clauses.is_empty() => clauses.clone(),
            Composition::Composing { text, .. } => match self.target() {
                Some((a, b)) => [Clause::new(0..a), Clause::new(a..b).target(), Clause::new(b..text.len())].into_iter().filter(|c| !c.range.is_empty()).collect(),
                None => vec![Clause::new(0..text.len())],
            },
        }
    }

    /// The clause being converted, a byte range of the preedit: the input method's target
    /// clause, else a cursor range that isn't empty.
    pub(super) fn target(&self) -> Option<(usize, usize)> {
        let Composition::Composing { text, cursor, clauses } = self else { return None };
        if let Some(c) = clauses.iter().find(|c| c.target && c.range.end <= text.len()) {
            return Some((c.range.start, c.range.end));
        }
        match cursor {
            Some((a, b)) if a < b && *b <= text.len() => Some((*a, *b)),
            _ => None,
        }
    }
}

/// Steps `composition`.
pub(super) fn step(composition: &mut Composition, event: Event<'_>) -> Effects<Effect> {
    let previous = std::mem::take(composition);
    match (previous, event) {
        (mut old, Event::Ime(Ime::Preedit(text, cursor, clauses))) => {
            wipe(&mut old);
            if !text.is_empty() {
                // Clauses that don't fit the text or a char boundary are the platform's mistake: dropped.
                let fits = |c: &Clause| c.range.start < c.range.end && c.range.end <= text.len() && text.is_char_boundary(c.range.start) && text.is_char_boundary(c.range.end);
                let clauses = if clauses.iter().all(fits) { clauses.clone() } else { Vec::new() };
                *composition = Composition::Composing { text: text.clone(), cursor: *cursor, clauses };
            }
            Effects::none()
        }
        (mut old, Event::Ime(Ime::Commit(text))) => {
            wipe(&mut old);
            Effects::one(Effect::Commit(text.clone()))
        }
        (Composition::Idle, Event::Ime(Ime::Select(range))) => Effects::one(Effect::Select(range.clone())),
        // Mid-composition the input method converts what it already has.
        (old @ Composition::Composing { .. }, Event::Ime(Ime::Select(_))) => {
            *composition = old;
            Effects::none()
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
