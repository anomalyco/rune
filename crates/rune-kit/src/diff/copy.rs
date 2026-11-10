//! The side a diff's text selection copies: the code the pointer is over, held to the side a
//! press started on while the pointer drags across to the other.
use super::model::Side;

/// The side a text selection copies: the side of the code the pointer is over, held to the one
/// it started on while cells on both sides are entered at once (a press keeps its cell entered
/// as the pointer drags across). Leaving cells doesn't change the side; entering a cell on the
/// other side once every entered cell is there does.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum CopySide {
    Idle(Side),
    /// The cells whose code the pointer is over now (never empty), and the side they copy.
    Hovering { cells: Vec<(Side, String)>, side: Side },
}

impl Default for CopySide {
    fn default() -> Self { CopySide::Idle(Side::New) }
}

impl CopySide {
    pub(super) fn side(&self) -> Side {
        match self {
            CopySide::Idle(side) | CopySide::Hovering { side, .. } => *side,
        }
    }
    /// The pointer entered (`inside`) the text of the line on `side`, named `cell`; returns
    /// whether that changed the side a selection copies.
    pub(super) fn step(&mut self, side: Side, cell: &str, inside: bool) -> bool {
        let was = self.side();
        let current = std::mem::replace(&mut *self, CopySide::Idle(side));
        *self = match (current, inside) {
            (CopySide::Idle(_), true) => CopySide::Hovering { cells: vec![(side, cell.to_owned())], side },
            (CopySide::Hovering { mut cells, side: copy }, true) => {
                if !cells.iter().any(|(_, c)| c == cell) {
                    cells.push((side, cell.to_owned()));
                }
                let copy = if cells.iter().all(|(s, _)| *s == side) { side } else { copy };
                CopySide::Hovering { cells, side: copy }
            }
            (CopySide::Hovering { mut cells, side: copy }, false) => {
                cells.retain(|(_, c)| c != cell);
                if cells.is_empty() { CopySide::Idle(copy) } else { CopySide::Hovering { cells, side: copy } }
            }
            (idle @ CopySide::Idle(_), false) => idle,
        };
        self.side() != was
    }
}
