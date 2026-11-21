use std::borrow::Cow;
use std::rc::Rc;

use rune::{Align, Color, Cursor, El, GrowthSpec, KeyEvent, ScrollSpec, Tx, col, el, icon, row, text};

use super::{Cell, CellContent, KeyOutcome, SortDir, TableColumn, TableState, TableStyle, Width};
use crate::Kit;

type CellFn<S> = Rc<dyn Fn(&S, usize, &str) -> Cell>;
type RowIdFn<S> = for<'a> fn(&'a S, usize) -> Cow<'a, str>;

struct RowSpec {
    cols: Rc<Vec<TableColumn>>,
    sizes: Rc<Vec<Width>>,
    style: TableStyle,
    zebra: bool,
    focus: Color,
}

trait TableBind {
    fn bind_header_layout(&self, cell: El, id: String) -> El;
    fn bind_header_sort(&self, cell: El, id: String) -> El;
    fn bind_resize(&self, handle: El, c: TableColumn) -> El;
    fn bind_virtual_rows(&self, rows: El, count: usize, spec: RowSpec) -> El;
    fn bind_viewport_layout(&self, viewport: El, row_height: f32) -> El;
    fn bind_root_key(&self, root: El) -> El;
}

struct TableHandlers<S> {
    get: fn(&S) -> &TableState,
    get_mut: fn(&mut S) -> &mut TableState,
    len: fn(&S) -> usize,
    row_id: RowIdFn<S>,
    cell: Option<CellFn<S>>,
    on_sort: Option<Rc<dyn Fn(&mut S, &str, SortDir, &mut Tx)>>,
    on_select: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
    on_open: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
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

    fn bind_resize(&self, handle: El, c: TableColumn) -> El {
        let get_mut = self.get_mut;
        let reset = c.id.clone();
        handle
            .on_drag(move |s: &mut S, d, _tx| get_mut(s).resize_drag(&c, d))
            .on_double_press(move |s: &mut S| get_mut(s).reset_column_width(&reset))
    }

    fn bind_virtual_rows(&self, rows: El, count: usize, spec: RowSpec) -> El {
        let (get, get_mut, len, row_id) = (self.get, self.get_mut, self.len, self.row_id);
        let (cell, sel, open) = (self.cell.clone(), self.on_select.clone(), self.on_open.clone());
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
            r
        };
        rows.virtual_rows(count, row_height, move |s: &S, i| row_id(s, i), build_row)
    }

    fn bind_viewport_layout(&self, viewport: El, row_height: f32) -> El {
        let get_mut = self.get_mut;
        // Inside a scrolling page this reports every frame of the scroll; only a new height
        // changes what the view reads.
        viewport.on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| get_mut(s).update_viewport(r[3], row_height, tx))
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
    style: Option<TableStyle>,
    name: Option<String>,
}

impl TableCore {
    fn build(self, state: &TableState, count: usize, k: &Kit, bind: &dyn TableBind) -> El {
        let TableCore { columns, zebra, style, name } = self;
        let style = style.unwrap_or_else(|| TableStyle::from_kit(k));
        let columns = Rc::new(columns);
        let sizes: Rc<Vec<Width>> = Rc::new(columns.iter().map(|c| state.column_width(&c.id).map_or(c.width, |w| Width::Fixed(c.clamp(w)))).collect());
        let header = header_row(k, state, &columns, &sizes, &style, bind);
        let rows = bind.bind_virtual_rows(
            col().key("rows").w_full().growth(GrowthSpec::default()),
            count,
            RowSpec { cols: columns, sizes, style, zebra, focus: k.c.border_focus },
        );
        let viewport = bind
            .bind_viewport_layout(
                col().key("viewport").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }),
                style.row_height,
            )
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
        // Focus stays on the table; the row under the cursor is what a screen reader follows.
        if let Some(id) = &state.cursor {
            root = root.active_descendant(id.clone());
        }
        bind.bind_root_key(root).child(header).child(viewport)
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
            core: TableCore { columns: Vec::new(), zebra: false, style: None, name: None },
            handlers: TableHandlers { get, get_mut, len, row_id, cell: None, on_sort: None, on_select: None, on_open: None },
        }
    }
    pub fn column(mut self, c: TableColumn) -> Self { self.core.columns.push(c); self }
    /// What row `i` shows in column `col` (the column's id).
    pub fn cell(mut self, f: impl Fn(&S, usize, &str) -> Cell + 'static) -> Self { self.handlers.cell = Some(Rc::new(f)); self }
    /// A sortable column's header was clicked: the table already shows the new sort; reorder
    /// the rows to match.
    pub fn on_sort(mut self, f: impl Fn(&mut S, &str, SortDir, &mut Tx) + 'static) -> Self { self.handlers.on_sort = Some(Rc::new(f)); self }
    /// The cursor moved to row `id`, by click or key.
    pub fn on_select(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self { self.handlers.on_select = Some(Rc::new(f)); self }
    /// A row was double-clicked, or Return pressed on it.
    pub fn on_open(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self { self.handlers.on_open = Some(Rc::new(f)); self }
    /// Fill every other row.
    pub fn zebra(mut self, on: bool) -> Self { self.core.zebra = on; self }
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
    if st.multi && st.cursor.as_deref() == Some(id) {
        r = r.outline(1.0, -1.0, spec.focus);
    }
    for (j, c) in spec.cols.iter().enumerate() {
        r = r.child(cell_el(c, spec.sizes[j], cell(&c.id), style).key(c.id.clone()));
    }
    r
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
