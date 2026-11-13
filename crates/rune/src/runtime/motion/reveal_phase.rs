//! A streaming text's word reveal: `Idle` with every word shown, `Fading` the words that
//! arrived since, and the window's chain of them, which makes one chunk's words cross texts in
//! reading order.
//!
//! A commit says what a text's content is now (`commit/apply.rs`), a frame tick says how far its
//! clock has run (`motion/tracks.rs`), a clock change moves its word times
//! (`motion/loop_clock.rs`), and the node leaving keeps the state for a return, dropping the
//! oldest past `REVEALS_KEPT` (`commit/prune.rs`). The last of those is the only way a text
//! forgets a fade it was in the middle of.
use crate::machine::Effects;

/// How a text arrived at one commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Arrival {
    /// It was here at the last commit.
    Kept,
    /// It is new and shows whole: the commit lands its changes (`Tx::instant`), no input caused
    /// it (`Tx::unprompted`), or it mounted inside a remounting row.
    Landed,
    /// It is new while its entrance still plays: its words fade in from none.
    Entering,
    /// Its content was replaced or cut (`Tx::instant`, a shorter text, a word boundary moved):
    /// what faded is gone and the text shows from its start.
    Replaced,
}

/// What a commit, a frame tick or a clock change says about one streaming text.
pub(super) enum RevealEvent<'a> {
    /// A commit: its content now ends at `content.len()` bytes.
    Commit {
        content: &'a str,
        /// Seconds each word fades in over (`TextSpec::reveal`, or the reduced-motion fade).
        fade: f32,
        arrival: Arrival,
        /// The window's chain of streaming words, for a text on the window's clock; `None` for
        /// one on its own (`El::time_scale`), which queues behind nothing.
        chain: Option<&'a RevealChain>,
        now: f64,
        reduced_motion: bool,
    },
    /// A frame ticked `now` on the text's clock.
    Tick { now: f64 },
    /// The text's clock changed scale: its word times move by `shift`.
    Shift { shift: f64 },
}

/// What a transition asks the caller to carry out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum RevealEffect {
    /// A word starts fading at this time; the window's chain queues the next text behind it.
    Started(f64),
}

/// A streaming text's word reveal.
#[derive(Clone, Debug)]
pub(super) enum RevealPhase {
    /// Caught up: `shown` bytes of its content are visible and no word fades.
    Idle { shown: usize },
    /// The words in `words` (`(start byte, start time)`, oldest first) fade in over `fade`
    /// seconds each; the text shows the first `shown` bytes whole. Never empty: a commit with
    /// no new words leaves the phase `Idle`.
    Fading { shown: usize, words: Vec<(u32, f64)>, fade: f32 },
}

impl Default for RevealPhase {
    fn default() -> Self { RevealPhase::Idle { shown: 0 } }
}

impl RevealPhase {
    /// The state after `event`, and the effects the caller carries out.
    pub(super) fn step(&mut self, event: RevealEvent<'_>) -> Effects<RevealEffect> {
        match event {
            RevealEvent::Tick { now } => {
                // Only once every word has shown does the phase go back to `Idle`: the front of
                // the list leaving alone would move `shown` backwards.
                if let RevealPhase::Fading { shown, words, fade } = self
                    && words.iter().all(|w| now - w.1 >= f64::from(*fade))
                {
                    *self = RevealPhase::Idle { shown: *shown };
                }
                Effects::none()
            }
            RevealEvent::Shift { shift } => {
                if let RevealPhase::Fading { words, .. } = self {
                    for w in words { w.1 += shift; }
                }
                Effects::none()
            }
            RevealEvent::Commit { content, fade, arrival, chain, now, reduced_motion } => {
                self.commit(content, fade, arrival, chain, now, reduced_motion)
            }
        }
    }

    /// A commit's news about the text: its content ends at `content.len()` bytes, and `arrival`
    /// says how it got there.
    fn commit(
        &mut self,
        content: &str,
        fade: f32,
        arrival: Arrival,
        chain: Option<&RevealChain>,
        now: f64,
        reduced_motion: bool,
    ) -> Effects<RevealEffect> {
        // The words a commit keeps, and the byte its fade had already consumed.
        let (mut words, from) = match arrival {
            Arrival::Kept => match std::mem::take(self) {
                RevealPhase::Idle { shown } => (Vec::new(), shown),
                RevealPhase::Fading { shown, words, .. } => (words, shown),
            },
            Arrival::Entering => (Vec::new(), 0),
            Arrival::Landed | Arrival::Replaced => (Vec::new(), content.len()),
        };
        let mut effects = Effects::none();
        if content.len() > from && fade > 0.0 {
            let mut starts = Vec::new();
            word_starts(content, from, &mut starts);
            // A word continuing across the old end keeps its time. New words wait behind every
            // streaming text's newest word, taken in reading order, so one edge moves down the
            // page: a chunk spanning several paragraphs or list items fades them one after
            // another, never all at once. Only while the newest word is still fading: texts
            // mounting at once behind a long run (a remount, a restored turn) fade together as
            // before, never waiting through still frames.
            let own = words.last().map_or(now, |w| w.1.max(now));
            let chain_at = chain.map_or(now, |chain| chain.start(now, fade));
            let queued = ((chain_at - now) / 0.0055) as usize;
            let pending = words.iter().filter(|w| w.1 > now).count() + starts.len() + queued;
            let stagger = if reduced_motion {
                0.0
            } else {
                let k = (pending as f32 / 24.0).clamp(0.0, 1.0);
                (0.020 + (0.0055 - 0.020) * k) as f64
            };
            let mut tn = if chain_at > own { chain_at + stagger } else { own };
            for b in starts {
                words.push((b, tn));
                tn += stagger;
            }
            if let (true, Some(last)) = (chain.is_some(), words.last()) {
                // A text on its own clock keeps its words to itself, and so does the chain.
                effects.push(RevealEffect::Started(last.1));
            }
        }
        *self =
            if words.is_empty() { RevealPhase::Idle { shown: content.len() } }
            else { RevealPhase::Fading { shown: content.len(), words, fade } };
        effects
    }

    /// Every word caught up: nothing fades.
    pub(super) fn fading(&self) -> bool { matches!(self, RevealPhase::Fading { .. }) }

    /// The byte its words have been shown up to.
    pub(super) fn shown(&self) -> usize {
        match *self {
            RevealPhase::Idle { shown } | RevealPhase::Fading { shown, .. } => shown,
        }
    }

    /// Nothing fading, and `end` bytes of content are all shown: a memo or a cache holding this
    /// text holds what the text draws.
    pub(super) fn caught_up(&self, end: usize) -> bool { !self.fading() && self.shown() == end }

    /// The words still fading, oldest first; empty when the text has caught up.
    #[cfg(test)]
    pub(super) fn words(&self) -> &[(u32, f64)] {
        match self {
            RevealPhase::Idle { .. } => &[],
            RevealPhase::Fading { words, .. } => words,
        }
    }

    /// The fade it is in, for paint to carry over a piece's glyphs: taken once per piece, so no
    /// glyph looks at the phase again.
    pub(super) fn fade(&self) -> Option<Fade<'_>> {
        match self {
            RevealPhase::Fading { words, fade, .. } => Some(Fade { words, fade: *fade }),
            RevealPhase::Idle { .. } => None,
        }
    }
}

/// The words still fading and how long each takes.
#[derive(Clone, Copy, Debug)]
pub(super) struct Fade<'a> {
    words: &'a [(u32, f64)],
    fade: f32,
}

impl Fade<'_> {
    /// The alpha of `byte`'s word, `now` on the text's clock: 1 anywhere no word fades.
    pub(super) fn alpha(self, byte: u32, now: f64) -> f32 {
        let i = self.words.partition_point(|w| w.0 <= byte);
        if i == 0 { return 1.0; }
        let t0 = self.words[i - 1].1;
        if self.fade <= 0.0 {
            return if now >= t0 { 1.0 } else { 0.0 };
        }
        ease_out(((now - t0) / self.fade as f64) as f32)
    }
}

/// The window's chain of streaming words: the newest word's start, which the next text's words
/// queue behind.
///
/// It holds a start only while a run still fades: a start more than one fade ahead is a run
/// longer than a fade, and the texts behind it fade together rather than wait it out. A run
/// that ended is replaced by the next word's start, so the chain never drifts into the past.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct RevealChain {
    until: f64,
}

impl RevealChain {
    /// Where `now`'s first new word starts: behind the newest one while it still fades, else
    /// now.
    pub(super) fn start(self, now: f64, fade: f32) -> f64 {
        if self.until - now > f64::from(fade) { now } else { self.until.max(now) }
    }

    /// `at` is a word's start: the chain reaches no further back than it.
    pub(super) fn started(&mut self, at: f64) { self.until = self.until.max(at); }

    /// The newest streaming word's start.
    #[cfg(test)]
    pub(super) fn until(self) -> f64 { self.until }
}

/// Where each word of `s` after `from` starts, as byte offsets.
fn word_starts(s: &str, from: usize, out: &mut Vec<u32>) {
    let mut prev_space = true;
    for (i, c) in s[from..].char_indices() {
        let space = c.is_whitespace();
        if !space && prev_space {
            out.push((from + i) as u32);
        }
        prev_space = space;
    }
}

/// A word's fade curve: fast in, easing to full.
fn ease_out(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    1.0 - (1.0 - x) * (1.0 - x) * (1.0 - x)
}
