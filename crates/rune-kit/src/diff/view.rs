//! Drawing a [`DiffState`]: a virtual list of file headers, hunk separators and lines, a
//! sticky header for the file being read, and the handlers that feed selection, expansion and
//! the reading position back into the state.

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use rune::{Color, Cursor, Drag, El, GrowthSpec, Icon, Phase, ScrollSpec, Span, TextStyle, Tx, col, icon, memo, rich, row, text};

use super::model::{FileStatus, LineKind, Side};
use super::state::{Annotation, DiffMode, DiffState, EXPAND_STEP, Expand, GAP_H, HEADER_H, LINE_H, LineRange, LineRef, NOTE_H, Row};
use crate::Kit;
use crate::palette::tint;
use crate::syntax::SyntaxTheme;

static EXPAND_UP: Icon = Icon {
    name: "kit-diff-expand-up",
    view: 16.0,
    body: r#"<path d="M4.5 9.5L8 6L11.5 9.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static EXPAND_DOWN: Icon = Icon {
    name: "kit-diff-expand-down",
    view: 16.0,
    body: r#"<path d="M4.5 6.5L8 10L11.5 6.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
static EXPAND_ALL: Icon = Icon {
    name: "kit-diff-expand-all",
    view: 16.0,
    body: r#"<path d="M5 5.5L8 2.5L11 5.5M5 10.5L8 13.5L11 10.5" stroke="currentColor" stroke-width="1.25" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};
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
        let mut root = col().key(key).w_full().grow(1.0).min_h(0.0).fill(v.look.bg).child(scroll);
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

/// Colours and type, from the kit's palette so themes restyle them.
struct Look {
    font: TextStyle,
    label: TextStyle,
    small: TextStyle,
    /// The advance of a mono column.
    adv: f32,
    bg: Color,
    text: Color,
    muted: Color,
    faint: Color,
    border: Color,
    header_bg: Color,
    hover: Color,
    add: Tints,
    del: Tints,
    gap_bg: Color,
    empty_bg: Color,
    selected: Color,
    selected_gutter: Color,
    selection: Color,
    accent: Color,
    on_accent: Color,
    triangle_down: &'static Icon,
    triangle_right: &'static Icon,
    syntax: SyntaxTheme,
    /// Changes when anything above does, for memoised rows.
    version: u64,
}

#[derive(Clone, Copy)]
struct Tints {
    line: Color,
    gutter: Color,
    word: Color,
    number: Color,
    bar: Color,
    count: Color,
}

impl Look {
    fn new(k: &Kit) -> Look {
        let c = &k.c;
        let bg = c.bg_base;
        let dark = k.dark;
        let tints = |hue: Color, fg: Color| Tints {
            line: tint(bg, hue, if dark { 0.13 } else { 0.1 }),
            gutter: tint(bg, hue, if dark { 0.2 } else { 0.17 }),
            word: tint(bg, hue, if dark { 0.32 } else { 0.27 }),
            number: tint(c.text_muted, hue, 0.7),
            bar: hue,
            count: fg,
        };
        let font = TextStyle { size: 13.0, line_height: LINE_H, ..k.ty.mono };
        let info = c.info.icon;
        let mut look = Look {
            font,
            label: TextStyle { size: 13.0, line_height: 20.0, ..k.ty.ui_medium },
            small: TextStyle { size: 12.0, line_height: 16.0, ..k.ty.mono },
            adv: 0.6 * font.size,
            bg,
            text: c.text_base,
            muted: c.text_muted,
            faint: c.text_faint,
            border: c.border_weak,
            header_bg: c.layer_01,
            hover: k.alpha(c.text_base, if dark { 0.06 } else { 0.035 }),
            add: tints(c.success.icon, c.success.fg),
            del: tints(c.danger.icon, c.danger.fg),
            gap_bg: tint(bg, info, if dark { 0.1 } else { 0.06 }),
            empty_bg: tint(bg, c.text_base, if dark { 0.03 } else { 0.025 }),
            selected: tint(bg, info, if dark { 0.2 } else { 0.14 }),
            selected_gutter: tint(bg, info, if dark { 0.32 } else { 0.26 }),
            selection: k.selection(),
            accent: info,
            on_accent: c.white,
            triangle_down: k.glyph.triangle_down,
            triangle_right: k.glyph.triangle_right,
            syntax: SyntaxTheme::from_kit(k),
            version: 0,
        };
        let mut h = std::hash::DefaultHasher::new();
        for c in [look.bg, look.text, look.add.bar, look.del.bar, look.accent, look.header_bg] {
            [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(&mut h);
        }
        look.font.size.to_bits().hash(&mut h);
        look.version = h.finish();
        look
    }

    fn tints(&self, kind: LineKind) -> Option<&Tints> {
        match kind {
            LineKind::Added => Some(&self.add),
            LineKind::Removed => Some(&self.del),
            LineKind::Context => None,
        }
    }

    fn gutter_fill(&self, kind: LineKind, selected: bool) -> Color {
        match (selected, self.tints(kind)) {
            (true, _) => self.selected_gutter,
            (false, Some(t)) => t.gutter,
            (false, None) => self.bg,
        }
    }

    /// One line-number column for numbers of `digits` digits.
    fn number_w(&self, digits: u32) -> f32 {
        (digits as f32 * self.adv + 16.0).ceil()
    }
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
            Row::Gap { f, gap } => memo(key, (self.look.version, st.rev, st.view_width().to_bits()), || self.gap(st, f as usize, gap as usize)),
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
                let half = |key: &'static str, els: Vec<El>| {
                    let mut c = col().key(key).w_frac(0.5).min_w(0.0);
                    if let Some(w) = self.view_w(st, true) {
                        c = c.max_w(w);
                    }
                    c.children(els)
                };
                out = out.child(row().key("notes").w_full().items_start().child(half("old", old)).child(half("new", new)));
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
        let half = |side: Side, c: Option<Cell<'a>>| -> El {
            let l = &self.look;
            let key = if side == Side::Old { "old" } else { "new" };
            let mut h = row().key(key).w_frac(0.5).min_w(0.0).min_h(LINE_H).items_start().self_stretch();
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
        let divider = col().key("divider").w(1.0).self_stretch().fill(self.look.border);
        row().w_full().min_h(LINE_H).items_stretch().child(half(Side::Old, old)).child(divider).child(half(Side::New, new))
    }

    fn bar(&self, kind: LineKind) -> El {
        let b = col().key("bar").w(BAR).self_stretch();
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
        let mut c = row().w(l.number_w(digits)).min_h(LINE_H).self_stretch().px(8.0).justify_end().items_start();
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
                Phase::Down => st.press_line(f, at),
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

    /// The unchanged lines between two hunks, folded: buttons to show more, and the next
    /// hunk's header.
    fn gap(&self, st: &DiffState, f: usize, gap: usize) -> El {
        let l = &self.look;
        let file = &st.files()[f];
        let hidden = st.hidden(f, gap).unwrap_or(0);
        let last = gap == file.hunks.len();
        let expandable = file.expandable();
        let mut buttons = Vec::new();
        if expandable {
            let mut add = |how: Expand, glyph: &'static Icon, label: String| {
                let get = self.d.get_mut.clone();
                buttons.push(
                    row()
                        .key(format!("{how:?}"))
                        .size(24.0, 24.0)
                        .center()
                        .radius(6.0)
                        .hover(l.hover)
                        .cursor(Cursor::Pointer)
                        .role("button")
                        .label(label)
                        .child(icon(glyph, 16.0).color(l.muted))
                        .on_press_tx(move |s: &mut S, _| get(s).expand(f, gap, how)),
                );
            };
            let step = hidden.min(EXPAND_STEP);
            if hidden <= EXPAND_STEP {
                add(Expand::All, &EXPAND_ALL, format!("Show {hidden} hidden lines"));
            } else {
                if gap > 0 {
                    add(Expand::Down, &EXPAND_DOWN, format!("Show {step} more lines below"));
                }
                if !last {
                    add(Expand::Up, &EXPAND_UP, format!("Show {step} more lines above"));
                }
            }
        }
        let buttons = row().key("expand").w(self.gutter_w(st, f)).h(GAP_H).center().gap(2.0).children(buttons);
        let mut inner = row().key("inner").h(GAP_H).items_center().gap(12.0).child(buttons);
        if let Some(w) = self.view_w(st, false) {
            inner = inner.max_w(w).sticky_left(0.0);
        }
        if hidden > 0 {
            let count = text(if hidden == 1 { "1 unchanged line".to_owned() } else { format!("{hidden} unchanged lines") }).font(l.small).color(l.muted).key("count");
            let count = if expandable && hidden > EXPAND_STEP {
                let get = self.d.get_mut.clone();
                row().key("all").px(6.0).h(24.0).items_center().radius(6.0).hover(l.hover).cursor(Cursor::Pointer).role("button").label(format!("Show all {hidden} lines")).on_press_tx(move |s: &mut S, _| get(s).expand(f, gap, Expand::All)).child(count)
            } else {
                row().key("all").px(6.0).child(count)
            };
            inner = inner.child(count);
        }
        if let Some(h) = file.hunks.get(gap) {
            inner = inner.child(text(h.header()).font(l.small).color(l.faint).single_line().key("header"));
        }
        let label = match file.hunks.get(gap) {
            Some(h) => format!("{hidden} unchanged lines, then {}", h.header()),
            None => format!("{hidden} unchanged lines"),
        };
        row().w_full().h(GAP_H).items_center().fill(l.gap_bg).role("group").label(label).child(inner)
    }

    /// A file with no lines to show.
    fn note(&self, st: &DiffState, f: usize) -> El {
        let l = &self.look;
        let file = &st.files()[f];
        let msg = if file.binary {
            "Binary file not shown"
        } else if file.status == FileStatus::Renamed || file.status == FileStatus::Copied {
            "File moved without changes"
        } else if file.old_mode != file.new_mode {
            "File mode changed"
        } else if file.status == FileStatus::Added || file.status == FileStatus::Deleted {
            "Empty file"
        } else {
            "No changes"
        };
        let mut inner = row().key("inner").h(NOTE_H).items_center().pl(self.gutter_w(st, f) + CODE_PAD.0).child(text(msg).font(l.small).color(l.muted));
        if let Some(w) = self.view_w(st, false) {
            inner = inner.max_w(w).sticky_left(0.0);
        }
        row().w_full().h(NOTE_H).fill(l.bg).role("note").label(msg).child(inner)
    }

    fn file_header(&self, s: &S, st: &DiffState, f: usize, sticky: bool) -> El {
        let l = &self.look;
        let file = &st.files()[f];
        let collapsed = st.collapsed(f);
        let get = self.d.get_mut.clone();
        let toggle = row()
            .key("toggle")
            .size(24.0, 24.0)
            .center()
            .radius(6.0)
            .hover(l.hover)
            .cursor(Cursor::Pointer)
            .role("button")
            .label(if collapsed { "Expand file" } else { "Collapse file" })
            .child(icon(if collapsed { l.triangle_right } else { l.triangle_down }, 16.0).color(l.muted))
            .on_press_tx(move |s: &mut S, tx| {
                let st = get(s);
                st.set_collapsed(f, !st.collapsed(f));
                if sticky {
                    st.scroll_to_file(f, tx);
                }
            });
        let path = |p: &str, c: Color| text(p.to_owned()).font(l.label).color(c).truncate_head().min_w(0.0).shrink();
        let mut name = row().key("path").items_center().gap(6.0).min_w(0.0).shrink();
        let (old, new) = (file.old_path.as_deref(), file.new_path.as_deref());
        name = match (file.status, old, new) {
            (FileStatus::Renamed | FileStatus::Copied, Some(a), Some(b)) => {
                name.child(path(a, l.muted).key("old")).child(text("→").font(l.label).color(l.faint).key("arrow")).child(path(b, l.text).key("new"))
            }
            _ => name.child(path(file.path(), l.text).key("new")),
        };
        let status = match file.status {
            FileStatus::Added => Some(("Added", l.add.count)),
            FileStatus::Deleted => Some(("Deleted", l.del.count)),
            FileStatus::Renamed => Some(("Renamed", l.muted)),
            FileStatus::Copied => Some(("Copied", l.muted)),
            FileStatus::Modified => None,
        };
        let mut inner = row().key("inner").w_full().grow(1.0).px(10.0).gap(8.0).items_center().child(toggle).child(name);
        if let Some((word, color)) = status {
            inner = inner.child(text(word).font(l.small).color(color).key("status"));
        }
        if let (Some(a), Some(b)) = (&file.old_mode, &file.new_mode)
            && a != b
        {
            inner = inner.child(text(format!("{a} → {b}")).font(l.small).color(l.faint).key("mode"));
        }
        inner = inner.child(row().key("space").grow(1.0));
        let (adds, dels) = (file.additions(), file.deletions());
        if adds > 0 {
            inner = inner.child(text(format!("+{adds}")).font(l.small).color(l.add.count).key("adds"));
        }
        if dels > 0 {
            inner = inner.child(text(format!("−{dels}")).font(l.small).color(l.del.count).key("dels"));
        }
        if let Some(h) = &self.d.header {
            inner = inner.child(h(s, f).key("actions"));
        }
        let mut label = file.path().to_owned();
        if let (FileStatus::Renamed, Some(a)) = (file.status, old) {
            label.push_str(&format!(", renamed from {a}"));
        }
        label.push_str(&format!(", {adds} added, {dels} removed"));
        let line = |key: &'static str, c: Color| col().key(key).w_full().h(1.0).fill(c);
        let mut h = col().w_full().h(HEADER_H).fill(l.header_bg).role("heading").label(label);
        // In the list, as wide as the view and held to its left edge while the code scrolls
        // sideways, so the counts and actions stay in sight.
        if let (false, Some(w)) = (sticky, self.view_w(st, false)) {
            h = h.w(w).sticky_left(0.0);
        }
        h = h.child(line("top", if f > 0 && !sticky { l.border } else { l.header_bg }));
        h.child(inner).child(line("bottom", l.border))
    }
}

/// An annotation's widget, keyed by its id so it keeps its state as rows come and go.
fn note(el: El, a: &Annotation) -> El {
    col().key(format!("a.{}", a.id)).w_full().child(el)
}

/// Spans for a line from its syntax colours and the byte ranges given `bg` (word changes):
/// cut where either starts or ends, so each piece has one colour and one background.
fn layer(len: usize, syntax: &[(Range<usize>, Color)], words: &[Range<usize>], bg: Option<Color>) -> Vec<Span> {
    let mut cuts: Vec<usize> = syntax.iter().flat_map(|(r, _)| [r.start, r.end]).chain(words.iter().flat_map(|r| [r.start, r.end])).collect();
    cuts.retain(|&c| c <= len);
    cuts.sort_unstable();
    cuts.dedup();
    let mut out: Vec<Span> = Vec::new();
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let color = syntax.get(syntax.partition_point(|(r, _)| r.end <= a)).filter(|(r, _)| r.start <= a).map(|(_, c)| *c);
        let on = words.get(words.partition_point(|r| r.end <= a)).is_some_and(|r| r.start <= a);
        let bg = bg.filter(|_| on);
        if color.is_none() && bg.is_none() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.end as usize == a && last.color == color && last.bg == bg => last.end = b as u32,
            _ => {
                let mut sp = Span::new(a, b);
                sp.color = color;
                sp.bg = bg;
                out.push(sp);
            }
        }
    }
    out
}

/// `code` as drawn: text layout draws a tab as one space, so each tab is followed by spaces up
/// to the next four-column stop; and when `wrap`, a zero-width space after `.`, `(`, `[` and
/// `/` lets a long call chain or path break there. Returns the text, `spans` moved to match,
/// and the inserted ranges (drawn, never copied).
fn tabbed(code: &str, mut spans: Vec<Span>, wrap: bool) -> (String, Vec<Span>, Vec<Range<usize>>) {
    if !code.contains('\t') && !(wrap && code.contains(['.', '(', '[', '/'])) {
        return (code.to_owned(), spans, Vec::new());
    }
    let mut shown = String::with_capacity(code.len() + 8);
    // (byte in `code` after the insertion, bytes added up to and including it)
    let mut shifts: Vec<(usize, usize)> = Vec::new();
    let mut pads = Vec::new();
    let mut col = 0;
    let mut chars = code.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        shown.push(ch);
        col += 1;
        let start = shown.len();
        if ch == '\t' {
            let pad = 3 - (col - 1) % 4;
            shown.extend(std::iter::repeat_n(' ', pad));
            col += pad;
        } else if wrap && matches!(ch, '.' | '(' | '[' | '/') && chars.peek().is_some_and(|(_, c)| !c.is_whitespace()) {
            shown.push('\u{200B}');
        }
        if shown.len() > start {
            pads.push(start..shown.len());
            shifts.push((i + ch.len_utf8(), shifts.last().map_or(0, |s| s.1) + shown.len() - start));
        }
    }
    let shift = |b: u32| match shifts.partition_point(|s| s.0 <= b as usize) {
        0 => b,
        n => b + shifts[n - 1].1 as u32,
    };
    for sp in &mut spans {
        (sp.start, sp.end) = (shift(sp.start), shift(sp.end));
    }
    (shown, spans, pads)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::single_range_in_vec_init)]
    fn word_backgrounds_cut_syntax_colours() {
        let red = Color::WHITE;
        let green = Color::BLACK;
        // "let total = 1;" with `let` and `1` coloured, `total` changed.
        let spans = layer(14, &[(0..3, red), (12..13, red)], &[4..9], Some(green));
        let got: Vec<(u32, u32, Option<Color>, Option<Color>)> = spans.iter().map(|s| (s.start, s.end, s.color, s.bg)).collect();
        assert_eq!(got, vec![(0, 3, Some(red), None), (4, 9, None, Some(green)), (12, 13, Some(red), None)]);
        // A change across a coloured token splits it.
        let spans = layer(10, &[(0..6, red)], &[3..8], Some(green));
        let got: Vec<(u32, u32, Option<Color>, Option<Color>)> = spans.iter().map(|s| (s.start, s.end, s.color, s.bg)).collect();
        assert_eq!(got, vec![(0, 3, Some(red), None), (3, 6, Some(red), Some(green)), (6, 8, None, Some(green))]);
    }

    #[test]
    fn tabs_pad_to_four_columns_without_copying_the_padding() {
        let (shown, spans, pads) = tabbed("a\tb\t\tc", vec![Span::new(2, 3)], false);
        assert_eq!(shown, "a\t  b\t  \t   c");
        assert_eq!(pads, vec![2..4, 6..8, 9..12]);
        assert_eq!(&shown[spans[0].start as usize..spans[0].end as usize], "b");
        let copied: String = shown.char_indices().filter(|(i, _)| !pads.iter().any(|r| r.contains(i))).map(|(_, c)| c).collect();
        assert_eq!(copied, "a\tb\t\tc");
    }

    #[test]
    fn wrapped_lines_can_break_inside_call_chains() {
        let (shown, spans, pads) = tabbed("a.b(c) x/ y", vec![Span::new(2, 3)], true);
        assert_eq!(shown, "a.\u{200B}b(\u{200B}c) x/ y");
        assert_eq!(&shown[spans[0].start as usize..spans[0].end as usize], "b");
        let copied: String = shown.char_indices().filter(|(i, _)| !pads.iter().any(|r| r.contains(i))).map(|(_, c)| c).collect();
        assert_eq!(copied, "a.b(c) x/ y");
    }
}
