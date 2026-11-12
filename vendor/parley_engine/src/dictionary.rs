// Copyright 2026 the Parley Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Word boundaries for the scripts whose words need a dictionary (Thai, Lao, Khmer, Burmese,
//! Chinese and Japanese), from outside Parley (Rune: vendor/parley_engine.patch). A build without
//! `complex-scripts` links none of ICU's dictionaries; a host that has its own (a browser's
//! `Intl.Segmenter`) gives them here. Its boundaries are the words inside each such run and, in
//! Thai, Lao, Khmer and Burmese, the run's line break opportunities too, as ICU's dictionary
//! line segmenter makes them (UAX #14 breaks class SA at word boundaries).

use alloc::vec::Vec;

use icu_properties::props::{GeneralCategory, Script};
use parley_data::Properties;

/// Appends to `out` the word boundaries strictly inside `text`, as ascending byte offsets, for a
/// `text` all in one dictionary language: `language` is `th`, `lo`, `km`, `my` or `ja` (Han,
/// hiragana and katakana, whose dictionary Chinese and Japanese share).
pub type DictionarySegmenter = fn(text: &str, language: &str, out: &mut Vec<usize>);

#[cfg(feature = "std")]
static SEGMENTER: std::sync::OnceLock<DictionarySegmenter> = std::sync::OnceLock::new();

/// Gives Parley the host's dictionary word boundaries, once (a second call changes nothing).
/// Without `complex-scripts` and without this, a Thai phrase has no break opportunity and a
/// Chinese sentence's words are its characters. Needs `std`.
pub fn set_dictionary_segmenter(segmenter: DictionarySegmenter) {
    #[cfg(feature = "std")]
    let _ = SEGMENTER.set(segmenter);
    #[cfg(not(feature = "std"))]
    let _ = segmenter;
}

fn segmenter() -> Option<DictionarySegmenter> {
    #[cfg(all(feature = "std", not(feature = "complex-scripts")))]
    return SEGMENTER.get().copied();
    #[cfg(not(all(feature = "std", not(feature = "complex-scripts"))))]
    None
}

/// The dictionary language a character's script asks for, and whether its words are its line
/// break opportunities (class SA).
fn language(script: Script) -> Option<(&'static str, bool)> {
    Some(match script {
        Script::Thai => ("th", true),
        Script::Lao => ("lo", true),
        Script::Khmer => ("km", true),
        Script::Myanmar => ("my", true),
        Script::Han | Script::Hiragana | Script::Katakana => ("ja", false),
        _ => return None,
    })
}

/// A mark, a joiner or a length mark (ー) carries on the run before it.
fn continues(properties: Properties, ch: char) -> bool {
    matches!(properties.script(), Script::Inherited)
        || matches!(
            properties.general_category(),
            GeneralCategory::NonspacingMark | GeneralCategory::SpacingMark | GeneralCategory::EnclosingMark
        )
        || matches!(ch, '\u{200C}' | '\u{200D}' | '\u{30FC}')
}

/// The text's dictionary runs and the host's word boundaries inside them.
#[derive(Default)]
pub(crate) struct Words {
    /// `(start, end, sa)` byte ranges, ascending.
    runs: Vec<(usize, usize, bool)>,
    /// Boundaries strictly inside the runs, ascending.
    bounds: Vec<usize>,
    run: usize,
    bound: usize,
}

impl Words {
    pub(crate) fn of(text: &str, properties: impl Fn(char) -> Properties) -> Self {
        let Some(segment) = segmenter() else { return Self::default() };
        let mut words = Self::default();
        let mut open: Option<(usize, &'static str, bool)> = None;
        let close = |words: &mut Self, start: usize, end: usize, lang: &'static str, sa: bool| {
            let run = &text[start..end];
            // A single character has no boundary inside it.
            if run.chars().nth(1).is_none() {
                return;
            }
            let mut found = Vec::new();
            segment(run, lang, &mut found);
            words.bounds.extend(found.into_iter().filter(|&b| b > 0 && b < run.len() && run.is_char_boundary(b)).map(|b| b + start));
            words.runs.push((start, end, sa));
        };
        for (at, ch) in text.char_indices() {
            let props = properties(ch);
            if let Some((start, lang, sa)) = open {
                if continues(props, ch) || language(props.script()).is_some_and(|(l, _)| l == lang) {
                    continue;
                }
                close(&mut words, start, at, lang, sa);
            }
            open = language(props.script()).map(|(lang, sa)| (at, lang, sa));
        }
        if let Some((start, lang, sa)) = open {
            close(&mut words, start, text.len(), lang, sa);
        }
        words
    }

    /// The boundaries inside Thai, Lao, Khmer and Burmese runs: their line break opportunities.
    pub(crate) fn line_breaks(&self) -> impl Iterator<Item = usize> + '_ {
        let mut run = 0;
        self.bounds.iter().copied().filter(move |&b| {
            while self.runs.get(run).is_some_and(|r| r.1 <= b) {
                run += 1;
            }
            self.runs.get(run).is_some_and(|r| r.0 < b && r.2)
        })
    }

    /// Inside a run (past its first character), whether `at` is a word boundary; `None` outside
    /// one, where the rule-based segmenter decides. Asked in ascending order.
    pub(crate) fn word_at(&mut self, at: usize) -> Option<bool> {
        while self.runs.get(self.run).is_some_and(|r| r.1 <= at) {
            self.run += 1;
        }
        let run = self.runs.get(self.run)?;
        if at <= run.0 {
            return None;
        }
        while self.bounds.get(self.bound).is_some_and(|&b| b < at) {
            self.bound += 1;
        }
        Some(self.bounds.get(self.bound) == Some(&at))
    }
}
