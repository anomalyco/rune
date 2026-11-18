//! Drawing a [`DiffState`]: a virtual list of file headers, hunk separators and lines, a
//! sticky header for the file being read, and the handlers that feed selection, expansion and
//! the reading position back into the state.

use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;

use rune::{Color, Cursor, Drag, El, GrowthSpec, Icon, Phase, ScrollSpec, Tx, col, icon, memo, rich, row, text};

use super::model::{LineKind, Side};
use super::state::{Annotation, DiffMode, DiffState, LINE_H, LineRange, LineRef, Row};
use crate::Kit;

mod chrome;
mod line_info;
mod look;
mod spans;

pub use line_info::FoldRow;

use look::Look;
use spans::{layer, tabbed};

static PLUS: Icon = Icon {
    name: "kit-diff-plus",
    view: 16.0,
    body: r#"<path d="M8 4V12M4 8H12" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"/>"#,
};

/// Width of the coloured bar that marks an added or removed line.
const BAR: f32 = 3.0;
/// Space between the line numbers and the code, and after the code.
const CODE_PAD: (f32, f32) = (12.0, 24.0);

type Get<S> = Rc<dyn Fn(&S) -> &DiffState>;
type GetMut<S> = Rc<dyn Fn(&mut S) -> &mut DiffState>;

/// Draws the [`DiffState`] an app keeps in its state: give it the state's accessors and build
/// it in the view. It fills the height it's given and scrolls; Cmd-C, `j`/`k` and `]`/`[` work
/// once the app routes keys to [`DiffState::key`].
///
/// ```ignore
/// DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
///     .annotation(|s: &App, a: &Annotation| comment(s, &a.id))
///     .on_comment(|s: &mut App, lines, _| s.compose(lines))
///     .on_visible_file(|s: &mut App, f| s.tree.reveal(f))
///     .build(s, &k)
/// ```
#[allow(clippy::type_complexity)]
pub struct DiffView<S> {
    get: Get<S>,
    get_mut: GetMut<S>,
    key: String,
    fold_row: FoldRow,
    annotation: Option<Rc<dyn Fn(&S, &Annotation) -> El>>,
    header: Option<Rc<dyn Fn(&S, usize) -> El>>,
    on_select: Option<Rc<dyn Fn(&mut S, Option<LineRange>, &mut Tx)>>,
    on_comment: Option<Rc<dyn Fn(&mut S, LineRange, &mut Tx)>>,
    on_visible_file: Option<Rc<dyn Fn(&mut S, usize)>>,
}

impl<S: 'static> DiffView<S> {
    pub fn new(get: impl Fn(&S) -> &DiffState + 'static, get_mut: impl Fn(&mut S) -> &mut DiffState + 'static) -> DiffView<S> {
        DiffView {
            get: Rc::new(get),
            get_mut: Rc::new(get_mut),
            key: "diff".into(),
            fold_row: FoldRow::default(),
            annotation: None,
            header: None,
            on_select: None,
            on_comment: None,
            on_visible_file: None,
        }
    }

    /// The view's element key (`"diff"` by default); give each diff in a window its own.
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// How folded unchanged lines draw: [`FoldRow::Summary`] by default, or Pierre's
    /// [`FoldRow::LineInfo`] as OpenCode Desktop's review has them.
    pub fn fold_row(mut self, fold_row: FoldRow) -> Self {
        self.fold_row = fold_row;
        self
    }

    /// Build each [`Annotation`] under its line. Rows holding annotations are built every
    /// commit, so a widget can read app state (a composer's text) freely.
    pub fn annotation(mut self, f: impl Fn(&S, &Annotation) -> El + 'static) -> Self {
        self.annotation = Some(Rc::new(f));
        self
    }

    /// Extra controls at the end of each file's header (a "viewed" toggle, a menu), by file.
    pub fn header_actions(mut self, f: impl Fn(&S, usize) -> El + 'static) -> Self {
        self.header = Some(Rc::new(f));
        self
    }

    /// Called when a click, shift-click or drag on the line numbers ends, with the selection.
    pub fn on_select(mut self, f: impl Fn(&mut S, Option<LineRange>, &mut Tx) + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// Show a "+" by the line numbers of the hovered line, and call `f` with the lines it
    /// stands for when pressed: the selection when the line is in it, else that line.
    pub fn on_comment(mut self, f: impl Fn(&mut S, LineRange, &mut Tx) + 'static) -> Self {
        self.on_comment = Some(Rc::new(f));
        self
    }

    /// Called when the file at the top of the view changes as it scrolls, to follow it in a
    /// file tree.
    pub fn on_visible_file(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self {
        self.on_visible_file = Some(Rc::new(f));
        self
    }

    pub fn build(self, s: &S, k: &Kit) -> El {
        let st = (self.get)(s);
        let look = Look::new(k);
        let wrap = st.wraps();
        let key = self.key.clone();
        if !wrap && let Some(r) = rune::memo::layout(&format!("{key}/scroll")) {
            st.set_viewport(r);
        }
        let v = Rc::new(View { look, d: self });
        let count = st.rows.len();
        let mut list = col().key("rows").w_full().growth(GrowthSpec::default()).selection_group(true).role("list").label("Diff");
        if !wrap {
            list = list.min_w(v.content_width(st).max(st.view_width()));
        }
        let build = v.clone();
        let vis_mut = v.d.get_mut.clone();
        let on_file = v.d.on_visible_file.clone();
        let list = list
            .virtual_rows(count, LINE_H, keys(v.d.get.clone()), move |s: &S, i| build.row(s, i))
            .virtual_rows_version(st.keys_rev)
            .on_visible_rows(move |s: &mut S, rows| {
                let (reported, sticky_changed) = vis_mut(s).set_visible_rows(rows);
                if let (Some(f), Some(cb)) = (reported, &on_file) {
                    cb(s, f);
                    return true;
                }
                sticky_changed
            });
        let scroll = col().key("scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false });
        let scroll = if wrap { scroll } else { scroll.scroll_xy() };
        let scroll = scroll.child(list);
        // Code reads left to right in any window: gutters at the left, old before new.
        let mut root = col().key(key).dir(rune::Direction::Ltr).w_full().grow(1.0).min_h(0.0).fill(v.look.bg).child(scroll);
        if let Some(f) = st.sticky() {
            root = root.child(v.file_header(s, st, f, true).key("sticky").absolute(0.0, 0.0).w_full());
        }
        root
    }
}

/// The row-key closure for [`El::virtual_rows`], borrowing the keys from the state.
fn keys<S: 'static>(get: Get<S>) -> impl for<'a> Fn(&'a S, usize) -> Cow<'a, str> + 'static {
    move |s, i| Cow::Borrowed(get(s).keys[i].as_str())
}

/// One side's line in a row: what a cell draws.
#[derive(Clone, Copy)]
struct Cell<'a> {
    kind: LineKind,
    old: Option<u32>,
    new: Option<u32>,
    text: &'a str,
    changes: &'a [Range<usize>],
    no_newline: bool,
}

impl Cell<'_> {
    /// The side its text is read from, and its number there.
    fn source(&self) -> (Side, u32) {
        match (self.kind, self.old, self.new) {
            (LineKind::Removed, Some(n), _) => (Side::Old, n),
            (_, _, Some(n)) => (Side::New, n),
            (_, Some(n), None) => (Side::Old, n),
            _ => (Side::New, 1),
        }
    }

    fn label(&self) -> String {
        let (_, n) = self.source();
        let kind = match self.kind {
            LineKind::Added => "added",
            LineKind::Removed => "removed",
            LineKind::Context => "unchanged",
        };
        format!("{kind} line {n}: {}", self.text)
    }
}

struct View<S> {
    d: DiffView<S>,
    look: Look,
}

impl<S: 'static> View<S> {
    /// How wide unwrapped (unified) content is, so the list doesn't change width as rows are built.
    fn content_width(&self, st: &DiffState) -> f32 {
        let l = &self.look;
        let digits = (0..st.files().len()).map(|f| st.digits(f)).max().unwrap_or(3);
        BAR + 2.0 * l.number_w(digits) + st.columns() as f32 * l.adv + CODE_PAD.0 + CODE_PAD.1
    }

    /// The most a full-width part of a row (a header, an annotation) spans: the viewport, not
    /// the unwrapped content.
    fn view_w(&self, st: &DiffState, split_half: bool) -> Option<f32> {
        let w = st.view_width();
        (w > 0.0 && !st.wraps()).then_some(if split_half { (w / 2.0).floor() } else { w })
    }

    fn row(&self, s: &S, i: usize) -> El {
        let st = (self.d.get)(s);
        let r = st.rows[i];
        let key = st.keys[i].as_str();
        match r {
            Row::File(f) => self.file_header(s, st, f as usize, false),
            Row::Note(f) => memo(key, (self.look.version, st.rev, st.view_width().to_bits()), || self.note(st, f as usize)),
            Row::Gap { f, gap } => memo(key, (self.look.version, st.rev, st.view_width().to_bits(), self.d.fold_row), || match self.d.fold_row {
                FoldRow::Summary => self.gap(st, f as usize, gap as usize),
                FoldRow::LineInfo => self.line_info(st, f as usize, gap as usize),
            }),
            _ if !st.annotations().is_empty() && self.d.annotation.is_some() && self.annotated(st, r) => self.line(s, st, i),
            _ => {
                let version = (self.look.version, st.rev, st.row_selected(i, None), st.copy_side(), st.selection().map(|s| s.start.side));
                memo(key, version, || self.line(s, st, i))
            }
        }
    }

    fn annotated(&self, st: &DiffState, r: Row) -> bool {
        let f = r.file();
        [Side::Old, Side::New].into_iter().any(|side| st.row_line(r, side).is_some_and(|n| st.annotations_at(f, LineRef::new(side, n)).next().is_some()))
    }

    fn cells<'a>(&self, st: &'a DiffState, r: Row) -> (Option<Cell<'a>>, Option<Cell<'a>>) {
        let of = |f: u32, h: u32, i: u32| {
            let l = &st.files()[f as usize].hunks[h as usize].lines[i as usize];
            Cell { kind: l.kind, old: l.old, new: l.new, text: &l.text, changes: &l.changes, no_newline: l.no_newline }
        };
        match r {
            Row::Ctx { f, old, new } => {
                let text = st.side_line(f as usize, Side::New, new).or_else(|| st.side_line(f as usize, Side::Old, old)).unwrap_or("");
                let c = Cell { kind: LineKind::Context, old: Some(old), new: Some(new), text, changes: &[], no_newline: false };
                (Some(c), None)
            }
            Row::Line { f, h, i } => (Some(of(f, h, i)), None),
            Row::Pair { f, h, del, add } => (Some(of(f, h, del)), Some(of(f, h, add))),
            _ => (None, None),
        }
    }

    fn line(&self, s: &S, st: &DiffState, i: usize) -> El {
        let r = st.rows[i];
        let f = r.file();
        let (a, b) = self.cells(st, r);
        let Some(a) = a else { return col() };
        let line = match st.mode() {
            DiffMode::Unified => self.unified(st, i, f, a),
            DiffMode::Split => {
                let (old, new) = match (b, a.kind) {
                    (Some(b), _) => (Some(a), Some(b)),
                    (None, LineKind::Removed) => (Some(a), None),
                    (None, LineKind::Added) => (None, Some(a)),
                    (None, LineKind::Context) => (Some(a), Some(a)),
                };
                self.split(st, i, f, old, new)
            }
        };
        let Some(build) = self.d.annotation.as_ref() else { return line };
        let notes = |side: Side| -> Vec<El> {
            st.row_line(r, side).map_or(Vec::new(), |n| st.annotations_at(f, LineRef::new(side, n)).map(|a| note(build(s, a), a)).collect())
        };
        let (old, new) = (notes(Side::Old), notes(Side::New));
        if old.is_empty() && new.is_empty() {
            return line;
        }
        let mut out = col().w_full().child(line.key("line"));
        match st.mode() {
            DiffMode::Unified => {
                let mut c = col().key("notes").w_full();
                if let Some(w) = self.view_w(st, false) {
                    c = c.w(w).sticky_left(0.0);
                }
                // An unchanged line's annotations from either side, once.
                let mut seen = std::collections::HashSet::new();
                let all = st
                    .row_line(r, Side::Old)
                    .into_iter()
                    .flat_map(|n| st.annotations_at(f, LineRef::new(Side::Old, n)))
                    .chain(st.row_line(r, Side::New).into_iter().flat_map(|n| st.annotations_at(f, LineRef::new(Side::New, n))))
                    .filter(|a| seen.insert(a.id.clone()));
                out = out.child(c.children(all.map(|a| note(build(s, a), a))));
            }
            DiffMode::Split => {
                let single = st.single_column(f);
                let half = |key: &'static str, els: Vec<El>| {
                    let mut c = col().key(key).w_frac(if single.is_some() { 1.0 } else { 0.5 }).min_w(0.0);
                    if let Some(w) = self.view_w(st, single.is_none()) {
                        c = c.max_w(w);
                    }
                    c.children(els)
                };
                let notes = row().key("notes").w_full().items_start();
                out = out.child(match single {
                    Some(Side::Old) => notes.child(half("old", old)),
                    Some(Side::New) => notes.child(half("new", new)),
                    None => notes.child(half("old", old)).child(half("new", new)),
                });
            }
        }
        out
    }

    fn unified(&self, st: &DiffState, i: usize, f: usize, c: Cell<'_>) -> El {
        let l = &self.look;
        let selected = st.row_selected(i, None);
        let tints = l.tints(c.kind);
        let fill = if selected { l.selected } else { tints.map_or(l.bg, |t| t.line) };
        let digits = st.digits(f);
        let copy = match c.kind {
            LineKind::Context => true,
            LineKind::Added => st.copy_side() == Side::New,
            LineKind::Removed => st.copy_side() == Side::Old,
        };
        let hover_side = if c.kind == LineKind::Removed { Side::Old } else { Side::New };
        // The marker, numbers and "+" hold to the viewport's left edge while the code scrolls
        // sideways under them, on the gutter's own opaque fill.
        let mut gutter = row()
            .key("gutter")
            .self_stretch()
            .items_stretch()
            .fill(l.gutter_fill(c.kind, selected))
            .child(self.bar(c.kind))
            .child(self.number(st, i, f, c.old, Side::Old, c.kind, selected, digits).key("no"))
            .child(self.number(st, i, f, c.new, Side::New, c.kind, selected, digits).key("nn"));
        if self.d.on_comment.is_some() {
            let at = c.new.map(|n| LineRef::new(Side::New, n)).or(c.old.map(|n| LineRef::new(Side::Old, n)));
            if let Some(at) = at {
                gutter = gutter.child(self.plus(f, at).absolute(BAR + 2.0 * l.number_w(digits) - 10.0, 2.0));
            }
        }
        if !st.wraps() {
            gutter = gutter.sticky_left(0.0);
        }
        row()
            .w_full()
            .min_h(LINE_H)
            .items_start()
            .fill(fill)
            .hover(l.hover)
            .hover_group()
            .role("listitem")
            .label(c.label())
            .child(gutter)
            .child(self.code(st, f, &c, copy, hover_side, &st.keys[i]).key("code"))
    }

    fn split<'a>(&self, st: &'a DiffState, i: usize, f: usize, old: Option<Cell<'a>>, new: Option<Cell<'a>>) -> El {
        let digits = st.digits(f);
        let single = st.single_column(f);
        let half = |side: Side, c: Option<Cell<'a>>| -> El {
            let l = &self.look;
            let key = if side == Side::Old { "old" } else { "new" };
            let mut h = row().key(key).w_frac(if single.is_some() { 1.0 } else { 0.5 }).min_w(0.0).min_h(LINE_H).items_start().self_stretch();
            let Some(c) = c else {
                return h.fill(l.empty_bg);
            };
            // An unchanged line shows on both sides as context.
            let kind = c.kind;
            let selected = st.row_selected(i, Some(side));
            let fill = if selected { l.selected } else { l.tints(kind).map_or(l.bg, |t| t.line) };
            let n = if side == Side::Old { c.old } else { c.new };
            let mut cell = c;
            if kind == LineKind::Context {
                cell.text = st.side_line(f, side, n.unwrap_or(1)).unwrap_or(c.text);
            }
            let copy = st.copy_side() == side;
            h = h
                .fill(fill)
                .hover(l.hover)
                .hover_group()
                .role("listitem")
                .label(cell.label())
                .child(self.bar(kind))
                .child(self.number(st, i, f, n, side, kind, selected, digits).key("n"))
                .child(self.code(st, f, &cell, copy, side, &st.keys[i]).key("code"));
            if let (Some(n), true) = (n, self.d.on_comment.is_some()) {
                h = h.child(self.plus(f, LineRef::new(side, n)).absolute(BAR + l.number_w(digits) - 10.0, 2.0));
            }
            h
        };
        let line = row().w_full().min_h(LINE_H).items_stretch();
        // An added or deleted file has one side: it takes the whole width, as one column.
        match single {
            Some(Side::Old) => line.child(half(Side::Old, old)),
            Some(Side::New) => line.child(half(Side::New, new)),
            None => {
                let divider = col().key("divider").w(1.0).self_stretch().fill(self.look.border);
                line.child(half(Side::Old, old)).child(divider).child(half(Side::New, new))
            }
        }
    }

    fn bar(&self, kind: LineKind) -> El {
        let b = col().key("bar").w(BAR).self_stretch().a11y_hidden();
        match self.look.tints(kind) {
            Some(t) => b.fill(t.bar),
            None => b,
        }
    }

    /// A line-number cell: pressing it selects the line, shift-pressing extends the selection
    /// to it, and dragging down the numbers selects the lines passed.
    #[allow(clippy::too_many_arguments)]
    fn number(&self, st: &DiffState, i: usize, f: usize, n: Option<u32>, side: Side, kind: LineKind, selected: bool, digits: u32) -> El {
        let l = &self.look;
        let unified = st.mode() == DiffMode::Unified;
        let mut c = row().w(l.number_w(digits)).min_h(LINE_H).self_stretch().px(8.0).justify_end().items_start().a11y_hidden();
        if !unified {
            c = c.fill(l.gutter_fill(kind, selected));
        }
        if let Some(n) = n {
            let color = if selected { l.text } else { l.tints(kind).map_or(l.faint, |t| t.number) };
            c = c.child(text(n.to_string()).font(l.font).color(color));
        }
        let at = n.map(|n| LineRef::new(side, n)).or_else(|| {
            let other = if side == Side::Old { Side::New } else { Side::Old };
            unified.then(|| st.row_line(st.rows[i], other).map(|n| LineRef::new(other, n))).flatten()
        });
        let Some(at) = at else { return c };
        let (get, on_select) = (self.d.get_mut.clone(), self.d.on_select.clone());
        let row_at = i as u32;
        c.cursor(Cursor::Pointer).on_drag(move |s: &mut S, d: &mut Drag, tx| {
            let st = get(s);
            match d.phase {
                Phase::Down => st.press_line(f, at, tx.mods().shift),
                Phase::Move => {
                    let Some(r) = st.row_from(row_at, d.y) else { return };
                    let r = st.rows[r as usize];
                    if r.file() != f {
                        return;
                    }
                    // Unified rows hold one side or both: keep to the pressed side where it can.
                    let to = st.row_line(r, side).map(|n| LineRef::new(side, n)).or_else(|| {
                        let other = if side == Side::Old { Side::New } else { Side::Old };
                        unified.then(|| st.row_line(r, other).map(|n| LineRef::new(other, n))).flatten()
                    });
                    if let Some(to) = to {
                        st.drag_to(f, to);
                    }
                }
                Phase::Up => {
                    if d.cancelled() {
                        st.cancel_line();
                        return;
                    }
                    let sel = st.release_line();
                    if let Some(cb) = &on_select {
                        cb(s, sel, tx);
                    }
                }
            }
        })
    }

    /// The "+" that appears by the numbers of the hovered line.
    fn plus(&self, f: usize, at: LineRef) -> El {
        let l = &self.look;
        let (get, cb) = (self.d.get_mut.clone(), self.d.on_comment.clone());
        row()
            .key("plus")
            .size(16.0, 16.0)
            .center()
            .radius(4.0)
            .fill(l.accent)
            .group_opacity(0.0, 1.0)
            .cursor(Cursor::Pointer)
            .role("button")
            .label("Comment on this line")
            .child(icon(&PLUS, 12.0).color(l.on_accent))
            .on_press_tx(move |s: &mut S, tx| {
                let st = get(s);
                let lines = match st.selection() {
                    Some(sel) if sel.file == f && st.contains(&sel, at) => sel,
                    _ => LineRange::line(f, at),
                };
                if let Some(cb) = &cb {
                    cb(s, lines, tx);
                }
            })
    }

    /// The code of a line: syntax colours under word changes, tabs padded to four columns.
    fn code(&self, st: &DiffState, f: usize, c: &Cell<'_>, copy: bool, side: Side, row_key: &str) -> El {
        let l = &self.look;
        let (src, n) = c.source();
        let syntax: Vec<(Range<usize>, Color)> = st
            .highlights(f, src, n)
            .into_iter()
            .map(|(r, p)| (r, l.syntax.paint(p)))
            .collect();
        let word = l.tints(c.kind).map(|t| t.word);
        let spans = layer(c.text.len(), &syntax, word.map_or(&[][..], |_| c.changes), word);
        let (shown, spans, pads) = tabbed(c.text, spans, st.wraps());
        let mut t = rich(shown, spans).font(l.font).color(l.text).key("text");
        t = if st.wraps() { t.w_full() } else { t.single_line() };
        if copy {
            t = t.selectable(true).selection_color(l.selection);
            t = pads.into_iter().fold(t, |t, r| t.uncopied(r));
        }
        let (get, cell) = (self.d.get_mut.clone(), format!("{row_key}.{}", side as u8));
        let mut code = col().grow(1.0).min_w(0.0).pl(CODE_PAD.0).pr(CODE_PAD.1).child(t).on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
            if !get(s).hover_text(side, &cell, inside) { tx.live_only(); }
        });
        if c.no_newline {
            code = code.child(text("No newline at end of file").font(l.small).color(l.muted).opacity(0.6).key("eof").pb(2.0));
        }
        code
    }

    fn gutter_w(&self, st: &DiffState, f: usize) -> f32 {
        let n = self.look.number_w(st.digits(f));
        BAR + if st.mode() == DiffMode::Unified { 2.0 * n } else { n }
    }
}

/// An annotation's widget, keyed by its id so it keeps its state as rows come and go.
fn note(el: El, a: &Annotation) -> El {
    col().key(format!("a.{}", a.id)).w_full().child(el)
}
