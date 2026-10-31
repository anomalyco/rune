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
use std::rc::Rc;

use rune::{Align, Color, Cursor, Drag, El, GrowthSpec, Key, KeyEvent, Phase, ScrollSpec, TextStyle, Tx, col, el, icon, row, text};

use crate::Kit;

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
    pub fn id(&self) -> &str {
        &self.id
    }
    /// `w` within the column's min and max.
    pub fn clamp(&self, w: f32) -> f32 {
        w.max(self.min).min(self.max.max(self.min))
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
    /// Each column's laid-out width, for starting a drag.
    laid_out: HashMap<String, f32>,
    resize_start: Option<Option<f32>>,
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
            laid_out: HashMap::new(),
            resize_start: None,
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
        let m = ev.mods;
        if len == 0 || m.alt || m.ctrl && !m.cmd || m.logo && !m.cmd {
            return KeyOutcome::Ignored;
        }
        if ev.is_command('a') && self.multi {
            self.selected = (0..len).map(|i| id_at(i).into_owned()).collect();
            return KeyOutcome::Handled;
        }
        if m.cmd {
            return KeyOutcome::Ignored;
        }
        let at = self.cursor_row(len, id_at);
        let page = ((self.viewport / self.row_height.max(1.0)).floor() as usize).saturating_sub(1).max(1);
        let target = match ev.key {
            Key::Up => at.map_or(len - 1, |r| r.saturating_sub(1)),
            Key::Down => at.map_or(0, |r| (r + 1).min(len - 1)),
            Key::Home => 0,
            Key::End => len - 1,
            Key::PageUp => at.map_or(0, |r| r.saturating_sub(page)),
            Key::PageDown => at.map_or(0, |r| (r + page).min(len - 1)),
            Key::Enter => {
                return match at {
                    Some(_) if tx.key_repeat() => KeyOutcome::Handled,
                    Some(r) => KeyOutcome::Open(r),
                    None => KeyOutcome::Ignored,
                };
            }
            Key::Escape if self.multi && self.selected.len() > 1 => {
                if let Some(r) = at {
                    self.cursor_hint.set(r);
                    self.anchor_hint.set(r);
                    self.select(id_at(r).into_owned());
                } else {
                    self.clear_selection();
                }
                return KeyOutcome::Handled;
            }
            _ => return KeyOutcome::Ignored,
        };
        let before = self.cursor.clone();
        if m.shift && self.multi {
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
}

enum KeyOutcome {
    Ignored,
    Handled,
    Moved,
    Open(usize),
}

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
type CellFn<S> = Rc<dyn Fn(&S, usize, &str) -> Cell>;
type RowIdFn<S> = for<'a> fn(&'a S, usize) -> Cow<'a, str>;

/// A virtualised table over the app's rows in its state `S`.
pub struct Table<S> {
    get: fn(&S) -> &TableState,
    get_mut: fn(&mut S) -> &mut TableState,
    len: fn(&S) -> usize,
    row_id: RowIdFn<S>,
    columns: Vec<TableColumn>,
    cell: Option<CellFn<S>>,
    on_sort: Option<Rc<dyn Fn(&mut S, &str, SortDir, &mut Tx)>>,
    on_select: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
    on_open: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
    zebra: bool,
    style: Option<TableStyle>,
    name: Option<String>,
}

impl<S: 'static> Table<S> {
    /// `get` and `get_mut` find the [`TableState`]; `len` counts the rows and `row_id` names
    /// row `i` (in the app's current order) with an id unique among them.
    pub fn new(get: fn(&S) -> &TableState, get_mut: fn(&mut S) -> &mut TableState, len: fn(&S) -> usize, row_id: RowIdFn<S>) -> Self {
        Table { get, get_mut, len, row_id, columns: Vec::new(), cell: None, on_sort: None, on_select: None, on_open: None, zebra: false, style: None, name: None }
    }
    pub fn column(mut self, c: TableColumn) -> Self {
        self.columns.push(c);
        self
    }
    /// What row `i` shows in column `col` (the column's id).
    pub fn cell(mut self, f: impl Fn(&S, usize, &str) -> Cell + 'static) -> Self {
        self.cell = Some(Rc::new(f));
        self
    }
    /// A sortable column's header was clicked: the table already shows the new sort; reorder
    /// the rows to match.
    pub fn on_sort(mut self, f: impl Fn(&mut S, &str, SortDir, &mut Tx) + 'static) -> Self {
        self.on_sort = Some(Rc::new(f));
        self
    }
    /// The cursor moved to row `id`, by click or key.
    pub fn on_select(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }
    /// A row was double-clicked, or Return pressed on it.
    pub fn on_open(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.on_open = Some(Rc::new(f));
        self
    }
    /// Fill every other row.
    pub fn zebra(mut self, on: bool) -> Self {
        self.zebra = on;
        self
    }
    pub fn style(mut self, s: TableStyle) -> Self {
        self.style = Some(s);
        self
    }
    /// The table's accessibility label.
    pub fn name(mut self, n: impl Into<String>) -> Self {
        self.name = Some(n.into());
        self
    }

    /// The table, filling the height its parent gives it.
    pub fn build(self, s: &S, k: &Kit) -> El {
        let Table { get, get_mut, len, row_id, columns, cell, on_sort, on_select, on_open, zebra, style, name } = self;
        let style = style.unwrap_or_else(|| TableStyle::from_kit(k));
        let state = get(s);
        let count = len(s);
        let columns = Rc::new(columns);
        let sizes: Rc<Vec<Width>> = Rc::new(columns.iter().map(|c| state.column_width(&c.id).map_or(c.width, |w| Width::Fixed(c.clamp(w)))).collect());

        let header = header_row(k, state, &columns, &sizes, &style, on_sort.clone(), get, get_mut);

        let cols = columns.clone();
        let sz = sizes.clone();
        let sel = on_select.clone();
        let open = on_open.clone();
        let focus = k.c.border_focus;
        let build_row = move |s: &S, i: usize| -> El {
            let st = get(s);
            let id = row_id(s, i);
            let selected = st.is_selected(&id);
            let mut r = row()
                .key(id.to_string())
                .role("row")
                .focus_selected(selected)
                .w_full()
                .h(style.row_height)
                .center_y();
            r = if selected {
                r.fill(style.selected)
            } else if zebra && i % 2 == 1 {
                r.fill(style.stripe).hover(style.stripe.mix(style.selected, 0.5))
            } else {
                r.hover(style.stripe.mix(style.selected, 0.5))
            };
            if st.multi && st.cursor.as_deref() == Some(id.as_ref()) {
                r = r.outline(1.0, -1.0, focus);
            }
            for (j, c) in cols.iter().enumerate() {
                let content = cell.as_ref().map_or_else(|| Cell::text(""), |f| f(s, i, &c.id));
                r = r.child(cell_el(c, sz[j], content, &style).key(c.id.clone()));
            }
            let sel = sel.clone();
            r = r.on_press_tx(move |s: &mut S, tx: &mut Tx| {
                let n = len(s);
                if i >= n {
                    return;
                }
                let mods = tx.mods();
                // Row ids borrow `s`; work on a copy of the state's selection, then store it.
                let mut st = std::mem::take(get_mut(s));
                st.click(i, n, mods, &|j| row_id(s, j));
                *get_mut(s) = st;
                if let Some(f) = &sel {
                    let id = row_id(s, i).into_owned();
                    f(s, &id, tx);
                }
            });
            if let Some(f) = open.clone() {
                r = r.on_double_press_tx(move |s: &mut S, tx: &mut Tx| {
                    if i < len(s) {
                        let id = row_id(s, i).into_owned();
                        f(s, &id, tx);
                    }
                });
            }
            r
        };
        let rows = col()
            .key("rows")
            .w_full()
            .growth(GrowthSpec::default())
            .virtual_rows(count, style.row_height, move |s: &S, i| row_id(s, i), build_row);
        let viewport = col()
            .key("viewport")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: false })
            // Inside a scrolling page this reports every frame of the scroll; only a new height
            // changes what the view reads.
            .on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
                let st = get_mut(s);
                if st.viewport == r[3] && st.row_height == style.row_height { tx.live_only(); }
                st.viewport = r[3];
                st.row_height = style.row_height;
            })
            .child(rows);
        let mut root = col()
            .key(state.key.clone())
            .role("grid")
            .focusable(true)
            .w_full()
            .clip_soft(0.0);
        if let Some(n) = name {
            root = root.label(n);
        }
        root
            .on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
                let n = len(s);
                let mut st = std::mem::take(get_mut(s));
                let out = st.handle_key(ev, n, &|j| row_id(s, j), tx);
                let cursor = st.cursor.clone();
                *get_mut(s) = st;
                match out {
                    KeyOutcome::Ignored => false,
                    KeyOutcome::Handled => true,
                    KeyOutcome::Moved => {
                        if let (Some(f), Some(id)) = (&on_select, cursor) {
                            f(s, &id, tx);
                        }
                        true
                    }
                    KeyOutcome::Open(r) => {
                        if let Some(f) = &on_open {
                            let id = row_id(s, r).into_owned();
                            f(s, &id, tx);
                        }
                        true
                    }
                }
            })
            .child(header)
            .child(viewport)
    }
}

/// Sizes a header or body cell to its column.
fn sized(e: El, c: &TableColumn, w: Width) -> El {
    let e = match w {
        Width::Fixed(w) => e.w(w),
        Width::Flex(g) => e.w(0.0).grow(g).shrink(),
    };
    let e = e.min_w(c.min);
    if c.max.is_finite() { e.max_w(c.max) } else { e }
}

fn justify(e: El, a: Align) -> El {
    match a {
        Align::Start => e,
        Align::Center => e.justify_center(),
        Align::End => e.justify_end(),
    }
}

fn cell_el(c: &TableColumn, w: Width, cell: Cell, style: &TableStyle) -> El {
    let inner = match cell.content {
        CellContent::Text(t) => {
            let color = cell.color.unwrap_or(if cell.muted { style.muted } else { style.text });
            text(t.clone())
                .font(if cell.mono { style.mono } else { style.font })
                .color(color)
                .align(c.align)
                .truncate()
                .shrink()
                .min_w(0.0)
                .help_when_truncated(t)
        }
        CellContent::El(e) => e,
    };
    justify(sized(row().role("gridcell").h_full().px(style.cell_pad).center_y().clip_soft(0.0), c, w), c.align).child(inner)
}

#[allow(clippy::too_many_arguments)]
fn header_row<S: 'static>(
    k: &Kit,
    state: &TableState,
    columns: &Rc<Vec<TableColumn>>,
    sizes: &Rc<Vec<Width>>,
    style: &TableStyle,
    on_sort: Option<Rc<dyn Fn(&mut S, &str, SortDir, &mut Tx)>>,
    get: fn(&S) -> &TableState,
    get_mut: fn(&mut S) -> &mut TableState,
) -> El {
    let _ = get;
    let mut h = row()
        .key("header")
        .role("row")
        .w_full()
        .h(style.header_height)
        .child(el().key("rule").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(style.rule));
    let last = columns.len().saturating_sub(1);
    for (j, c) in columns.iter().enumerate() {
        let sorted = state.sort().filter(|(id, _)| *id == c.id).map(|(_, d)| d);
        let label = match sorted {
            Some(SortDir::Ascending) => format!("{}, sorted ascending", c.title),
            Some(SortDir::Descending) => format!("{}, sorted descending", c.title),
            None => c.title.clone(),
        };
        let title = text(c.title.clone())
            .key("title")
            .font(style.header_font)
            .color(if sorted.is_some() { style.text } else { style.header_text })
            .truncate()
            .shrink()
            .min_w(0.0)
            .help_when_truncated(c.title.clone());
        let mut inner = row().key("inner").gap(2.0).center_y().min_w(0.0).shrink().child(title);
        if let Some(d) = sorted {
            let glyph = if d == SortDir::Ascending { k.glyph.chevron_up } else { k.glyph.chevron_down };
            inner = inner.child(icon(glyph, 14.0).key("sort").color(style.text));
        }
        let mut cell = justify(sized(row().role("columnheader").label(label).h_full().px(style.cell_pad).center_y(), c, sizes[j]), c.align)
            .key(c.id.clone())
            .child(inner);
        let id = c.id.clone();
        cell = cell.on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
            if get_mut(s).laid_out.insert(id.clone(), r[2]) == Some(r[2]) { tx.live_only(); }
        });
        if c.sortable {
            let id = c.id.clone();
            let on_sort = on_sort.clone();
            cell = cell.cursor(Cursor::Pointer).hover(k.c.hover).on_press_tx(move |s: &mut S, tx: &mut Tx| {
                let st = get_mut(s);
                let dir = st.next_sort(&id);
                st.set_sort(id.clone(), dir);
                if let Some(f) = &on_sort {
                    f(s, &id, dir, tx);
                }
            });
        }
        if c.resizable {
            cell = cell.child(resize_handle(c.clone(), get_mut, style.rule, j == last));
        } else if j != last {
            cell = cell.child(el().key("rule").pin(None, Some(8.0), Some(0.0), Some(8.0)).w(1.0).fill(style.rule));
        }
        h = h.child(cell);
    }
    h
}

/// The drag target on a header cell's right edge: a hairline that widens the column as it's
/// dragged, clamped to its bounds. A double click puts the column back.
fn resize_handle<S: 'static>(c: TableColumn, get_mut: fn(&mut S) -> &mut TableState, rule: Color, last: bool) -> El {
    let id = c.id.clone();
    let reset = c.id.clone();
    let line = el().key("line").w(1.0).h_full().fill(if last { Color::TRANSPARENT } else { rule });
    row()
        .key("resize")
        .label(format!("Resize {}", c.title))
        .pin(None, Some(6.0), Some(-4.0), Some(6.0))
        .w(9.0)
        .justify_center()
        .cursor(Cursor::ResizeColumn)
        .on_drag(move |s: &mut S, d: &mut Drag, _tx: &mut Tx| {
            let st = get_mut(s);
            match d.phase {
                Phase::Down => {
                    st.resize_start = Some(st.column_width(&id));
                    d.data = st.column_width(&id).or_else(|| st.laid_out.get(&id).copied()).unwrap_or(c.min);
                }
                Phase::Move | Phase::Up => {
                    if d.cancelled() {
                        match st.resize_start.take().flatten() {
                            Some(w) => st.set_column_width(id.clone(), w),
                            None => st.reset_column_width(&id),
                        }
                    } else {
                        st.set_column_width(id.clone(), c.clamp(d.data + d.dx));
                        if d.phase == Phase::Up { st.resize_start = None; }
                    }
                }
            }
        })
        .on_double_press(move |s: &mut S| get_mut(s).reset_column_width(&reset))
        .child(line)
}

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
}
