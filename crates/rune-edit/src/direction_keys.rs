//! Ctrl+Left Shift and Ctrl+Right Shift on Windows: a field's direction, as Notepad,
//! Word and Chrome (`ui/base/ime/win/input_method_win_base.cc`) set an edit control's.
//! Ctrl first, then one Shift pressed together and released with no other key or modifier
//! between them set the field left to right (Left Shift) or right to left (Right Shift),
//! every line of it, aligned to that side at once, an empty field too. Any other key or
//! modifier while they're held, or focus loss, disarms it. The runtime gates the chord on
//! an installed right-to-left keyboard layout (`GetKeyboardLayoutList`), and non-Windows
//! platforms have no such gesture, matching Chrome.

use rune_text::Flow;

use super::{KeySide, Modifier, TextEdit};

/// The direction a field's lines read in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum FieldDir {
    /// As its text and the element give it.
    #[default]
    Natural,
    /// Set by Ctrl+Shift: every line, whatever its words.
    Forced(Flow),
}

/// How far the Ctrl+Shift chord has gone.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DirChord {
    #[default]
    Idle,
    /// Ctrl is down, nothing else.
    Ctrl,
    /// Ctrl and one Shift are down, nothing else: its release sets the direction.
    Armed(KeySide),
}

/// One modifier key going down or up (`ctrl` and `shift` the flags after it): the next state,
/// and the direction a release sets. Active on Windows only, matching Chrome.
#[doc(hidden)]
pub fn step_dir_chord(state: DirChord, modifier: Modifier, pressed: bool, ctrl: bool, shift: bool) -> (DirChord, Option<Flow>) {
    if !cfg!(target_os = "windows") {
        return (DirChord::Idle, None);
    }
    step_dir_chord_win(state, modifier, pressed, ctrl, shift)
}

fn step_dir_chord_win(state: DirChord, modifier: Modifier, pressed: bool, ctrl: bool, shift: bool) -> (DirChord, Option<Flow>) {
    use Modifier::*;
    match (state, modifier, pressed) {
        (DirChord::Idle, ControlLeft | ControlRight, true) if !shift => (DirChord::Ctrl, None),
        (DirChord::Ctrl, ShiftLeft, true) if ctrl => (DirChord::Armed(KeySide::Left), None),
        (DirChord::Ctrl, ShiftRight, true) if ctrl => (DirChord::Armed(KeySide::Right), None),
        // Shift first, then Ctrl, does not arm it (matching Chrome's InputMethodWinBase).
        (DirChord::Idle, ControlLeft | ControlRight, true) => (DirChord::Idle, None),
        (DirChord::Armed(side), ShiftLeft | ShiftRight | ControlLeft | ControlRight, false) => {
            (DirChord::Idle, Some(if side == KeySide::Left { Flow::Ltr } else { Flow::Rtl }))
        }
        (DirChord::Armed(_), _, true) => (DirChord::Idle, None),
        (DirChord::Ctrl, ControlLeft | ControlRight, false) => {
            (if ctrl { DirChord::Ctrl } else { DirChord::Idle }, None)
        }
        (DirChord::Ctrl, ControlLeft | ControlRight, true) => (DirChord::Ctrl, None),
        // Any other modifier pressed while Ctrl is held (Alt, Super, Fn) disarms the chord.
        (DirChord::Ctrl, _, true) => (DirChord::Idle, None),
        (s @ DirChord::Ctrl, _, false) => (s, None),
        _ => (DirChord::Idle, None),
    }
}

impl TextEdit {
    /// The direction Ctrl+Shift set, if it set one.
    #[doc(hidden)]
    pub fn forced_dir(&self) -> Option<Flow> {
        match self.dir {
            FieldDir::Natural => None,
            FieldDir::Forced(flow) => Some(flow),
        }
    }

    /// Sets every line's direction, as Ctrl+Shift does.
    #[doc(hidden)]
    pub fn force_dir(&mut self, flow: Flow) {
        self.dir = FieldDir::Forced(flow);
    }

    /// Clears a direction forced by Ctrl+Shift, returning the field to its element's and text's
    /// natural direction while keeping its text and undo history.
    pub fn clear_dir(&mut self) {
        self.dir = FieldDir::Natural;
    }
}

#[cfg(test)]
#[path = "direction_keys_tests.rs"]
mod tests;
