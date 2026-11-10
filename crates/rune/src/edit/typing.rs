//! The style the next text typed at the caret takes.

use super::Mark;

/// What text typed at an empty selection is marked with.
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub(super) enum Typing {
    /// The marks around the caret (`TextEdit::inherited_marks_at`).
    #[default]
    Inherited,
    /// A mark was toggled with nothing selected: text typed with the selection still at `at`
    /// (`(anchor, focus)`) carries exactly `marks`. Anywhere else they're stale, since an app
    /// can move the caret through the public fields without disarming them.
    Armed { at: (usize, usize), marks: Vec<Mark> },
}

impl Typing {
    /// The armed marks, while the selection is still where they were armed.
    pub(super) fn at(&self, selection: (usize, usize)) -> Option<&[Mark]> {
        match self {
            Typing::Armed { at, marks } if *at == selection => Some(marks),
            _ => None,
        }
    }

    /// Disarms, returning the marks if they were armed at `selection`: typed text uses them
    /// up.
    pub(super) fn take_at(&mut self, selection: (usize, usize)) -> Option<Vec<Mark>> {
        match std::mem::take(self) {
            Typing::Armed { at, marks } if at == selection => Some(marks),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "typing_tests.rs"]
mod tests;
