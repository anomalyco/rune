use std::borrow::Cow;
use std::rc::Rc;

use rune::{Align, Color, Cursor, El, GrowthSpec, KeyEvent, PointerPress, ScrollSpec, Tx, col, el, icon, row, text};

use super::{Cell, CellContent, KeyOutcome, SortDir, TableColumn, TableState, TableStyle, Width};
use crate::Kit;

type CellFn<S> = Rc<dyn Fn(&S, usize, &str) -> Cell>;
type RowFn<S> = Rc<dyn Fn(&S, usize, El) -> El>;
type RowIdFn<S> = for<'a> fn(&'a S, usize) -> Cow<'a, str>;

struct RowSpec {
    cols: Rc<Vec<TableColumn>>,
    sizes: Rc<Vec<Width>>,
    style: TableStyle,
    zebra: bool,
    cursor_outline: bool,
    focus: Color,
}

trait TableBind {
    fn bind_header_layout(&self, cell: El, id: String) -> El;
    fn bind_header_sort(&self, cell: El, id: String) -> El;
    fn bind_header_secondary_press(&self, cell: El, id: String) -> El;
    fn bind_resize(&self, handle: El, c: TableColumn) -> El;
    fn bind_virtual_rows(&self, rows: El, count: usize, spec: RowSpec) -> El;
    fn bind_viewport_layout(&self, viewport: El, header_inset: f32, row_height: f32) -> El;
    fn bind_root_key(&self, root: El) -> El;
}

struct TableHandlers<S> {
    get: fn(&S) -> &TableState,
    get_mut: fn(&mut S) -> &mut TableState,
    len: fn(&S) -> usize,
    row_id: RowIdFn<S>,
    cell: Option<CellFn<S>>,
    row: Option<RowFn<S>>,
    on_sort: Option<Rc<dyn Fn(&mut S, &str, SortDir, &mut Tx)>>,
    on_select: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
    on_open: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
    on_header_secondary_press: Option<Rc<dyn Fn(&mut S, &str, PointerPress, &mut Tx)>>,
    on_column_fit: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
}

impl<S: 'static> TableBind for TableHandlers<S> {
    fn bind_header_layout(&self, cell: El, id: String) -> El {
        let get_mut = self.get_mut;
        cell.on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| get_mut(s).report_column_width(&id, r[2], tx))
    }

    fn bind_header_sort(&self, cell: El, id: String) -> El {
        let (get_mut, on_sort) = (self.get_mut, self.on_sort.clone());
        cell.on_press_tx(move |s: &mut S, tx: &mut Tx| {
            let dir = get_mut(s).sort_click(&id);
            if let Some(f) = &on_sort {
                f(s, &id, dir, tx);
            }
        })
    }

    fn bind_header_secondary_press(&self, cell: El, id: String) -> El {
        let Some(f) = self.on_header_secondary_press.clone() else {
            return cell;
        };
        cell.on_secondary_press(move |s: &mut S, press, tx| f(s, &id, press, tx))
    }

    fn bind_resize(&self, handle: El, c: TableColumn) -> El {
        let get_mut = self.get_mut;
        let id = c.id.clone();
        let on_fit = self.on_column_fit.clone();
        handle
            .on_drag(move |s: &mut S, d, _tx| get_mut(s).resize_drag(&c, d))
            .on_double_press_tx(move |s: &mut S, tx: &mut Tx| {
                if let Some(f) = &on_fit {
                    f(s, &id, tx);
                } else {
                    get_mut(s).reset_column_width(&id);
                }
            })
    }

    fn bind_virtual_rows(&self, rows: El, count: usize, spec: RowSpec) -> El {
        let (get, get_mut, len, row_id) = (self.get, self.get_mut, self.len, self.row_id);
        let (cell, row_hook, sel, open) = (self.cell.clone(), self.row.clone(), self.on_select.clone(), self.on_open.clone());
        let row_height = spec.style.row_height;
        let build_row = move |s: &S, i: usize| -> El {
            let st = get(s);
            let id = row_id(s, i);
            let mut r = build_row_el(st, &id, i, &spec, &|col| {
                cell.as_ref().map_or_else(|| Cell::text(""), |f| f(s, i, col))
            });
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
            if let Some(f) = &row_hook {
                r = f(s, i, r);
            }
            r
        };
        rows.virtual_rows(count, row_height, move |s: &S, i| row_id(s, i), build_row)
    }

    fn bind_viewport_layout(&self, viewport: El, header_inset: f32, row_height: f32) -> El {
        let get_mut = self.get_mut;
        // Inside a scrolling page this reports every frame of the scroll; only a new height
        // changes what the view reads.
        viewport.on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
            get_mut(s).update_viewport((r[3] - header_inset).max(0.0), row_height, tx)
        })
    }

    fn bind_root_key(&self, root: El) -> El {
        let (get_mut, len, row_id) = (self.get_mut, self.len, self.row_id);
        let (on_select, on_open) = (self.on_select.clone(), self.on_open.clone());
        root.on_key(move |s: &mut S, ev: &KeyEvent, tx: &mut Tx| {
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
    }
}

struct TableCore {
    columns: Vec<TableColumn>,
    zebra: bool,
    stick_bottom: bool,
    cursor_outline: bool,
    scroll_wide: bool,
    virtual_overscan: Option<(f32, f32)>,
    style: Option<TableStyle>,
    name: Option<String>,
}

/// Minimum total content width across visible columns, respecting resized widths.
fn min_content_width(columns: &[TableColumn], sizes: &[Width]) -> f32 {
    columns
        .iter()
        .zip(sizes)
        .map(|(c, &w)| match w {
            Width::Fixed(fw) => c.clamp(fw),
            Width::Flex(_) => c.min,
        })
        .sum()
}

impl TableCore {
    fn build(self, state: &TableState, count: usize, k: &Kit, bind: &dyn TableBind) -> El {
        let TableCore { columns, zebra, stick_bottom, cursor_outline, scroll_wide, virtual_overscan, style, name } = self;
        let style = style.unwrap_or_else(|| TableStyle::from_kit(k));
        let columns: Rc<Vec<TableColumn>> = Rc::new(
            columns.into_iter().filter(|c| !state.is_column_hidden(&c.id)).collect(),
        );
        let sizes: Rc<Vec<Width>> = Rc::new(
            columns.iter().map(|c| c.resolved_width(state.column_width(&c.id))).collect(),
        );
        let min_total = if scroll_wide { min_content_width(&columns, &sizes) } else { 0.0 };
        let header = header_row(k, state, &columns, &sizes, &style, bind);
        let mut rows = bind.bind_virtual_rows(
            col().key("rows").w_full().growth(GrowthSpec::default()),
            count,
            RowSpec { cols: columns, sizes, style, zebra, cursor_outline, focus: k.c.border_focus },
        );
        if let Some((overscan, edge)) = virtual_overscan {
            rows = rows.virtual_overscan(overscan, edge);
        }
        let mut root = col()
            .key(state.key.clone())
            .role("grid")
            .focusable(true)
            .w_full()
            .clip_soft(0.0);
        if let Some(n) = name {
            root = root.label(n);
        }
        // Focus stays on the table; the row under the cursor is what a screen reader follows.
        if let Some(id) = &state.cursor {
            root = root.active_descendant(id.clone());
        }
        if scroll_wide {
            let view_w = rune::memo::layout_with(&format!("{}/viewport", state.key), |r| r[2]).unwrap_or(0.0);
            let header = header.fill(k.c.bg_base).sticky_top(0.0);
            let content_w = min_total.max(view_w);
            let content = if content_w > 0.0 {
                col().key("content").w(content_w)
            } else {
                col().key("content").w_full()
            }
            .child(header)
            .child(rows);
            let viewport = bind
                .bind_viewport_layout(
                    col()
                        .key("viewport")
                        .w_full()
                        .grow(1.0)
                        .min_h(0.0)
                        .scroll(ScrollSpec { stick_bottom })
                        .scroll_xy(),
                    style.header_height,
                    style.row_height,
                )
                .child(content);
            bind.bind_root_key(root).child(viewport)
        } else {
            let viewport = bind
                .bind_viewport_layout(
                    col().key("viewport").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom }),
                    0.0,
                    style.row_height,
                )
                .child(rows);
            bind.bind_root_key(root).child(header).child(viewport)
        }
    }
}

/// A virtualised table over the app's rows in its state `S`.
pub struct Table<S> {
    core: TableCore,
    handlers: TableHandlers<S>,
}

impl<S: 'static> Table<S> {
    /// `get` and `get_mut` find the [`TableState`]; `len` counts the rows and `row_id` names
    /// row `i` (in the app's current order) with an id unique among them.
    pub fn new(get: fn(&S) -> &TableState, get_mut: fn(&mut S) -> &mut TableState, len: fn(&S) -> usize, row_id: RowIdFn<S>) -> Self {
        Table {
            core: TableCore { columns: Vec::new(), zebra: false, stick_bottom: false, cursor_outline: true, scroll_wide: false, virtual_overscan: None, style: None, name: None },
            handlers: TableHandlers {
                get,
                get_mut,
                len,
                row_id,
                cell: None,
                row: None,
                on_sort: None,
                on_select: None,
                on_open: None,
                on_header_secondary_press: None,
                on_column_fit: None,
            },
        }
    }
    pub fn column(mut self, c: TableColumn) -> Self { self.core.columns.push(c); self }
    /// What row `i` shows in column `col` (the column's id).
    pub fn cell(mut self, f: impl Fn(&S, usize, &str) -> Cell + 'static) -> Self { self.handlers.cell = Some(Rc::new(f)); self }
    /// Customise each mounted row element after the table has added its striping, selection,
    /// hover, cursor outline, and click/double-click handlers.
    ///
    /// Runs only for rows currently mounted by `virtual_rows`, so large tables keep their
    /// virtualisation cost. Use it to attach per-row handlers such as
    /// [`El::on_secondary_press`] (context menus), [`El::on_drag`], or row decorations without
    /// wrapping every cell in [`Cell::el`] (which would drop [`Cell::text`]'s truncation tooltip).
    /// Attaching `on_press` / `on_double_press` replaces the table's own handler on that row.
    ///
    /// When opening a context menu from `on_secondary_press`, select the clicked row only if it is
    /// not already selected so multi-selections stay intact on already-selected rows:
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune::PointerPress;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState, menu: Option<(String, PointerPress)> }
    /// let app = App { rows: vec!["alpha".into()], table: TableState::new(), menu: None };
    /// let k = Kit::neutral(true);
    /// let _el = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///     .column(TableColumn::new("name", "Name"))
    ///     .cell(|s: &App, i, _| Cell::text(&s.rows[i]))
    ///     .row(|s: &App, i, row| {
    ///         let id = s.rows[i].clone();
    ///         row.on_secondary_press(move |s: &mut App, p, _| {
    ///             if !s.table.is_selected(&id) { s.table.select(id.clone()); }
    ///             s.menu = Some((id.clone(), p));
    ///         })
    ///     })
    ///     .build(&app, &k);
    /// ```
    pub fn row(mut self, f: impl Fn(&S, usize, El) -> El + 'static) -> Self { self.handlers.row = Some(Rc::new(f)); self }
    /// A sortable column's header was clicked: the table already shows the new sort; reorder
    /// the rows to match.
    pub fn on_sort(mut self, f: impl Fn(&mut S, &str, SortDir, &mut Tx) + 'static) -> Self { self.handlers.on_sort = Some(Rc::new(f)); self }
    /// The cursor moved to row `id`, by click or key.
    pub fn on_select(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self { self.handlers.on_select = Some(Rc::new(f)); self }
    /// A row was double-clicked, or Return pressed on it.
    pub fn on_open(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self { self.handlers.on_open = Some(Rc::new(f)); self }
    /// A column header received a secondary press (right-click, macOS control-click, or the
    /// Menu / Shift+F10 key): delivers the column id and window point for a context menu
    /// (for example to toggle column visibility via [`TableState::hide_column`] or fit widths).
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune::PointerPress;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState, header_menu: Option<(String, PointerPress)> }
    /// let app = App { rows: vec!["alpha".into()], table: TableState::new(), header_menu: None };
    /// let k = Kit::neutral(true);
    /// let _el = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///     .column(TableColumn::new("name", "Name"))
    ///     .cell(|s: &App, i, _| Cell::text(&s.rows[i]))
    ///     .on_header_secondary_press(|s: &mut App, col, press, _| {
    ///         s.header_menu = Some((col.to_owned(), press));
    ///     })
    ///     .build(&app, &k);
    /// ```
    pub fn on_header_secondary_press(mut self, f: impl Fn(&mut S, &str, PointerPress, &mut Tx) + 'static) -> Self {
        self.handlers.on_header_secondary_press = Some(Rc::new(f));
        self
    }
    /// A resizable column header's right edge was double-clicked: delivers the column id so the
    /// app can fit the column to its content via [`TableState::set_column_width`]. When unset,
    /// double-clicking resets the column to its declared width ([`TableState::reset_column_width`]).
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState }
    /// let app = App { rows: vec!["alpha".into()], table: TableState::new() };
    /// let k = Kit::neutral(true);
    /// let _el = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///     .column(TableColumn::new("name", "Name"))
    ///     .cell(|s: &App, i, _| Cell::text(&s.rows[i]))
    ///     .on_column_fit(|s: &mut App, col, _| s.table.set_column_width(col, 180.0))
    ///     .build(&app, &k);
    /// ```
    pub fn on_column_fit(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        self.handlers.on_column_fit = Some(Rc::new(f));
        self
    }
    /// Fill every other row.
    pub fn zebra(mut self, on: bool) -> Self { self.core.zebra = on; self }
    /// Keep the bottom of the table in view as rows are appended while scrolled to the end
    /// (`ScrollSpec { stick_bottom: on }`, off by default). Scrolling up detaches and preserves
    /// the scroll position until the user returns to the bottom.
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { logs: Vec<String>, table: TableState }
    /// let app = App { logs: vec!["ready".into()], table: TableState::new() };
    /// let k = Kit::neutral(true);
    /// let _el = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.logs.len(), |s: &App, i| Cow::Borrowed(s.logs[i].as_str()))
    ///     .column(TableColumn::new("msg", "Message"))
    ///     .cell(|s: &App, i, _| Cell::text(&s.logs[i]))
    ///     .stick_bottom(true)
    ///     .build(&app, &k);
    /// ```
    pub fn stick_bottom(mut self, on: bool) -> Self { self.core.stick_bottom = on; self }
    /// Draw the 1 pt inset focus outline around the cursor row in multi-select mode (on by
    /// default). Pass `false` when rows draw their own selection or cursor mark; keyboard
    /// navigation and accessibility (`active_descendant`) are unchanged.
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState }
    /// let app = App { rows: vec!["a".into()], table: TableState::new().multi_select(true) };
    /// let k = Kit::neutral(true);
    /// let _el = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///     .column(TableColumn::new("name", "Name"))
    ///     .cell(|s: &App, i, _| Cell::text(&s.rows[i]))
    ///     .cursor_outline(false)
    ///     .build(&app, &k);
    /// ```
    pub fn cursor_outline(mut self, on: bool) -> Self { self.core.cursor_outline = on; self }
    /// Scroll the header and rows sideways together when the visible columns' minimum or resized
    /// widths exceed the viewport width (off by default, matching `Markdown::scroll_wide`).
    ///
    /// When the columns fit inside the viewport, flex columns still expand to fill the available
    /// width. While scrolling vertically, the header stays sticky at the top with an opaque
    /// `k.c.bg_base` fill.
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState }
    /// let app = App { rows: vec!["r0".into()], table: TableState::new() };
    /// let k = Kit::neutral(true);
    /// let _el = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///     .column(TableColumn::new("c0", "Column 0").flex(1.0).min(120.0))
    ///     .column(TableColumn::new("c1", "Column 1").flex(1.0).min(120.0))
    ///     .cell(|s: &App, i, col| Cell::text(format!("{}:{col}", s.rows[i])))
    ///     .scroll_wide(true)
    ///     .build(&app, &k);
    /// ```
    pub fn scroll_wide(mut self, on: bool) -> Self { self.core.scroll_wide = on; self }
    /// Forwards [`El::virtual_overscan`] to the table's internal `virtual_rows` list, setting
    /// how many viewports of rows stay mounted on each side (`overscan`, default `2.0`) and how
    /// close the viewport may come to the mounted edge before a commit moves the window (`edge`,
    /// default `1.75`).
    ///
    /// Smaller values mount fewer offscreen rows and reduce per-frame scroll work at the cost of
    /// more frequent window-move commits while scrolling; larger values commit less often while
    /// keeping more offscreen nodes mounted. Clamping and non-finite handling follow
    /// [`El::virtual_overscan`].
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState }
    /// let app = App { rows: vec!["r0".into()], table: TableState::new() };
    /// let k = Kit::neutral(true);
    /// let _el = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///     .column(TableColumn::new("name", "Name"))
    ///     .cell(|s: &App, i, _| Cell::text(&s.rows[i]))
    ///     .virtual_overscan(0.5, 0.25)
    ///     .build(&app, &k);
    /// ```
    pub fn virtual_overscan(mut self, overscan: f32, edge: f32) -> Self {
        if overscan.is_finite() && edge.is_finite() {
            self.core.virtual_overscan = Some((overscan, edge));
        }
        self
    }
    pub fn style(mut self, s: TableStyle) -> Self { self.core.style = Some(s); self }
    /// The table's accessibility label.
    pub fn name(mut self, n: impl Into<String>) -> Self { self.core.name = Some(n.into()); self }

    /// The table, filling the height its parent gives it.
    pub fn build(self, s: &S, k: &Kit) -> El {
        let state = (self.handlers.get)(s);
        let count = (self.handlers.len)(s);
        self.core.build(state, count, k, &self.handlers)
    }
}

fn build_row_el(
    st: &TableState,
    id: &str,
    i: usize,
    spec: &RowSpec,
    cell: &dyn Fn(&str) -> Cell,
) -> El {
    let style = &spec.style;
    let selected = st.is_selected(id);
    let mut r = row()
        .key(id.to_owned())
        .role("row")
        .focus_selected(selected)
        .w_full()
        .h(style.row_height)
        .center_y();
    r = if selected {
        r.fill(style.selected)
    } else if spec.zebra && i % 2 == 1 {
        r.fill(style.stripe).hover(style.stripe.mix(style.selected, 0.5))
    } else {
        r.hover(style.stripe.mix(style.selected, 0.5))
    };
    if spec.cursor_outline && st.multi && st.cursor.as_deref() == Some(id) {
        r = r.outline(1.0, -1.0, spec.focus);
    }
    for (j, c) in spec.cols.iter().enumerate() {
        r = r.child(cell_el(c, spec.sizes[j], cell(&c.id), style).key(c.id.clone()));
    }
    r
}

/// Sizes a header or body cell to its column.
fn sized(e: El, c: &TableColumn, w: Width) -> El {
    c.apply_width(e, w)
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

fn header_row(k: &Kit, state: &TableState, columns: &[TableColumn], sizes: &[Width], style: &TableStyle, bind: &dyn TableBind) -> El {
    let mut h = row()
        .key("header")
        .role("row")
        .w_full()
        .h(style.header_height)
        .child(el().key("rule").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(1.0).fill(style.rule).a11y_hidden());
    let last = columns.len().saturating_sub(1);
    for (j, c) in columns.iter().enumerate() {
        let sorted = state.sort().filter(|(id, _)| *id == c.id).map(|(_, d)| d);
        let title = text(c.title.clone())
            .key("title")
            .font(style.header_font)
            .color(if sorted.is_some() { style.text } else { style.header_text })
            .truncate()
            .shrink()
            .min_w(0.0)
            .help_when_truncated(c.title.clone());
        let mut inner = row().key("inner").gap(2.0).center_y().min_w(0.0).shrink().child(title).a11y_hidden();
        if let Some(d) = sorted {
            let glyph = if d == SortDir::Ascending { k.glyph.chevron_up } else { k.glyph.chevron_down };
            inner = inner.child(icon(glyph, 14.0).key("sort").color(style.text));
        }
        let cell = justify(sized(row().role("columnheader").label(c.title.clone()).sorted(sorted.map(|d| d == SortDir::Ascending)).h_full().px(style.cell_pad).center_y(), c, sizes[j]), c.align)
            .key(c.id.clone())
            .child(inner);
        let mut cell = bind.bind_header_layout(cell, c.id.clone());
        if c.sortable {
            cell = bind.bind_header_sort(cell.cursor(Cursor::Pointer).hover(k.c.hover), c.id.clone());
        }
        cell = bind.bind_header_secondary_press(cell, c.id.clone());
        if c.resizable {
            cell = cell.child(resize_handle(c.clone(), style.rule, j == last, bind));
        } else if j != last {
            cell = cell.child(el().key("rule").pin(None, Some(8.0), Some(0.0), Some(8.0)).w(1.0).fill(style.rule).a11y_hidden());
        }
        h = h.child(cell);
    }
    h
}

/// The drag target on a header cell's right edge: a hairline that widens the column as it's
/// dragged, clamped to its bounds. A double click puts the column back.
fn resize_handle(c: TableColumn, rule: Color, last: bool, bind: &dyn TableBind) -> El {
    let line = el().key("line").w(1.0).h_full().fill(if last { Color::TRANSPARENT } else { rule }).a11y_hidden();
    let handle = row()
        .key("resize")
        .role("separator")
        .label(format!("Resize {}", c.title))
        .pin(None, Some(6.0), Some(-4.0), Some(6.0))
        .w(9.0)
        .justify_center()
        .cursor(Cursor::ResizeColumn);
    bind.bind_resize(handle, c).child(line)
}
