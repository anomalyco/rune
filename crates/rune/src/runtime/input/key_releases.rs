//! Where key releases go. Those an input method's composition took come back up unheard; the
//! rest go first to the element that had focus when their key went down, so a release after
//! focus moved still reaches it.
use std::num::NonZeroU32;

/// Which key releases belong to an input method.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Eat {
    #[default]
    None,
    /// The release of the key that confirmed or cancelled a composition.
    One,
    /// This many keys went down while a composition was open: their releases are its.
    Keys(NonZeroU32),
}

/// Keys down outside a composition, and the focus where the last of them went down.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Held {
    keys: NonZeroU32,
    focus: Option<String>,
}

#[derive(Default)]
pub(super) struct KeyReleases {
    eat: Eat,
    held: Option<Held>,
}

impl KeyReleases {
    /// A key other than a modifier went down, while a composition is open or not; `focus` is
    /// the focus it went down in.
    pub(super) fn down(&mut self, composing: bool, repeat: bool, focus: impl FnOnce() -> Option<String>) {
        if composing {
            self.eat = match (self.eat, repeat) {
                (Eat::Keys(n), false) => Eat::Keys(n.saturating_add(1)),
                (_, false) => Eat::Keys(NonZeroU32::MIN),
                (Eat::None, true) => Eat::One,
                (eat, true) => eat,
            };
        } else if !repeat {
            self.eat = Eat::None;
            let keys = self.held.as_ref().map_or(NonZeroU32::MIN, |h| h.keys.saturating_add(1));
            self.held = Some(Held { keys, focus: focus() });
        }
    }

    /// The input method took a key: its release comes back up unheard.
    pub(super) fn ime_took_key(&mut self) {
        if self.eat == Eat::None { self.eat = Eat::One; }
    }

    /// A key came up while a composition is open, with no release to route after it. The
    /// release of the key that ends the composition is still the input method's.
    pub(super) fn composing_release(&mut self) {
        if let Eat::Keys(n) = self.eat { self.eat = NonZeroU32::new(n.get() - 1).map_or(Eat::One, Eat::Keys); }
    }

    /// A key came up: whether the input method takes its release.
    pub(super) fn eat(&mut self, composing: bool) -> bool {
        if composing {
            self.composing_release();
            return true;
        }
        match self.eat {
            Eat::Keys(n) => self.eat = NonZeroU32::new(n.get() - 1).map_or(Eat::None, Eat::Keys),
            Eat::One => self.eat = Eat::None,
            Eat::None => return false,
        }
        true
    }

    /// A release the input method didn't take: the focus its key went down in, if a key is
    /// held. The last release ends the hold.
    pub(super) fn route(&mut self) -> Option<String> {
        let held = self.held.as_mut()?;
        let focus = held.focus.clone();
        match NonZeroU32::new(held.keys.get() - 1) {
            Some(keys) => held.keys = keys,
            None => self.held = None,
        }
        focus
    }

    /// The window gets no more keys: nothing is held, and the input method takes nothing.
    pub(super) fn lost(&mut self) { *self = Self::default(); }
}

#[cfg(test)]
#[path = "key_releases_tests.rs"]
mod tests;
