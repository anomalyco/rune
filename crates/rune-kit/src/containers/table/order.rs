//! Column order in [`TableState`], and dragging a header to move its column.

use rune::{Drag, Phase};

use super::{TableColumn, TableState};

/// Points the pointer travels before a press on a header becomes a column drag.
const SLOP: f32 = 4.0;

/// A header held by a press: still a press inside the slop (a click that sorts), then a drag
/// that carries the column from its place `from` among the visible columns, `dx` points sideways
/// from where it was pressed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ColumnLift {
    Pressed { index: usize },
    Dragging { from: usize, dx: f32 },
}

impl TableState {
    /// The column order the user chose by dragging headers (or [`Self::set_column_order`] set),
    /// as column ids. Empty while the columns stand in the order the table declares them.
    /// Declared columns missing from it follow the listed ones, in declared order.
    ///
    /// ```
    /// use rune_kit::TableState;
    /// let mut state = TableState::new();
    /// assert!(state.column_order().is_empty());
    /// state.set_column_order(["size", "name"]);
    /// assert_eq!(state.column_order(), ["size", "name"]);
    /// ```
    pub fn column_order(&self) -> &[String] {
        &self.order
    }
    /// Sets the column order by id, for example one the app saved. Ids the table doesn't declare
    /// are skipped; declared columns not listed follow, in declared order.
    pub fn set_column_order<I, T>(&mut self, order: I)
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        self.order = order.into_iter().map(Into::into).collect();
    }
    /// Puts the columns back in their declared order.
    pub fn reset_column_order(&mut self) {
        self.order.clear();
    }
    /// The columns the table shows, in the order it shows them: `columns` (as declared) in
    /// [`Self::column_order`], without the hidden ones. Rows an app draws itself under
    /// [`Table::header`](super::Table::header) lay out their cells from these with
    /// [`TableColumn::size_el`] and [`Self::column_width`].
    ///
    /// ```
    /// use rune_kit::{TableColumn, TableState};
    /// let columns = [TableColumn::new("name", "Name"), TableColumn::new("kind", "Kind"), TableColumn::new("size", "Size")];
    /// let mut state = TableState::new();
    /// state.set_column_order(["size"]);
    /// state.hide_column("kind");
    /// let ids: Vec<_> = state.visible_columns(&columns).into_iter().map(|c| c.id().to_owned()).collect();
    /// assert_eq!(ids, ["size", "name"]);
    /// ```
    pub fn visible_columns(&self, columns: &[TableColumn]) -> Vec<TableColumn> {
        self.ordered(columns).into_iter().filter(|c| !self.is_column_hidden(&c.id)).cloned().collect()
    }
    /// Moves the visible column at `from` to `to` (both places among
    /// [`Self::visible_columns`]), as dragging its header does. Hidden columns keep their places
    /// relative to their neighbours. Out of range places change nothing.
    ///
    /// ```
    /// use rune_kit::{TableColumn, TableState};
    /// let columns = [TableColumn::new("a", "A"), TableColumn::new("b", "B"), TableColumn::new("c", "C")];
    /// let mut state = TableState::new();
    /// state.move_column(&columns, 0, 2);
    /// assert_eq!(state.column_order(), ["b", "c", "a"]);
    /// ```
    pub fn move_column(&mut self, columns: &[TableColumn], from: usize, to: usize) {
        let mut visible: Vec<String> = self.visible_columns(columns).into_iter().map(|c| c.id).collect();
        if from == to || from >= visible.len() || to >= visible.len() {
            return;
        }
        let mut all: Vec<String> = self.ordered(columns).into_iter().map(|c| c.id.clone()).collect();
        let id = visible.remove(from);
        all.retain(|c| *c != id);
        let at = match visible.get(to) {
            Some(next) => all.iter().position(|c| c == next),
            None => visible.last().and_then(|last| all.iter().position(|c| c == last)).map(|i| i + 1),
        };
        all.insert(at.unwrap_or(all.len()), id);
        self.order = all;
    }
    /// The width `column`'s header was last laid out at, in points: what a flex column
    /// resolved to. `None` before the header first lays out.
    pub fn laid_out_width(&self, column: &str) -> Option<f32> {
        self.laid_out.get(column).copied()
    }

    /// Every declared column, hidden ones included, in the current order.
    pub(super) fn ordered<'c>(&self, columns: &'c [TableColumn]) -> Vec<&'c TableColumn> {
        let mut out: Vec<&TableColumn> = Vec::with_capacity(columns.len());
        let listed = self.order.iter().filter_map(|id| columns.iter().find(|c| c.id == *id));
        for c in listed.chain(columns) {
            if !out.iter().any(|o| o.id == c.id) {
                out.push(c);
            }
        }
        out
    }

    /// The header being dragged sideways to move its column (once it left the press slop): its
    /// place among the visible columns, and how far the pointer has gone in points. Rows an app
    /// draws under [`Table::header`](super::Table::header) can shift that column's cells by it.
    ///
    /// ```
    /// use rune_kit::TableState;
    /// assert_eq!(TableState::new().dragged_column(), None);
    /// ```
    pub fn dragged_column(&self) -> Option<(usize, f32)> {
        match self.reorder.get() {
            Some(&ColumnLift::Dragging { from, dx }) => Some((from, dx)),
            _ => None,
        }
    }

    /// Whether a press on a header holds a column (pressed or dragging).
    pub(super) fn holding_column(&self) -> bool {
        self.reorder.is_held()
    }

    /// Visible index of the column whose header is currently pressed or being dragged.
    pub(super) fn held_column_index(&self) -> Option<usize> {
        match self.reorder.get() {
            Some(&ColumnLift::Pressed { index }) => Some(index),
            Some(&ColumnLift::Dragging { from, .. }) => Some(from),
            None => None,
        }
    }

    /// The pointer moved to `at` while a header is held: true when this shows the drag's release
    /// was lost (its header unmounted), and the column drag ended.
    pub(super) fn column_pointer(&mut self, at: [f32; 2]) -> bool {
        self.reorder.pointer(at).is_some()
    }

    /// A drag on the header of the visible column at `index`. Returns `(from, to)` once a drop
    /// moved it; the move is already in [`Self::column_order`].
    pub(super) fn reorder_drag(&mut self, columns: &[TableColumn], index: usize, d: &Drag) -> Option<(usize, usize)> {
        self.reorder_step(
            columns,
            index,
            ColumnDragEvent { phase: d.phase, at: d.window_point(), dx: d.dx, cancelled: d.cancelled() },
        )
    }

    fn reorder_step(&mut self, columns: &[TableColumn], index: usize, d: ColumnDragEvent) -> Option<(usize, usize)> {
        match d.phase {
            Phase::Down => {
                self.reorder.press(ColumnLift::Pressed { index }, Some(d.at));
                None
            }
            Phase::Move => {
                let lift = match self.reorder.moved(d.at) {
                    Some(&mut ColumnLift::Pressed { index }) if d.dx.abs() > SLOP => {
                        ColumnLift::Dragging { from: index, dx: d.dx }
                    }
                    Some(&mut ColumnLift::Dragging { from, .. }) => ColumnLift::Dragging { from, dx: d.dx },
                    // Still inside the slop, or nothing held (the grip ended as a lost release).
                    Some(_) | None => return None,
                };
                self.reorder.hold(lift);
                None
            }
            Phase::Up => {
                let Some(ColumnLift::Dragging { from, .. }) = self.reorder.release() else {
                    return None;
                };
                if d.cancelled {
                    return None;
                }
                let widths: Vec<f32> = self
                    .visible_columns(columns)
                    .iter()
                    .map(|c| {
                        self.laid_out_width(&c.id)
                            .unwrap_or_else(|| super::window::resolved_min_width(c, c.resolved_width(self.column_width(&c.id))))
                    })
                    .collect();
                if from >= widths.len() {
                    return None;
                }
                let to = target_slot(&widths, from, d.dx);
                if to == from {
                    return None;
                }
                self.move_column(columns, from, to);
                Some((from, to))
            }
        }
    }
}

/// What a header's drag reports to [`TableState::reorder_step`].
#[derive(Clone, Copy, Debug)]
struct ColumnDragEvent {
    phase: Phase,
    /// The pointer, in window points.
    at: [f32; 2],
    /// Sideways from the press.
    dx: f32,
    cancelled: bool,
}

/// Where the column at `from` lands when its header is dropped `dx` points from where it was
/// pressed: past each neighbour whose middle its own middle crossed.
fn target_slot(widths: &[f32], from: usize, dx: f32) -> usize {
    let mut x = 0.0;
    let middles: Vec<f32> = widths
        .iter()
        .map(|w| {
            let m = x + w / 2.0;
            x += w;
            m
        })
        .collect();
    let dropped = middles[from] + dx;
    middles.iter().enumerate().filter(|&(i, &m)| i != from && m < dropped).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cols(ids: &[&str]) -> Vec<TableColumn> {
        ids.iter().map(|id| TableColumn::new(*id, id.to_uppercase())).collect()
    }

    fn ids(cols: &[TableColumn]) -> Vec<&str> {
        cols.iter().map(|c| c.id()).collect()
    }

    #[test]
    fn a_dropped_header_lands_past_the_middles_it_crossed() {
        let w = [100.0, 100.0, 100.0];
        assert_eq!(target_slot(&w, 0, 40.0), 0);
        assert_eq!(target_slot(&w, 0, 90.0), 0);
        assert_eq!(target_slot(&w, 0, 110.0), 1);
        assert_eq!(target_slot(&w, 0, 260.0), 2);
        assert_eq!(target_slot(&w, 2, -160.0), 1);
        assert_eq!(target_slot(&w, 2, -210.0), 0);
        assert_eq!(target_slot(&w, 1, -1000.0), 0);
    }

    fn drag(phase: Phase, at: [f32; 2], dx: f32) -> ColumnDragEvent {
        ColumnDragEvent { phase, at, dx, cancelled: false }
    }

    #[test]
    fn a_press_inside_the_slop_never_drags_and_a_lost_release_lets_go() {
        let c = cols(&["a", "b", "c"]);
        let mut st = TableState::new();
        assert_eq!(st.reorder_step(&c, 0, drag(Phase::Down, [50.0, 10.0], 0.0)), None);
        st.reorder_step(&c, 0, drag(Phase::Move, [53.0, 10.0], 3.0));
        assert!(st.holding_column() && st.dragged_column().is_none(), "still a press inside the slop");
        assert_eq!(st.reorder_step(&c, 0, drag(Phase::Up, [53.0, 10.0], 3.0)), None);
        assert!(!st.holding_column());

        st.reorder_step(&c, 0, drag(Phase::Down, [50.0, 10.0], 0.0));
        st.reorder_step(&c, 0, drag(Phase::Move, [90.0, 10.0], 40.0));
        assert_eq!(st.dragged_column(), Some((0, 40.0)));
        // The pointer moves on twice without the drag hearing it: its release was lost.
        assert!(!st.column_pointer([120.0, 10.0]), "one unheard move is normal");
        assert!(st.column_pointer([150.0, 10.0]));
        assert!(!st.holding_column() && st.dragged_column().is_none());
        assert!(st.column_order().is_empty(), "a lost release moves nothing");
    }

    #[test]
    fn order_skips_unknown_ids_and_appends_unlisted_columns() {
        let c = cols(&["a", "b", "c", "d"]);
        let mut st = TableState::new();
        st.set_column_order(["c", "zzz", "a", "c"]);
        assert_eq!(ids(&st.visible_columns(&c)), ["c", "a", "b", "d"]);
        st.reset_column_order();
        assert_eq!(ids(&st.visible_columns(&c)), ["a", "b", "c", "d"]);
    }

    #[test]
    fn moving_a_column_keeps_hidden_ones_beside_their_neighbours() {
        let c = cols(&["a", "b", "c", "d"]);
        let mut st = TableState::new();
        st.hide_column("b");
        st.move_column(&c, 0, 2);
        assert_eq!(ids(&st.visible_columns(&c)), ["c", "d", "a"]);
        assert_eq!(st.column_order(), ["b", "c", "d", "a"]);
        st.show_column("b");
        assert_eq!(ids(&st.visible_columns(&c)), ["b", "c", "d", "a"]);
        st.move_column(&c, 3, 9);
        assert_eq!(st.column_order(), ["b", "c", "d", "a"], "out of range moves nothing");
    }
}
