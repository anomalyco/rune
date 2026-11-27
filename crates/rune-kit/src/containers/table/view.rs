use std::borrow::Cow;
use std::rc::Rc;

use rune::{Anim, Color, Drag, El, GrowthSpec, KeyEvent, PointerEvent, PointerPress, ScrollSpec, Tx, col};

use super::render::{build_row_el, header_row};
use super::window::{self, BandParams, ColumnPlan};
use super::{Cell, KeyOutcome, SortDir, TableColumn, TableState, TableStyle, Width};
use crate::Kit;

type CellFn<S> = Rc<dyn Fn(&S, usize, &str) -> Cell>;
type RowFn<S> = Rc<dyn Fn(&S, usize, El) -> El>;
type GetFn<S> = Rc<dyn Fn(&S) -> &TableState>;
type GetMutFn<S> = Rc<dyn Fn(&mut S) -> &mut TableState>;
type LenFn<S> = Rc<dyn Fn(&S) -> usize>;
type RowIdFn<S> = Rc<dyn for<'a> Fn(&'a S, usize) -> Cow<'a, str>>;
type ReorderFn<S> = Rc<dyn Fn(&mut S, usize, usize, &mut Tx)>;

pub(super) struct RowSpec {
    pub(super) cols: Rc<Vec<TableColumn>>,
    pub(super) sizes: Rc<Vec<Width>>,
    pub(super) plan: Rc<ColumnPlan>,
    pub(super) style: TableStyle,
    pub(super) bg: Color,
    pub(super) zebra: bool,
    pub(super) cursor_outline: bool,
    pub(super) focus: Color,
    pub(super) hover_motion: Option<(Anim, Anim)>,
}

pub(super) trait TableBind {
    fn bind_header_layout(&self, cell: El, id: String, live_only: bool) -> El;
    fn bind_header_sort(&self, cell: El, id: String) -> El;
    fn bind_header_secondary_press(&self, cell: El, id: String) -> El;
    fn bind_header_reorder(&self, cell: El, index: usize, columns: Rc<Vec<TableColumn>>, held: bool) -> El;
    fn bind_resize(&self, handle: El, c: TableColumn) -> El;
    fn bind_virtual_rows(&self, rows: El, count: usize, spec: RowSpec) -> El;
    fn bind_viewport_layout(&self, viewport: El, header_inset: f32, row_height: f32) -> El;
    fn bind_content_layout(&self, content: El, band: BandParams) -> El;
    fn bind_root_key(&self, root: El) -> El;
}

struct TableHandlers<S> {
    get: GetFn<S>,
    get_mut: GetMutFn<S>,
    len: LenFn<S>,
    row_id: RowIdFn<S>,
    cell: Option<CellFn<S>>,
    row: Option<RowFn<S>>,
    on_sort: Option<Rc<dyn Fn(&mut S, &str, SortDir, &mut Tx)>>,
    on_select: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
    on_open: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
    on_header_secondary_press: Option<Rc<dyn Fn(&mut S, &str, PointerPress, &mut Tx)>>,
    on_column_fit: Option<Rc<dyn Fn(&mut S, &str, &mut Tx)>>,
    on_reorder: Option<ReorderFn<S>>,
}

impl<S: 'static> TableBind for TableHandlers<S> {
    fn bind_header_layout(&self, cell: El, id: String, live_only: bool) -> El {
        let get_mut = self.get_mut.clone();
        cell.on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| get_mut(s).report_column_width(&id, r[2], live_only, tx))
    }

    fn bind_header_sort(&self, cell: El, id: String) -> El {
        let (get_mut, on_sort) = (self.get_mut.clone(), self.on_sort.clone());
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

    fn bind_header_reorder(&self, mut cell: El, index: usize, columns: Rc<Vec<TableColumn>>, held: bool) -> El {
        let Some(f) = self.on_reorder.clone() else {
            return cell;
        };
        // While a header is held, every header hears where the pointer goes: a move its drag
        // never hears means the release was lost (the header unmounted, or a modal took it).
        if held {
            let get_mut = self.get_mut.clone();
            cell = cell.on_pointer_move_local(move |s: &mut S, p: PointerEvent, tx: &mut Tx| {
                if !get_mut(s).column_pointer([p.window_x, p.window_y]) {
                    tx.live_only();
                }
            });
        }
        let get_mut = self.get_mut.clone();
        cell.on_drag(move |s: &mut S, d: &mut Drag, tx: &mut Tx| {
            if let Some((from, to)) = get_mut(s).reorder_drag(&columns, index, d) {
                f(s, from, to, tx);
            }
        })
    }

    fn bind_resize(&self, handle: El, c: TableColumn) -> El {
        let get_mut = self.get_mut.clone();
        let id = c.id.clone();
        let on_fit = self.on_column_fit.clone();
        handle
            .on_drag({
                let get_mut = get_mut.clone();
                move |s: &mut S, d, _tx| get_mut(s).resize_drag(&c, d)
            })
            .on_double_press_tx(move |s: &mut S, tx: &mut Tx| {
                if let Some(f) = &on_fit {
                    f(s, &id, tx);
                } else {
                    get_mut(s).reset_column_width(&id);
                }
            })
    }

    fn bind_virtual_rows(&self, rows: El, count: usize, spec: RowSpec) -> El {
        let (get, get_mut, len, row_id) = (self.get.clone(), self.get_mut.clone(), self.len.clone(), self.row_id.clone());
        let (cell, row_hook, sel, open) = (self.cell.clone(), self.row.clone(), self.on_select.clone(), self.on_open.clone());
        let row_height = spec.style.row_height;
        let build_row = move |s: &S, i: usize| -> El {
            let st = get(s);
            let id = row_id(s, i);
            let focus_gm = get_mut.clone();
            let mut r = build_row_el(
                st,
                &id,
                i,
                &spec,
                &|col| cell.as_ref().map_or_else(|| Cell::text(""), |f| f(s, i, col)),
                &|el, col_id, in_base| {
                    let gm = focus_gm.clone();
                    let cid = col_id.to_owned();
                    el.on_focus_within_change(move |s: &mut S, within, tx: &mut Tx| {
                        gm(s).set_focused_column(&cid, within, in_base, tx);
                    })
                },
            );
            let sel = sel.clone();
            let (get_mut, len_at, id_at) = (get_mut.clone(), len.clone(), row_id.clone());
            r = r.on_press_tx(move |s: &mut S, tx: &mut Tx| {
                let (len, row_id) = (&len_at, &id_at);
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
                let (len, row_id) = (len.clone(), row_id.clone());
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
        let key = self.row_id.clone();
        rows.virtual_rows(count, row_height, move |s: &S, i| key(s, i), build_row)
    }

    fn bind_viewport_layout(&self, viewport: El, header_inset: f32, row_height: f32) -> El {
        let get_mut = self.get_mut.clone();
        // Inside a scrolling page this reports every frame of the scroll; only a new height
        // changes what the view reads.
        viewport.on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
            get_mut(s).update_viewport(r[0], (r[3] - header_inset).max(0.0), row_height, tx)
        })
    }

    fn bind_content_layout(&self, content: El, band: BandParams) -> El {
        let get_mut = self.get_mut.clone();
        content.on_layout_tx(move |s: &mut S, r: [f32; 4], tx: &mut Tx| {
            get_mut(s).update_scroll_x(r[0], band, tx);
        })
    }

    fn bind_root_key(&self, root: El) -> El {
        let (get_mut, len, row_id) = (self.get_mut.clone(), self.len.clone(), self.row_id.clone());
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
    virtual_columns: bool,
    frozen_columns: usize,
    virtual_overscan: Option<(f32, f32)>,
    style: Option<TableStyle>,
    name: Option<String>,
}

/// The columns a table shows, in order, with their widths, and every declared column (which a
/// header drag needs to work out the new order).
pub(super) struct Layout {
    pub(super) declared: Rc<Vec<TableColumn>>,
    pub(super) columns: Rc<Vec<TableColumn>>,
    pub(super) sizes: Rc<Vec<Width>>,
}

impl Layout {
    fn new(declared: Vec<TableColumn>, state: &TableState) -> Layout {
        let columns = state.visible_columns(&declared);
        let sizes = columns.iter().map(|c| c.resolved_width(state.column_width(&c.id))).collect();
        Layout { declared: Rc::new(declared), columns: Rc::new(columns), sizes: Rc::new(sizes) }
    }
}

impl TableCore {
    fn header(self, state: &TableState, k: &Kit, bind: &dyn TableBind) -> El {
        let style = self.style.unwrap_or_else(|| TableStyle::from_kit(k));
        let layout = Layout::new(self.columns, state);
        let plan = ColumnPlan::all(&layout.columns, self.virtual_columns || self.frozen_columns > 0);
        header_row(k, state, &layout, &plan, &style, bind)
    }

    fn build(self, state: &TableState, count: usize, k: &Kit, bind: &dyn TableBind) -> El {
        let TableCore {
            columns,
            zebra,
            stick_bottom,
            cursor_outline,
            scroll_wide,
            virtual_columns,
            frozen_columns,
            virtual_overscan,
            style,
            name,
        } = self;
        let style = style.unwrap_or_else(|| TableStyle::from_kit(k));
        let layout = Layout::new(columns, state);
        let view_w = if scroll_wide {
            rune::memo::layout_with(&format!("{}/viewport", state.key), |r| r[2]).unwrap_or(0.0)
        } else {
            0.0
        };
        let pinned = if scroll_wide && virtual_columns {
            window::pinned_indices(state, &layout.columns)
        } else {
            Vec::new()
        };
        let plan = Rc::new(ColumnPlan::compute(
            &layout.columns,
            &layout.sizes,
            scroll_wide,
            virtual_columns,
            frozen_columns,
            view_w,
            state.columns.anchor_x(),
            &pinned,
        ));
        let col_count = layout.columns.len();
        let header = header_row(k, state, &layout, &plan, &style, bind);
        let Layout { columns, sizes, .. } = layout;
        let mut rows = bind.bind_virtual_rows(
            col().key("rows").w_full().growth(GrowthSpec::default()),
            count,
            RowSpec {
                cols: columns,
                sizes,
                plan: Rc::clone(&plan),
                style,
                bg: k.c.bg_base,
                zebra,
                cursor_outline,
                focus: k.c.border_focus,
                hover_motion: k.motion.hover,
            },
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
        if plan.grid_coords {
            root = root.row_count(count).column_count(col_count);
        }
        if let Some(n) = name {
            root = root.label(n);
        }
        // Focus stays on the table; the row under the cursor is what a screen reader follows.
        if let Some(id) = &state.cursor {
            root = root.active_descendant(id.clone());
        }
        if scroll_wide {
            let header = header.fill(k.c.bg_base).sticky_top(0.0);
            let content_w = plan.total_width.max(view_w);
            let mut content = if content_w > 0.0 {
                col().key("content").w(content_w)
            } else {
                col().key("content").w_full()
            }
            .child(header)
            .child(rows);
            if let Some(band) = plan.band {
                content = bind.bind_content_layout(content, band);
            }
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
    ///
    /// Each is any closure, so one app can keep several tables over its own state.
    pub fn new(
        get: impl Fn(&S) -> &TableState + 'static,
        get_mut: impl Fn(&mut S) -> &mut TableState + 'static,
        len: impl Fn(&S) -> usize + 'static,
        row_id: impl for<'a> Fn(&'a S, usize) -> Cow<'a, str> + 'static,
    ) -> Self {
        Table {
            core: TableCore { columns: Vec::new(), zebra: false, stick_bottom: false, cursor_outline: true, scroll_wide: false, virtual_columns: false, frozen_columns: 0, virtual_overscan: None, style: None, name: None },
            handlers: TableHandlers {
                get: Rc::new(get),
                get_mut: Rc::new(get_mut),
                len: Rc::new(len),
                row_id: Rc::new(row_id),
                cell: None,
                row: None,
                on_sort: None,
                on_select: None,
                on_open: None,
                on_header_secondary_press: None,
                on_column_fit: None,
                on_reorder: None,
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
    /// Window non-frozen columns horizontally inside [`Self::scroll_wide`]`(true)` so wide tables
    /// build only frozen columns plus the columns near the horizontal viewport (default `false`).
    ///
    /// Leading and trailing fixed-width spacers keep total scrollable width and every column's
    /// x-position exact. Columns actively being resized, reordered, or holding keyboard focus
    /// (including caret browsing) stay mounted while scrolled out of view. Reports ARIA grid
    /// counts and 1-based row/column indices on the table, rows, headers, and cells.
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState }
    /// let app = App { rows: vec!["r0".into()], table: TableState::new() };
    /// let k = Kit::neutral(true);
    /// let mut t = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///     .scroll_wide(true)
    ///     .virtual_columns(true);
    /// for c in 0..20 {
    ///     t = t.column(TableColumn::new(format!("c{c}"), format!("Col {c}")).width(100.0));
    /// }
    /// let _el = t.cell(|s: &App, i, col| Cell::text(format!("{}:{col}", s.rows[i]))).build(&app, &k);
    /// ```
    pub fn virtual_columns(mut self, on: bool) -> Self { self.core.virtual_columns = on; self }
    /// Keep the first `count` visible columns pinned to the left edge with [`El::sticky_left`]
    /// while scrolling sideways inside [`Self::scroll_wide`]`(true)` (default `0`).
    ///
    /// Frozen header cells compose with the sticky top header so the top-left corner stays fixed
    /// in both directions, and frozen cells paint an opaque fill over scrolled columns. Clamped
    /// to the number of visible columns; has no effect when `scroll_wide` is `false`.
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState }
    /// let app = App { rows: vec!["r0".into()], table: TableState::new() };
    /// let k = Kit::neutral(true);
    /// let _el = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///     .scroll_wide(true)
    ///     .frozen_columns(1)
    ///     .column(TableColumn::new("id", "ID").width(80.0))
    ///     .column(TableColumn::new("val", "Value").width(200.0))
    ///     .cell(|s: &App, i, col| Cell::text(format!("{}:{col}", s.rows[i])))
    ///     .build(&app, &k);
    /// ```
    pub fn frozen_columns(mut self, count: usize) -> Self { self.core.frozen_columns = count; self }
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

    /// Dragging a header sideways moves its column: the table has already moved it in
    /// [`TableState::column_order`] when `f` hears the column's place before the move and after
    /// it, both among the visible columns. A press that doesn't move still sorts.
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune_kit::{Cell, Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState, saved_order: Vec<String> }
    /// let app = App { rows: vec!["alpha".into()], table: TableState::new(), saved_order: Vec::new() };
    /// let k = Kit::neutral(true);
    /// let _el = Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///     .column(TableColumn::new("name", "Name"))
    ///     .column(TableColumn::new("kind", "Kind"))
    ///     .cell(|s: &App, i, _| Cell::text(&s.rows[i]))
    ///     .on_reorder(|s: &mut App, _from, _to, _| s.saved_order = s.table.column_order().to_vec())
    ///     .build(&app, &k);
    /// ```
    pub fn on_reorder(mut self, f: impl Fn(&mut S, usize, usize, &mut Tx) + 'static) -> Self {
        self.handlers.on_reorder = Some(Rc::new(f));
        self
    }

    /// The columns the table shows, in order, without hidden ones
    /// ([`TableState::visible_columns`] over the declared columns). Rows an app draws itself
    /// under [`Self::header`] size each cell with [`TableColumn::size_el`] and
    /// [`TableState::column_width`], so they line up with the header's columns.
    pub fn visible_columns(&self, s: &S) -> Vec<TableColumn> {
        (self.handlers.get)(s).visible_columns(&self.core.columns)
    }

    /// Only the header row, with its sorting, resizing, reordering and header hooks, to put over
    /// rows the app draws itself (its own selection, presses and drags). Lay the rows' cells out
    /// from [`Self::visible_columns`] in a full-width row so they line up with the header; a
    /// flex column's resolved width is [`TableState::laid_out_width`].
    ///
    /// ```
    /// use std::borrow::Cow;
    /// use rune::{col, row, text};
    /// use rune_kit::{Kit, Table, TableColumn, TableState};
    ///
    /// struct App { rows: Vec<String>, table: TableState }
    /// let app = App { rows: vec!["alpha".into()], table: TableState::new() };
    /// let k = Kit::neutral(true);
    /// let table = || {
    ///     Table::new(|s: &App| &s.table, |s: &mut App| &mut s.table, |s: &App| s.rows.len(), |s: &App, i| Cow::Borrowed(s.rows[i].as_str()))
    ///         .column(TableColumn::new("name", "Name").flex(1.0))
    ///         .column(TableColumn::new("size", "Size").width(80.0))
    /// };
    /// let columns = table().visible_columns(&app);
    /// let mut list = col().child(table().header(&app, &k));
    /// for name in &app.rows {
    ///     let mut r = row().w_full();
    ///     for c in &columns {
    ///         r = r.child(c.size_el(row().child(text(name.clone())), app.table.column_width(c.id())));
    ///     }
    ///     list = list.child(r);
    /// }
    /// ```
    pub fn header(self, s: &S, k: &Kit) -> El {
        let state = (self.handlers.get)(s);
        self.core.header(state, k, &self.handlers)
    }

    /// The table, filling the height its parent gives it.
    pub fn build(self, s: &S, k: &Kit) -> El {
        let state = (self.handlers.get)(s);
        let count = (self.handlers.len)(s);
        self.core.build(state, count, k, &self.handlers)
    }
}

