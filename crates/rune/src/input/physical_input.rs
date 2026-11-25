//! Physical input is queued with its logical fallback, so a preceding click can claim it.
use zeroize::Zeroize;

use crate::{Mods, PhysicalKeyCode};

/// A hardware key, a modifier change, or the window losing focus.
#[derive(Clone, Debug)]
pub enum PhysicalInput {
    Key {
        key: PhysicalKeyCode,
        mods: Mods,
        pressed: bool,
        repeat: bool,
        /// The text the platform produced for this key under the current layout and
        /// modifiers (`å` for ⌥A on a US Mac layout, `A` for ⇧A, the composition on the key
        /// after a dead key). `None` when the key produces no text, such as a dead key that
        /// has not composed yet, an arrow, or a release. A key queued with a logical
        /// `Key::Text` and no `text` gets that text. Zeroized on drop, like `Key::Text`.
        text: Option<String>,
        /// The modifiers the platform spent producing `text`: Alt when the text differs from
        /// the unmodified key, Shift when it does and Ctrl is not held, and Ctrl with Alt for a
        /// Windows AltGr. `logo` is never consumed, nor anything for control text (Enter's `"\r"`,
        /// Tab's `"\t"`). Empty when the host can't tell.
        consumed_mods: Mods,
    },
    Modifiers(Mods),
    FocusLost,
}

impl Drop for PhysicalInput {
    fn drop(&mut self) {
        if let Self::Key { text: Some(text), .. } = self { text.zeroize(); }
    }
}

pub(crate) type Handler = std::rc::Rc<dyn Fn(crate::states::States<'_>, &PhysicalInput, &mut crate::Tx) -> bool>;

impl crate::El {
    /// Receives physical keys before logical editing, app shortcuts and focus navigation.
    /// Return true to consume the event. FocusLost also arrives on window deactivation.
    pub fn on_physical_input<S: 'static>(mut self,
        f: impl Fn(&mut S, &PhysicalInput, &mut crate::Tx) -> bool + 'static) -> Self {
        self.cold_mut().focus.physical = Some(std::rc::Rc::new(move |state, event, tx| {
            state.cast::<S>().is_some_and(|s| f(s, event, tx))
        }));
        self
    }
}
