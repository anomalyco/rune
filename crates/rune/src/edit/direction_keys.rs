//! Ctrl+Left Shift and Ctrl+Right Shift on Windows and Linux: a field's direction, as Notepad,
//! Word and Chrome set an edit control's. Ctrl and one Shift pressed together and released with
//! no other key between them set the field left to right (Left Shift) or right to left (Right
//! Shift), every line of it, aligned to that side at once, an empty field too. Ctrl+Shift+P and
//! the other shortcuts never set it: any other key while they're held disarms it. macOS has no
//! such gesture. The runtime steps the chord on every physical key (`direction_chord`), whoever
//! takes the key, so it works in any field.

use crate::el::Flow;
use crate::{KeySide, Modifier};

use super::TextEdit;

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DirChord {
    #[default]
    Idle,
    /// Ctrl is down, nothing else.
    Ctrl,
    /// Ctrl and one Shift are down, nothing else: its release sets the direction.
    Armed(KeySide),
}

/// One modifier key going down or up (`ctrl` and `shift` the flags after it): the next state,
/// and the direction a release sets.
pub(crate) fn step_dir_chord(state: DirChord, modifier: Modifier, pressed: bool, ctrl: bool, shift: bool) -> (DirChord, Option<Flow>) {
    use Modifier::*;
    match (state, modifier, pressed) {
        (_, _, _) if cfg!(target_os = "macos") => (DirChord::Idle, None),
        (DirChord::Idle, ControlLeft | ControlRight, true) if !shift => (DirChord::Ctrl, None),
        (DirChord::Ctrl, ShiftLeft, true) if ctrl => (DirChord::Armed(KeySide::Left), None),
        (DirChord::Ctrl, ShiftRight, true) if ctrl => (DirChord::Armed(KeySide::Right), None),
        // Shift first, then Ctrl, arms it too.
        (DirChord::Idle, ControlLeft | ControlRight, true) => (DirChord::Idle, None),
        (DirChord::Armed(side), ShiftLeft | ShiftRight | ControlLeft | ControlRight, false) => {
            (DirChord::Idle, Some(if side == KeySide::Left { Flow::Ltr } else { Flow::Rtl }))
        }
        (DirChord::Armed(_), ShiftLeft | ShiftRight | ControlLeft | ControlRight, true) => (DirChord::Idle, None),
        (DirChord::Ctrl, ControlLeft | ControlRight, false) => (DirChord::Idle, None),
        (s @ DirChord::Ctrl, _, _) => (s, None),
        _ => (DirChord::Idle, None),
    }
}

impl TextEdit {
    /// The direction Ctrl+Shift set, if it set one.
    pub(crate) fn forced_dir(&self) -> Option<Flow> {
        match self.dir {
            FieldDir::Natural => None,
            FieldDir::Forced(flow) => Some(flow),
        }
    }

    /// Sets every line's direction, as Ctrl+Shift does.
    pub(crate) fn force_dir(&mut self, flow: Flow) {
        self.dir = FieldDir::Forced(flow);
    }
}
