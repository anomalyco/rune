//! Table: a virtualised data table over rows the app keeps, with a sticky header, sortable and
//! resizable columns, row selection and keyboard navigation.
//!
//! The app owns the rows and their order; [`TableState`] in its state keeps what the table adds
//! (the sort shown in the header, columns the user resized, the selection and the keyboard
//! cursor). Only the rows on screen are built ([`El::virtual_rows`]), so 100,000 rows cost what
//! the visible thirty do. A header click sets the sort and calls [`Table::on_sort`], where the
//! app reorders its rows. Dragging a header's right edge resizes its column within its
//! [`min`](TableColumn::min) and [`max`](TableColumn::max); a double click on the edge puts it
//! back. The table is one focus stop: Up and Down move the selection, Home and End and Page Up
//! and Page Down jump, Return opens, and with [`TableState::multi_select`] Shift extends,
//! Cmd-click toggles and Cmd-A selects every row.
//!
//! ```ignore
//! struct App { files: Vec<File>, table: TableState }
//! Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.files.len(), |s: &App, i| s.files[i].id.as_str().into())
//!     .column(TableColumn::new("name", "Name").flex(2.0).min(160.0).sortable())
//!     .column(TableColumn::new("size", "Size").width(90.0).align(Align::End).sortable())
//!     .cell(|s: &App, i, col| match col {
//!         "name" => Cell::text(&s.files[i].name),
//!         _ => Cell::text(format_size(s.files[i].size)).mono(),
//!     })
//!     .on_sort(|s: &mut App, col, dir| s.sort_files(col, dir))
//!     .build(s, &kit)
//! ```

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use rune::{Align, Color, Drag, El, KeyEvent, Phase, TextStyle, Tx};

use crate::Kit;
use crate::grip::Grip;
use crate::list_nav::{NavAction, NavPolicy, page_size};

mod view;

pub use view::Table;

/// Which way a column sorts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SortDir {
    Ascending,
    Descending,
}

impl SortDir {
    pub fn flip(self) -> SortDir {
        match self {
            SortDir::Ascending => SortDir::Descending,
            SortDir::Descending => SortDir::Ascending,
        }
    }
    /// Applies the direction to an ascending comparison.
    pub fn apply(self, o: std::cmp::Ordering) -> std::cmp::Ordering {
        match self {
            SortDir::Ascending => o,
            SortDir::Descending => o.reverse(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Width {
    Fixed(f32),
    Flex(f32),
}

/// A column: its id (what [`Table::cell`] and [`Table::on_sort`] receive), header title, width,
/// alignment and whether it sorts and resizes.
#[derive(Clone, Debug)]
pub struct TableColumn {
    id: String,
    title: String,
    width: Width,
    min: f32,
    max: f32,
    align: Align,
    sortable: bool,
    resizable: bool,
}

impl TableColumn {
    /// A column sharing the free width with the other flex columns (weight 1), at least 48
    /// points wide, resizable and not sortable.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> TableColumn {
        TableColumn {
            id: id.into(),
            title: title.into(),
            width: Width::Flex(1.0),
            min: 48.0,
            max: f32::INFINITY,
            align: Align::Start,
            sortable: false,
            resizable: true,
        }
    }
    /// A fixed width in points.
    pub fn width(mut self, w: f32) -> Self {
        self.width = Width::Fixed(w.max(0.0));
        self
    }
    /// A share of the width the fixed columns leave, by weight (1 by default).
    pub fn flex(mut self, weight: f32) -> Self {
        self.width = Width::Flex(weight.max(0.0));
        self
    }
    /// The narrowest the column gets, by layout or by dragging.
    pub fn min(mut self, w: f32) -> Self {
        self.min = w.max(0.0);
        self
    }
    /// The widest the column gets.
    pub fn max(mut self, w: f32) -> Self {
        self.max = w.max(0.0);
        self
    }
    /// Where cells and the title sit: numbers read best at the end.
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
    /// A header click sorts by this column.
    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }
    /// Whether dragging the header's edge resizes the column (on by default).
    pub fn resizable(mut self, on: bool) -> Self {
        self.resizable = on;
        self
    }
    /// The column's id (what [`Table::cell`], [`Table::on_sort`], and [`TableState`] use).
    pub fn id(&self) -> &str {
        &self.id
    }
    /// The column's header title.
    ///
    /// ```
    /// use rune_kit::TableColumn;
    /// let col = TableColumn::new("size", "Size");
    /// assert_eq!(col.title(), "Size");
    /// ```
    pub fn title(&self) -> &str {
        &self.title
    }
    /// The declared fixed width in points, or `None` for a flex column.
    ///
    /// ```
    /// use rune_kit::TableColumn;
    /// assert_eq!(TableColumn::new("size", "Size").width(90.0).fixed_width(), Some(90.0));
    /// assert_eq!(TableColumn::new("name", "Name").flex(2.0).fixed_width(), None);
    /// ```
    pub fn fixed_width(&self) -> Option<f32> {
        match self.width {
            Width::Fixed(w) => Some(w),
            Width::Flex(_) => None,
        }
    }
    /// The declared flex weight, or `None` for a fixed-width column.
    ///
    /// ```
    /// use rune_kit::TableColumn;
    /// assert_eq!(TableColumn::new("name", "Name").flex(2.0).flex_weight(), Some(2.0));
    /// assert_eq!(TableColumn::new("size", "Size").width(90.0).flex_weight(), None);
    /// ```
    pub fn flex_weight(&self) -> Option<f32> {
        match self.width {
            Width::Flex(g) => Some(g),
            Width::Fixed(_) => None,
        }
    }
    /// The narrowest width in points (default `48.0`).
    ///
    /// ```
    /// use rune_kit::TableColumn;
    /// assert_eq!(TableColumn::new("name", "Name").min(120.0).min_width(), 120.0);
    /// ```
    pub fn min_width(&self) -> f32 {
        self.min
    }
    /// The widest width in points (default `f32::INFINITY`).
    ///
    /// ```
    /// use rune_kit::TableColumn;
    /// assert_eq!(TableColumn::new("size", "Size").max(240.0).max_width(), 240.0);
    /// ```
    pub fn max_width(&self) -> f32 {
        self.max
    }
    /// The cell and title alignment (default [`Align::Start`]).
    ///
    /// ```
    /// use rune::Align;
    /// use rune_kit::TableColumn;
    /// assert_eq!(TableColumn::new("size", "Size").align(Align::End).alignment(), Align::End);
    /// ```
    pub fn alignment(&self) -> Align {
        self.align
    }
    /// Whether clicking this column's header sorts by it (default `false`).
    ///
    /// ```
    /// use rune_kit::TableColumn;
    /// assert!(TableColumn::new("name", "Name").sortable().is_sortable());
    /// assert!(!TableColumn::new("kind", "Kind").is_sortable());
    /// ```
    pub fn is_sortable(&self) -> bool {
        self.sortable
    }
    /// Whether dragging this column's header edge resizes it (default `true`).
    ///
    /// ```
    /// use rune_kit::TableColumn;
    /// assert!(TableColumn::new("name", "Name").is_resizable());
    /// assert!(!TableColumn::new("kind", "Kind").resizable(false).is_resizable());
    /// ```
    pub fn is_resizable(&self) -> bool {
        self.resizable
    }
    /// `w` within the column's min and max.
    pub fn clamp(&self, w: f32) -> f32 {
        w.max(self.min).min(self.max.max(self.min))
    }
    /// Sizes `el` (a header or body cell) to this column's declared width or flex weight and
    /// min/max bounds, using `override_width` (for example from [`TableState::column_width`])
    /// clamped via [`Self::clamp`] when present.
    ///
    /// ```
    /// use rune::row;
    /// use rune_kit::{TableColumn, TableState};
    /// let col = TableColumn::new("name", "Name").flex(1.0).min(120.0).max(320.0);
    /// let state = TableState::new();
    /// let _cell = col.size_el(row(), state.column_width(col.id()));
    /// ```
    pub fn size_el(&self, el: El, override_width: Option<f32>) -> El {
        self.apply_width(el, self.resolved_width(override_width))
    }

    fn resolved_width(&self, override_width: Option<f32>) -> Width {
        override_width.map_or(self.width, |w| Width::Fixed(self.clamp(w)))
    }

    fn apply_width(&self, el: El, w: Width) -> El {
        let el = match w {
            Width::Fixed(w) => el.w(w),
            Width::Flex(g) => el.w(0.0).grow(g).shrink(),
        };
        let el = el.min_w(self.min);
        if self.max.is_finite() { el.max_w(self.max) } else { el }
    }
}

/// A cell's content: text that truncates (its whole text shows as a tooltip when cut short), or
/// any element.
pub struct Cell {
    content: CellContent,
    color: Option<Color>,
    mono: bool,
    muted: bool,
}

enum CellContent {
    Text(String),
    El(El),
}

impl Cell {
    pub fn text(t: impl Into<String>) -> Cell {
        Cell { content: CellContent::Text(t.into()), color: None, mono: false, muted: false }
    }
    pub fn el(e: El) -> Cell {
        Cell { content: CellContent::El(e), color: None, mono: false, muted: false }
    }
    /// The monospaced face with tabular digits, for ids, hashes and sizes.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }
    /// The theme's secondary text colour.
    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
}

impl From<String> for Cell {
    fn from(t: String) -> Cell {
        Cell::text(t)
    }
}
impl From<&str> for Cell {
    fn from(t: &str) -> Cell {
        Cell::text(t)
    }
}

/// What the table adds to the app's rows, kept in the app's state: the sort, resized columns,
/// the selection and the keyboard cursor. Rows are named by the ids [`Table::new`]'s `row_id`
/// gives, so the selection follows a row through a re-sort.
pub struct TableState {
    key: String,
    multi: bool,
    sort: Option<(String, SortDir)>,
    widths: HashMap<String, f32>,
    hidden: HashSet<String>,
    /// Each column's laid-out width, for starting a drag.
    laid_out: HashMap<String, f32>,
    resize: Grip<ColumnDrag>,
    selected: HashSet<String>,
    cursor: Option<String>,
    /// Where the cursor's row was last seen, to find it again without a scan.
    cursor_hint: std::cell::Cell<usize>,
    anchor: Option<String>,
    /// Where the anchor's row was last seen, to extend selections without a scan.
    anchor_hint: std::cell::Cell<usize>,
    viewport: f32,
    row_height: f32,
}

impl Default for TableState {
    fn default() -> Self {
        Self::new()
    }
}

impl TableState {
    pub fn new() -> TableState {
        TableState {
            key: "table".into(),
            multi: false,
            sort: None,
            widths: HashMap::new(),
            hidden: HashSet::new(),
            laid_out: HashMap::new(),
            resize: Grip::Idle,
            selected: HashSet::new(),
            cursor: None,
            cursor_hint: std::cell::Cell::new(0),
            anchor: None,
            anchor_hint: std::cell::Cell::new(0),
            viewport: 0.0,
            row_height: 28.0,
        }
    }
    /// The table's element key (`"table"` by default). Its rows are keyed by id inside it, and
    /// [`Self::reveal`] looks for the row there, so give each table on screen its own.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }
    /// Several rows can be selected (Shift, Cmd-click, Cmd-A).
    pub fn multi_select(mut self, on: bool) -> Self {
        self.multi = on;
        self
    }

    /// The sort the header shows: a column id and direction.
    pub fn sort(&self) -> Option<(&str, SortDir)> {
        self.sort.as_ref().map(|(c, d)| (c.as_str(), *d))
    }
    pub fn set_sort(&mut self, column: impl Into<String>, dir: SortDir) {
        self.sort = Some((column.into(), dir));
    }
    pub fn clear_sort(&mut self) {
        self.sort = None;
    }
    /// What a click on `column`'s header sorts by: the other way if it's the sort column,
    /// otherwise ascending.
    pub fn next_sort(&self, column: &str) -> SortDir {
        match &self.sort {
            Some((c, d)) if c == column => d.flip(),
            _ => SortDir::Ascending,
        }
    }

    /// The width the user gave `column` by dragging, if any.
    pub fn column_width(&self, column: &str) -> Option<f32> {
        self.widths.get(column).copied()
    }
    /// Sets `column`'s width as a drag does (the table clamps it to the column's bounds).
    pub fn set_column_width(&mut self, column: impl Into<String>, w: f32) {
        self.widths.insert(column.into(), w);
    }
    /// Puts `column` back to its declared width.
    pub fn reset_column_width(&mut self, column: &str) {
        self.widths.remove(column);
    }

    /// Hides `column` from the table's header, rows, and wide scroll width while preserving any
    /// resized width in [`Self::column_width`].
    ///
    /// ```
    /// use rune_kit::TableState;
    /// let mut state = TableState::new();
    /// state.set_column_width("size", 140.0);
    /// state.hide_column("size");
    /// assert!(state.is_column_hidden("size"));
    /// assert_eq!(state.column_width("size"), Some(140.0));
    /// ```
    pub fn hide_column(&mut self, column: impl Into<String>) {
        self.hidden.insert(column.into());
    }
    /// Shows a previously hidden `column`, restoring any resized width it had before hiding.
    ///
    /// ```
    /// use rune_kit::TableState;
    /// let mut state = TableState::new();
    /// state.hide_column("size");
    /// state.show_column("size");
    /// assert!(!state.is_column_hidden("size"));
    /// ```
    pub fn show_column(&mut self, column: &str) {
        self.hidden.remove(column);
    }
    /// Sets whether `column` is hidden (`true`) or visible (`false`), preserving any resized
    /// width in [`Self::column_width`].
    ///
    /// ```
    /// use rune_kit::TableState;
    /// let mut state = TableState::new();
    /// state.set_column_hidden("kind", true);
    /// assert!(state.is_column_hidden("kind"));
    /// state.set_column_hidden("kind", false);
    /// assert!(!state.is_column_hidden("kind"));
    /// ```
    pub fn set_column_hidden(&mut self, column: impl Into<String>, hidden: bool) {
        if hidden {
            self.hidden.insert(column.into());
        } else {
            self.hidden.remove(&column.into());
        }
    }
    /// Whether `column` is currently hidden.
    ///
    /// ```
    /// use rune_kit::TableState;
    /// let mut state = TableState::new();
    /// assert!(!state.is_column_hidden("name"));
    /// state.hide_column("name");
    /// assert!(state.is_column_hidden("name"));
    /// ```
    pub fn is_column_hidden(&self, column: &str) -> bool {
        self.hidden.contains(column)
    }

    pub fn selected(&self) -> impl Iterator<Item = &str> {
        self.selected.iter().map(String::as_str)
    }
    pub fn selected_len(&self) -> usize {
        self.selected.len()
    }
    pub fn is_selected(&self, id: &str) -> bool {
        self.selected.contains(id)
    }
    /// The keyboard cursor's row.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }
    /// Selects `id` alone and puts the cursor on it.
    pub fn select(&mut self, id: impl Into<String>) {
        let id = id.into();
        self.selected.clear();
        self.selected.insert(id.clone());
        self.cursor = Some(id.clone());
        self.anchor = Some(id);
    }
    pub fn clear_selection(&mut self) {
        self.selected.clear();
    }
    /// Selects `id` and scrolls its row into view.
    pub fn reveal(&mut self, id: impl Into<String>, tx: &mut Tx) {
        let id = id.into();
        tx.scroll_into_view_in(self.key.clone(), id.clone());
        self.select(id);
    }

    fn find_at(id: &str, hint: &std::cell::Cell<usize>, len: usize, id_at: &IdAt<'_>) -> Option<usize> {
        let h = hint.get();
        if h < len && id_at(h) == id {
            return Some(h);
        }
        let i = (0..len).find(|&i| id_at(i) == id)?;
        hint.set(i);
        Some(i)
    }

    /// The cursor's row index among `len` rows.
    fn cursor_row(&self, len: usize, id_at: &IdAt<'_>) -> Option<usize> {
        Self::find_at(self.cursor.as_deref()?, &self.cursor_hint, len, id_at)
    }

    fn anchor_row(&self, len: usize, id_at: &IdAt<'_>) -> Option<usize> {
        Self::find_at(self.anchor.as_deref()?, &self.anchor_hint, len, id_at)
    }

    /// A click on row `i` with `mods`, as [`Table`] handles it.
    fn click(&mut self, i: usize, len: usize, mods: rune::Mods, id_at: &IdAt<'_>) {
        let id = id_at(i).into_owned();
        if self.multi && mods.shift {
            return self.extend(i, len, id_at);
        }
        self.cursor_hint.set(i);
        self.anchor_hint.set(i);
        if self.multi && mods.cmd {
            if !self.selected.remove(&id) {
                self.selected.insert(id.clone());
            }
            self.cursor = Some(id.clone());
            self.anchor = Some(id);
            return;
        }
        self.select(id);
    }

    /// Selects from the anchor to row `i`.
    fn extend(&mut self, i: usize, len: usize, id_at: &IdAt<'_>) {
        let anchor = self.anchor_row(len, id_at).unwrap_or_else(|| {
            self.anchor = Some(id_at(i).into_owned());
            self.anchor_hint.set(i);
            i
        });
        let (a, b) = (anchor.min(i), anchor.max(i));
        self.selected = (a..=b).map(|j| id_at(j).into_owned()).collect();
        self.cursor = Some(id_at(i).into_owned());
        self.cursor_hint.set(i);
    }

    /// Applies a key to `len` rows named by `id_at`. Returns `Some(row)` for Return on a row.
    fn handle_key(&mut self, ev: &KeyEvent, len: usize, id_at: &IdAt<'_>, tx: &mut Tx) -> KeyOutcome {
        if len == 0 {
            return KeyOutcome::Ignored;
        }
        let at = self.cursor_row(len, id_at);
        let page = page_size(self.viewport, self.row_height);
        let Some(action) = NavPolicy::list(self.multi).decode(ev, page) else {
            return KeyOutcome::Ignored;
        };
        let (target, extend) = match action {
            NavAction::SelectAll => {
                self.selected = (0..len).map(|i| id_at(i).into_owned()).collect();
                return KeyOutcome::Handled;
            }
            NavAction::Activate => {
                return match at {
                    Some(_) if tx.key_repeat() => KeyOutcome::Handled,
                    Some(r) => KeyOutcome::Open(r),
                    None => KeyOutcome::Ignored,
                };
            }
            NavAction::Escape if self.multi && self.selected.len() > 1 => {
                if let Some(r) = at {
                    self.cursor_hint.set(r);
                    self.anchor_hint.set(r);
                    self.select(id_at(r).into_owned());
                } else {
                    self.clear_selection();
                }
                return KeyOutcome::Handled;
            }
            NavAction::Escape => return KeyOutcome::Ignored,
            NavAction::Move { step, extend } => (step.target(at, len, false).unwrap(), extend),
        };
        let before = self.cursor.clone();
        if extend && self.multi {
            self.extend(target, len, id_at);
        } else {
            self.cursor_hint.set(target);
            self.anchor_hint.set(target);
            self.select(id_at(target).into_owned());
        }
        tx.scroll_into_view_in(self.key.clone(), id_at(target).into_owned());
        if self.cursor != before {
            KeyOutcome::Moved
        } else {
            KeyOutcome::Handled
        }
    }

    fn report_column_width(&mut self, id: &str, width: f32, tx: &mut Tx) {
        if self.laid_out.get(id).copied() == Some(width) {
            tx.live_only();
        } else {
            self.laid_out.insert(id.to_owned(), width);
        }
    }

    fn sort_click(&mut self, id: &str) -> SortDir {
        let dir = self.next_sort(id);
        self.set_sort(id, dir);
        dir
    }

    fn update_viewport(&mut self, height: f32, row_height: f32, tx: &mut Tx) {
        if self.viewport == height && self.row_height == row_height {
            tx.live_only();
        }
        self.viewport = height;
        self.row_height = row_height;
    }

    fn resize_drag(&mut self, c: &TableColumn, d: &Drag) {
        let id = &c.id;
        match d.phase {
            Phase::Down => {
                let (restore, laid_out) = (self.column_width(id), self.laid_out.get(id).copied());
                self.resize.press(ColumnDrag { restore, from: restore.or(laid_out).unwrap_or(c.min) }, Some(d.window_point()));
            }
            Phase::Move => if let Some(drag) = self.resize.moved(d.window_point()).copied() {
                self.set_column_width(id.clone(), c.clamp(drag.from + d.dx));
            },
            Phase::Up => match self.resize.release() {
                Some(ColumnDrag { restore: Some(w), .. }) if d.cancelled() => self.set_column_width(id.clone(), w),
                Some(_) if d.cancelled() => self.reset_column_width(id),
                Some(drag) => self.set_column_width(id.clone(), c.clamp(drag.from + d.dx)),
                None => {}
            },
        }
    }
}

enum KeyOutcome {
    Ignored,
    Handled,
    Moved,
    Open(usize),
}

/// A drag on a column's edge: the width a cancel puts back (`None`: none of its own), and the
/// width it started from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ColumnDrag { restore: Option<f32>, from: f32 }

/// A table's metrics and colours. [`TableStyle::from_kit`] takes them from the theme;
/// `MarkdownStyle::table_like` draws Markdown tables with the same font, padding and rules.
#[derive(Clone, Copy, Debug)]
pub struct TableStyle {
    pub row_height: f32,
    pub header_height: f32,
    /// Space either side of a cell's content.
    pub cell_pad: f32,
    pub font: TextStyle,
    pub mono: TextStyle,
    pub header_font: TextStyle,
    pub text: Color,
    pub muted: Color,
    pub header_text: Color,
    /// The rule under the header and between header cells.
    pub rule: Color,
    pub selected: Color,
    /// Every other row's fill, when striped ([`Table::zebra`]).
    pub stripe: Color,
}

impl TableStyle {
    pub fn from_kit(k: &Kit) -> TableStyle {
        let c = &k.c;
        TableStyle {
            row_height: 28.0,
            header_height: 30.0,
            cell_pad: 10.0,
            font: k.ty.text_small,
            mono: k.ty.mono.features("'tnum' 1"),
            header_font: k.ty.text_small_medium,
            text: c.text_base,
            muted: c.text_muted,
            header_text: c.text_muted,
            rule: c.border_weak,
            selected: k.alpha(c.text_accent, if k.dark { 0.24 } else { 0.2 }),
            stripe: k.alpha(c.text_base, if k.dark { 0.035 } else { 0.03 }),
        }
    }
}

/// Names row `i`.
type IdAt<'a> = dyn Fn(usize) -> Cow<'a, str> + 'a;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_clamps_to_its_bounds() {
        let c = TableColumn::new("a", "A").min(60.0).max(200.0);
        assert_eq!(c.clamp(10.0), 60.0);
        assert_eq!(c.clamp(120.0), 120.0);
        assert_eq!(c.clamp(900.0), 200.0);
        let odd = TableColumn::new("b", "B").min(80.0).max(40.0);
        assert_eq!(odd.clamp(10.0), 80.0, "a max under the min yields to the min");
    }

    #[test]
    fn a_header_click_sorts_ascending_then_flips() {
        let mut st = TableState::new();
        assert_eq!(st.next_sort("size"), SortDir::Ascending);
        st.set_sort("size", SortDir::Ascending);
        assert_eq!(st.next_sort("size"), SortDir::Descending);
        assert_eq!(st.next_sort("name"), SortDir::Ascending, "another column starts ascending");
    }

    #[test]
    fn column_getters_reflect_defaults_and_builders() {
        let default = TableColumn::new("name", "Name");
        assert_eq!(default.id(), "name");
        assert_eq!(default.title(), "Name");
        assert_eq!(default.fixed_width(), None);
        assert_eq!(default.flex_weight(), Some(1.0));
        assert_eq!(default.min_width(), 48.0);
        assert_eq!(default.max_width(), f32::INFINITY);
        assert_eq!(default.alignment(), Align::Start);
        assert!(!default.is_sortable());
        assert!(default.is_resizable());

        let fixed = TableColumn::new("size", "Size")
            .width(96.0)
            .min(60.0)
            .max(180.0)
            .align(Align::End)
            .sortable()
            .resizable(false);
        assert_eq!(fixed.id(), "size");
        assert_eq!(fixed.title(), "Size");
        assert_eq!(fixed.fixed_width(), Some(96.0));
        assert_eq!(fixed.flex_weight(), None);
        assert_eq!(fixed.min_width(), 60.0);
        assert_eq!(fixed.max_width(), 180.0);
        assert_eq!(fixed.alignment(), Align::End);
        assert!(fixed.is_sortable());
        assert!(!fixed.is_resizable());

        let weighted = TableColumn::new("desc", "Description").flex(2.5);
        assert_eq!(weighted.fixed_width(), None);
        assert_eq!(weighted.flex_weight(), Some(2.5));
    }

    #[test]
    fn hiding_and_showing_columns_preserves_resized_widths() {
        let mut st = TableState::new();
        assert!(!st.is_column_hidden("size"));
        st.set_column_width("size", 144.0);
        st.hide_column("size");
        assert!(st.is_column_hidden("size"));
        assert_eq!(st.column_width("size"), Some(144.0));
        st.show_column("size");
        assert!(!st.is_column_hidden("size"));
        assert_eq!(st.column_width("size"), Some(144.0));
        st.set_column_hidden("size", true);
        assert!(st.is_column_hidden("size"));
        st.set_column_hidden("size", false);
        assert!(!st.is_column_hidden("size"));
        assert_eq!(st.column_width("size"), Some(144.0));
    }
}
