//! Drawing a [`DiffState`]: a virtual list of file headers, hunk separators and lines, a
//! sticky header for the file being read, and the handlers that feed selection, expansion and
//! the reading position back into the state.

use std::any::Any;
use std::borrow::Cow;
use std::ops::Range;
use std::rc::Rc;

use rune::{Color, Cursor, Drag, El, GrowthSpec, Icon, Phase, ScrollSpec, Tx, col, icon, memo, rich, row, text};

use super::model::{LineKind, Side};
use super::state::{Annotation, DiffMode, DiffState, Expand, HEADER_H, LineRange, LineRef, Row};
use crate::{Kit, ScrollBar, ScrollBarStyle};

mod chrome;
mod labels;
mod line_info;
mod look;
mod spans;

pub use labels::DiffLabel;
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
type AnnotateFn<S> = Rc<dyn Fn(&S, &Annotation) -> El>;
type FileHeaderFn<S> = Rc<dyn Fn(&S, usize, bool) -> El>;
type HeaderFn<S> = Rc<dyn Fn(&S, usize) -> El>;
type SelectFn<S> = Rc<dyn Fn(&mut S, Option<LineRange>, &mut Tx)>;
type CommentFn<S> = Rc<dyn Fn(&mut S, LineRange, &mut Tx)>;
type CommentFilterFn = Rc<dyn Fn(usize, LineRef) -> bool>;
type VisibleFileFn<S> = Rc<dyn Fn(&mut S, usize)>;

#[derive(Clone, Copy)]
struct GutterDrag {
    file: usize,
    at: LineRef,
    side: Side,
    unified: bool,
    row_at: u32,
}

trait DiffBind {
    fn has_comment(&self) -> bool;
    fn can_comment(&self, f: usize, at: LineRef) -> bool;
    fn has_annotation(&self) -> bool;
    fn file_header(&self, s: &dyn Any, f: usize, collapsed: bool) -> Option<El>;
    fn header_action(&self, s: &dyn Any, f: usize) -> Option<El>;
    fn annotation(&self, s: &dyn Any, a: &Annotation) -> Option<El>;
    fn bind_list(&self, list: El, count: usize, view: Rc<View>) -> El;
    fn bind_number_drag(&self, el: El, drag: GutterDrag) -> El;
    fn bind_plus(&self, el: El, f: usize, at: LineRef) -> El;
    fn bind_code_hover(&self, el: El, side: Side, cell: String) -> El;
    fn bind_expand(&self, el: El, f: usize, gap: usize, how: Expand, shift_all: bool) -> El;
    fn bind_expand_both(&self, el: El, f: usize, gap: usize, first: bool, last: bool) -> El;
    fn bind_toggle_file(&self, el: El, f: usize, sticky: bool) -> El;
}

struct DiffHandlers<S> {
    get: Get<S>,
    get_mut: GetMut<S>,
    annotation: Option<AnnotateFn<S>>,
    file_header: Option<FileHeaderFn<S>>,
    header: Option<HeaderFn<S>>,
    on_select: Option<SelectFn<S>>,
    on_comment: Option<CommentFn<S>>,
    comment_filter: Option<CommentFilterFn>,
    on_visible_file: Option<VisibleFileFn<S>>,
}

impl<S: 'static> DiffBind for DiffHandlers<S> {
    fn has_comment(&self) -> bool { self.on_comment.is_some() }
    fn can_comment(&self, f: usize, at: LineRef) -> bool {
        self.on_comment.is_some() && self.comment_filter.as_ref().is_none_or(|allow| allow(f, at))
    }
    fn has_annotation(&self) -> bool { self.annotation.is_some() }

    fn file_header(&self, s: &dyn Any, f: usize, collapsed: bool) -> Option<El> {
        let s = s.downcast_ref::<S>().expect("diff view state type mismatch");
        self.file_header.as_ref().map(|h| h(s, f, collapsed))
    }

    fn header_action(&self, s: &dyn Any, f: usize) -> Option<El> {
        let s = s.downcast_ref::<S>().expect("diff view state type mismatch");
        self.header.as_ref().map(|h| h(s, f))
    }

    fn annotation(&self, s: &dyn Any, a: &Annotation) -> Option<El> {
        let s = s.downcast_ref::<S>().expect("diff view state type mismatch");
        self.annotation.as_ref().map(|build| build(s, a))
    }

    fn bind_list(&self, list: El, count: usize, view: Rc<View>) -> El {
        let (get, row_get, vis_mut, on_file) = (self.get.clone(), self.get.clone(), self.get_mut.clone(), self.on_visible_file.clone());
        list.virtual_rows(count, view.look.line_h(), keys(get), move |s: &S, i| view.row(s, row_get(s), i))
            .on_visible_rows(move |s: &mut S, rows| {
                let (reported, sticky_changed) = vis_mut(s).set_visible_rows(rows);
                if let (Some(f), Some(cb)) = (reported, &on_file) {
                    cb(s, f);
                    return true;
                }
                sticky_changed
            })
    }

    fn bind_number_drag(&self, el: El, drag: GutterDrag) -> El {
        let (get, on_select) = (self.get_mut.clone(), self.on_select.clone());
        el.on_drag(move |s: &mut S, d: &mut Drag, tx| {
            if let Some(sel) = number_drag(get(s), d, tx, drag)
                && let Some(cb) = &on_select
            {
                cb(s, sel, tx);
            }
        })
    }

    fn bind_plus(&self, el: El, f: usize, at: LineRef) -> El {
        let (get, cb, filter) = (self.get_mut.clone(), self.on_comment.clone(), self.comment_filter.clone());
        el.on_press_tx(move |s: &mut S, tx| {
            if filter.as_ref().is_some_and(|allow| !allow(f, at)) {
                return;
            }
            let lines = comment_lines(get(s), f, at);
            if let Some(cb) = &cb {
                cb(s, lines, tx);
            }
        })
    }

    fn bind_code_hover(&self, el: El, side: Side, cell: String) -> El {
        let get = self.get_mut.clone();
        el.on_hover_change(move |s: &mut S, inside, tx: &mut Tx| {
            if !get(s).hover_text(side, &cell, inside) { tx.live_only(); }
        })
    }

    fn bind_expand(&self, el: El, f: usize, gap: usize, how: Expand, shift_all: bool) -> El {
        let get = self.get_mut.clone();
        el.on_press_tx(move |s: &mut S, tx: &mut Tx| {
            let how = if shift_all && tx.mods().shift { Expand::All } else { how };
            get(s).expand(f, gap, how);
        })
    }

    fn bind_expand_both(&self, el: El, f: usize, gap: usize, first: bool, last: bool) -> El {
        let get = self.get_mut.clone();
        el.on_press_tx(move |s: &mut S, _| expand_ends(get(s), f, gap, first, last))
    }

    fn bind_toggle_file(&self, el: El, f: usize, sticky: bool) -> El {
        let get = self.get_mut.clone();
        el.on_press_tx(move |s: &mut S, tx| toggle_file(get(s), f, sticky, tx))
    }
}

#[derive(Clone, Copy, Default)]
enum DiffScrollBar {
    #[default]
    None,
    Kit,
    Styled(ScrollBarStyle),
}

/// Draws the [`DiffState`] an app keeps in its state: give it the state's accessors and build
/// it in the view. It fills the height it's given and scrolls; Cmd-C, `j`/`k` and `]`/`[` work
/// once the app routes keys to [`DiffState::key`].
///
/// ```
/// # use rune::{El, el};
/// # use rune_kit::{Kit, diff::{Annotation, DiffState, DiffView, LineRange}};
/// # struct FileTree;
/// # impl FileTree { fn reveal(&mut self, _: usize) {} }
/// # struct App { diff: DiffState, tree: FileTree }
/// # impl App { fn compose(&mut self, _: LineRange) {} }
/// # fn comment(_: &App, _: &str) -> El { el() }
/// # let s = &App { diff: DiffState::default(), tree: FileTree };
/// # let k = Kit::neutral(true);
/// # let _ =
/// DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
///     .annotation(|s: &App, a: &Annotation| comment(s, &a.id))
///     .on_comment(|s: &mut App, lines, _| s.compose(lines))
///     .on_visible_file(|s: &mut App, f| s.tree.reveal(f))
///     .build(s, &k)
/// # ;
/// ```
pub struct DiffView<S> {
    key: String,
    fold_row: FoldRow,
    look: look::LookSpec,
    labels: Option<labels::LabelsFn>,
    scroll_bar: DiffScrollBar,
    handlers: DiffHandlers<S>,
}

impl<S: 'static> DiffView<S> {
    pub fn new(get: impl Fn(&S) -> &DiffState + 'static, get_mut: impl Fn(&mut S) -> &mut DiffState + 'static) -> DiffView<S> {
        DiffView {
            key: "diff".into(),
            fold_row: FoldRow::default(),
            look: look::LookSpec::default(),
            labels: None,
            scroll_bar: DiffScrollBar::None,
            handlers: DiffHandlers {
                get: Rc::new(get),
                get_mut: Rc::new(get_mut),
                annotation: None,
                file_header: None,
                header: None,
                on_select: None,
                on_comment: None,
                comment_filter: None,
                on_visible_file: None,
            },
        }
    }

    /// The view's element key (`"diff"` by default); give each diff in a window its own.
    pub fn key(mut self, key: impl Into<String>) -> Self { self.key = key.into(); self }

    /// How folded unchanged lines draw: [`FoldRow::Summary`] by default, or Pierre's
    /// [`FoldRow::LineInfo`] as OpenCode Desktop's review has them.
    pub fn fold_row(mut self, fold_row: FoldRow) -> Self { self.fold_row = fold_row; self }

    /// Localise or rename every visible and accessible string `DiffView` emits.
    ///
    /// Returning `None` (or an empty or whitespace-only string) keeps the default English text
    /// for that [`DiffLabel`].
    ///
    /// ```
    /// # use rune_kit::{Kit, diff::{DiffLabel, DiffState, DiffView}};
    /// # struct App { diff: DiffState }
    /// # let s = &App { diff: DiffState::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
    ///     .labels(|label| match label {
    ///         DiffLabel::ExpandFile => Some("Déplier le fichier".into()),
    ///         DiffLabel::CollapseFile => Some("Replier le fichier".into()),
    ///         DiffLabel::CommentLine => Some("Commenter cette ligne".into()),
    ///         _ => None,
    ///     })
    ///     .build(s, &k)
    /// # ;
    /// ```
    pub fn labels(mut self, f: impl Fn(DiffLabel<'_>) -> Option<String> + 'static) -> Self {
        self.labels = Some(Rc::new(f));
        self
    }

    /// Build each [`Annotation`] under its line. Rows holding annotations are built every
    /// commit, so a widget can read app state (a composer's text) freely.
    pub fn annotation(mut self, f: impl Fn(&S, &Annotation) -> El + 'static) -> Self { self.handlers.annotation = Some(Rc::new(f)); self }

    /// Replace the inner built-in file header for each file (called with the app state, the
    /// file's index in [`DiffState::files`], and whether it is currently collapsed).
    ///
    /// `DiffView` keeps the 44-point header row, its heading accessible label, horizontal
    /// left-pinning in unwrapped mode, and the sticky top header overlay. Collapse files from
    /// the custom header via [`DiffState::set_collapsed`]. When set, [`Self::header_actions`]
    /// is ignored.
    ///
    /// ```
    /// # use rune::{row, text};
    /// # use rune_kit::{Kit, diff::{DiffState, DiffView}};
    /// # struct App { diff: DiffState }
    /// # let s = &App { diff: DiffState::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
    ///     .file_header(|s: &App, file, collapsed| {
    ///         let path = s.diff.files()[file].path().to_owned();
    ///         row().w_full().h_full().px(10.0).items_center()
    ///             .on_press(move |s: &mut App| s.diff.set_collapsed(file, !collapsed))
    ///             .child(text(path))
    ///     })
    ///     .build(s, &k)
    /// # ;
    /// ```
    pub fn file_header(mut self, f: impl Fn(&S, usize, bool) -> El + 'static) -> Self {
        self.handlers.file_header = Some(Rc::new(f));
        self
    }

    /// Extra controls at the end of each file's header (a "viewed" toggle, a menu), by file.
    pub fn header_actions(mut self, f: impl Fn(&S, usize) -> El + 'static) -> Self { self.handlers.header = Some(Rc::new(f)); self }

    /// Called when a click, shift-click or drag on the line numbers ends, with the selection.
    pub fn on_select(mut self, f: impl Fn(&mut S, Option<LineRange>, &mut Tx) + 'static) -> Self { self.handlers.on_select = Some(Rc::new(f)); self }

    /// Show a "+" by the line numbers of the hovered line, and call `f` with the lines it
    /// stands for when pressed: the selection when the line is in it, else that line.
    pub fn on_comment(mut self, f: impl Fn(&mut S, LineRange, &mut Tx) + 'static) -> Self { self.handlers.on_comment = Some(Rc::new(f)); self }

    /// Restrict which lines show the "+" comment button (when [`Self::on_comment`] is set) and
    /// can invoke it. `file` is the index in [`DiffState::files`] and `at` is the candidate
    /// line. Both sides are enabled by default.
    ///
    /// In [`DiffMode::Unified`], unchanged context lines carry both an old and a new line number;
    /// the new side is tried first and the old side is used as a fallback so old-side-only filters
    /// work on context lines too.
    ///
    /// ```
    /// # use rune_kit::{Kit, diff::{DiffState, DiffView, LineRange, Side}};
    /// # struct App { diff: DiffState }
    /// # impl App { fn compose(&mut self, _: LineRange) {} }
    /// # let s = &App { diff: DiffState::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
    ///     .on_comment(|s: &mut App, lines, _| s.compose(lines))
    ///     .comment_filter(|_, at| at.side == Side::New)
    ///     .build(s, &k)
    /// # ;
    /// ```
    pub fn comment_filter(mut self, f: impl Fn(usize, LineRef) -> bool + 'static) -> Self {
        self.handlers.comment_filter = Some(Rc::new(f));
        self
    }

    /// Called when the file at the top of the view changes as it scrolls, to follow it in a
    /// file tree.
    pub fn on_visible_file(mut self, f: impl Fn(&mut S, usize) + 'static) -> Self { self.handlers.on_visible_file = Some(Rc::new(f)); self }

    /// Draws kit scroll bars on the diff's viewport using [`ScrollBarStyle::from_kit`] when `on`
    /// is true, following [`rune::system_scroll_bars`]. In unwrapped unified mode, both vertical
    /// and horizontal bars are attached; when lines wrap (or in [`DiffMode::Split`]), only a
    /// vertical bar is attached. Off by default.
    ///
    /// ```
    /// # use rune_kit::{Kit, diff::{DiffState, DiffView}};
    /// # struct App { diff: DiffState }
    /// # let s = &App { diff: DiffState::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
    ///     .scroll_bar(true)
    ///     .build(s, &k)
    /// # ;
    /// ```
    pub fn scroll_bar(mut self, on: bool) -> Self {
        self.scroll_bar = if on { DiffScrollBar::Kit } else { DiffScrollBar::None };
        self
    }

    /// Draws scroll bars on the diff's viewport in `style`, following
    /// [`rune::system_scroll_bars`]. In unwrapped unified mode, the vertical bar uses `style`
    /// and the horizontal bar uses `style.horizontal()`; when lines wrap (or in
    /// [`DiffMode::Split`]), only the vertical bar is attached.
    ///
    /// ```
    /// # use rune_kit::{Kit, ScrollBarStyle, diff::{DiffState, DiffView}};
    /// # struct App { diff: DiffState }
    /// # let s = &App { diff: DiffState::default() };
    /// # let k = Kit::neutral(true);
    /// # let _ =
    /// DiffView::new(|s: &App| &s.diff, |s: &mut App| &mut s.diff)
    ///     .scroll_bar_styled(ScrollBarStyle::scroll_view(&k))
    ///     .build(s, &k)
    /// # ;
    /// ```
    pub fn scroll_bar_styled(mut self, style: ScrollBarStyle) -> Self {
        self.scroll_bar = DiffScrollBar::Styled(style);
        self
    }

    pub fn build(self, s: &S, k: &Kit) -> El {
        let st = (self.handlers.get)(s);
        let labels_version = labels::fingerprint(self.labels.as_ref());
        let scroll_bar = match self.scroll_bar {
            DiffScrollBar::None => None,
            DiffScrollBar::Kit => Some(ScrollBarStyle::from_kit(k)),
            DiffScrollBar::Styled(style) => Some(style),
        };
        let view = Rc::new(View {
            look: Look::new(k, self.look),
            fold_row: self.fold_row,
            labels: self.labels,
            labels_version,
            scroll_bar,
            bind: Box::new(self.handlers),
        });
        view.build_root(st, self.key, s)
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

}

struct View {
    look: Look,
    fold_row: FoldRow,
    labels: Option<labels::LabelsFn>,
    labels_version: u64,
    scroll_bar: Option<ScrollBarStyle>,
    bind: Box<dyn DiffBind>,
}

impl View {
    pub(super) fn label(&self, label: DiffLabel<'_>) -> String {
        labels::resolve(self.labels.as_ref(), label)
    }

    fn cell_label(&self, c: &Cell<'_>) -> String {
        let (_, number) = c.source();
        self.label(DiffLabel::Line { kind: c.kind, number, text: c.text })
    }

    fn comment_sides(&self, st: &DiffState, r: Row) -> (bool, bool) {
        if !self.bind.has_comment() {
            return (false, false);
        }
        let f = r.file();
        let can = |side: Side| st.row_line(r, side).is_some_and(|n| self.bind.can_comment(f, LineRef::new(side, n)));
        (can(Side::Old), can(Side::New))
    }

    fn unified_comment_target(&self, f: usize, c: Cell<'_>) -> Option<LineRef> {
        if !self.bind.has_comment() {
            return None;
        }
        let new = c.new.map(|n| LineRef::new(Side::New, n)).filter(|&at| self.bind.can_comment(f, at));
        new.or_else(|| c.old.map(|n| LineRef::new(Side::Old, n)).filter(|&at| self.bind.can_comment(f, at)))
    }

    fn build_root(self: &Rc<Self>, st: &DiffState, key: String, s: &dyn Any) -> El {
        let wrap = st.wraps();
        let scroll_path = if self.scroll_bar.is_some() { format!("{key}/scroll-scroll-area/scroll") } else { format!("{key}/scroll") };
        if !wrap && let Some(r) = rune::memo::layout(&scroll_path) {
            st.set_viewport(r);
        }
        st.set_line_height(self.look.line_h());
        let count = st.rows.len();
        let mut list = col().key("rows").w_full().growth(GrowthSpec::default()).selection_group(true).role("list").label(self.label(DiffLabel::List));
        if !wrap {
            list = list.min_w(self.content_width(st).max(st.view_width()));
        }
        let list = self.bind.bind_list(list, count, self.clone()).virtual_rows_version(st.keys_rev);
        let scroll = col().key("scroll").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false });
        let scroll = if wrap { scroll } else { scroll.scroll_xy() };
        let scroll = scroll.child(list);
        let scroll = match self.scroll_bar {
            None => scroll,
            Some(style) => {
                // Keep the vertical track below the 44 pt sticky file header unless the caller
                // set a custom top offset, so the sticky overlay never covers the top of the thumb.
                let v = if style.top_offset() == 0.0 { style.track_offset(HEADER_H, style.end_offset()) } else { style };
                if wrap { scroll.scroll_bar_styled(v) } else { scroll.scroll_bars_xy_styled(v, style.horizontal()) }
            }
        };
        // Code reads left to right in any window: gutters at the left, old before new.
        let mut root = col().key(key).dir(rune::Direction::Ltr).w_full().grow(1.0).min_h(0.0).fill(self.look.bg).child(scroll);
        if let Some(f) = st.sticky() {
            root = root.child(self.file_header(s, st, f, true).key("sticky").absolute(0.0, 0.0).w_full());
        }
        root
    }

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

    fn row(&self, s: &dyn Any, st: &DiffState, i: usize) -> El {
        let r = st.rows[i];
        let key = st.keys[i].as_str();
        match r {
            Row::File(f) => self.file_header(s, st, f as usize, false),
            Row::Note(f) => memo(key, (self.look.version, st.rev, st.view_width().to_bits(), self.labels_version), || self.note(st, f as usize)),
            Row::Gap { f, gap } => memo(key, (self.look.version, st.rev, st.view_width().to_bits(), self.fold_row, self.labels_version), || match self.fold_row {
                FoldRow::Summary => self.gap(st, f as usize, gap as usize),
                FoldRow::LineInfo => self.line_info(st, f as usize, gap as usize),
            }),
            _ if !st.annotations().is_empty() && self.bind.has_annotation() && self.annotated(st, r) => self.line(s, st, i),
            _ => {
                let version = (
                    self.look.version,
                    st.rev,
                    st.row_selected(i, None),
                    st.copy_side(),
                    st.selection().map(|s| s.start.side),
                    self.comment_sides(st, r),
                    self.labels_version,
                );
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

    fn line(&self, s: &dyn Any, st: &DiffState, i: usize) -> El {
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
        if !self.bind.has_annotation() {
            return line;
        }
        let notes = |side: Side| -> Vec<El> {
            st.row_line(r, side).map_or(Vec::new(), |n| {
                st.annotations_at(f, LineRef::new(side, n))
                    .filter_map(|a| self.bind.annotation(s, a).map(|el| note(el, a)))
                    .collect()
            })
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
                out = out.child(c.children(all.filter_map(|a| self.bind.annotation(s, a).map(|el| note(el, a)))));
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
        if let Some(at) = self.unified_comment_target(f, c) {
            gutter = gutter.child(self.plus(f, at).absolute(BAR + 2.0 * l.number_w(digits) - 10.0, l.plus_y()));
        }
        if !st.wraps() {
            gutter = gutter.sticky_left(0.0);
        }
        l.apply_hover_motion(
            row()
                .w_full()
                .min_h(l.line_h())
                .items_start()
                .fill(fill)
                .hover(l.hover)
                .hover_group()
                .role("listitem")
                .label(self.cell_label(&c))
                .child(gutter)
                .child(self.code(st, f, &c, copy, hover_side, &st.keys[i]).key("code")),
        )
    }

    fn split<'a>(&self, st: &'a DiffState, i: usize, f: usize, old: Option<Cell<'a>>, new: Option<Cell<'a>>) -> El {
        let digits = st.digits(f);
        let single = st.single_column(f);
        let half = |side: Side, c: Option<Cell<'a>>| -> El {
            let l = &self.look;
            let key = if side == Side::Old { "old" } else { "new" };
            let mut h = row().key(key).w_frac(if single.is_some() { 1.0 } else { 0.5 }).min_w(0.0).min_h(l.line_h()).items_start().self_stretch();
            let Some(c) = c else {
                return h.fill(l.empty_bg);
            };
            // An unchanged line shows on both sides as context.
            let kind = c.kind;
            let selected = st.row_selected(i, Some(side));
            let fill = if selected { l.selected } else { l.tints(kind).map_or(l.bg, |t| t.line) };
            let n = if side == Side::Old { c.old } else { c.new };
            let mut cell = c;
            if kind == LineKind::Context
                && let Some(line) = n
            {
                cell.text = st.side_line(f, side, line).unwrap_or(c.text);
            }
            let copy = st.copy_side() == side;
            let number = n.unwrap_or_else(|| cell.source().1);
            h = l.apply_hover_motion(h.fill(fill).hover(l.hover).hover_group())
                .role("listitem")
                .label(self.label(DiffLabel::Line { kind: cell.kind, number, text: cell.text }))
                .child(self.bar(kind))
                .child(self.number(st, i, f, n, side, kind, selected, digits).key("n"))
                .child(self.code(st, f, &cell, copy, side, &st.keys[i]).key("code"));
            if let Some(at) = n.map(|n| LineRef::new(side, n)).filter(|&at| self.bind.can_comment(f, at)) {
                h = h.child(self.plus(f, at).absolute(BAR + l.number_w(digits) - 10.0, l.plus_y()));
            }
            h
        };
        let line = row().w_full().min_h(self.look.line_h()).items_stretch();
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
        let mut c = row().w(l.number_w(digits)).min_h(l.line_h()).self_stretch().px(8.0).justify_end().items_start().a11y_hidden();
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
        self.bind.bind_number_drag(c.cursor(Cursor::Pointer), GutterDrag { file: f, at, side, unified, row_at: i as u32 })
    }

    /// The "+" that appears by the numbers of the hovered line.
    fn plus(&self, f: usize, at: LineRef) -> El {
        let l = &self.look;
        let button = row()
            .key("plus")
            .size(16.0, 16.0)
            .center()
            .radius(4.0)
            .fill(l.accent)
            .group_opacity(0.0, 1.0)
            .cursor(Cursor::Pointer)
            .role("button")
            .label(self.label(DiffLabel::CommentLine))
            .child(icon(&PLUS, 12.0).color(l.on_accent));
        self.bind.bind_plus(button, f, at)
    }

    /// The code of a line: syntax colours under word changes, tabs padded to four columns.
    fn code(&self, st: &DiffState, f: usize, c: &Cell<'_>, copy: bool, side: Side, row_key: &str) -> El {
        let l = &self.look;
        let (src, n) = match (side, c.old, c.new) {
            (Side::Old, Some(n), _) => (Side::Old, n),
            (Side::New, _, Some(n)) => (Side::New, n),
            _ => c.source(),
        };
        let syntax: Vec<(Range<usize>, Color)> = st
            .highlights_for(f, src, n, c.text)
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
        let mut code = self.bind.bind_code_hover(
            col().grow(1.0).min_w(0.0).pl(CODE_PAD.0).pr(CODE_PAD.1).child(t),
            side,
            format!("{row_key}.{}", side as u8),
        );
        if c.no_newline {
            code = code.child(text(self.label(DiffLabel::NoNewlineAtEndOfFile)).font(l.small).color(l.muted).opacity(0.6).key("eof").pb(2.0));
        }
        code
    }

    fn gutter_w(&self, st: &DiffState, f: usize) -> f32 {
        let n = self.look.number_w(st.digits(f));
        BAR + if st.mode() == DiffMode::Unified { 2.0 * n } else { n }
    }
}

fn number_drag(st: &mut DiffState, d: &Drag, tx: &Tx, drag: GutterDrag) -> Option<Option<LineRange>> {
    let GutterDrag { file: f, at, side, unified, row_at } = drag;
    match d.phase {
        Phase::Down => {
            st.press_line(f, at, tx.mods().shift);
            None
        }
        Phase::Move => {
            let Some(r) = st.row_from(row_at, d.y) else { return None };
            let r = st.rows[r as usize];
            if r.file() != f {
                return None;
            }
            // Unified rows hold one side or both: keep to the pressed side where it can.
            let to = st.row_line(r, side).map(|n| LineRef::new(side, n)).or_else(|| {
                let other = if side == Side::Old { Side::New } else { Side::Old };
                unified.then(|| st.row_line(r, other).map(|n| LineRef::new(other, n))).flatten()
            });
            if let Some(to) = to {
                st.drag_to(f, to);
            }
            None
        }
        Phase::Up => {
            if d.cancelled() {
                st.cancel_line();
                return None;
            }
            Some(st.release_line())
        }
    }
}

fn comment_lines(st: &DiffState, f: usize, at: LineRef) -> LineRange {
    match st.selection() {
        Some(sel) if sel.file == f && st.contains(&sel, at) => sel,
        _ => LineRange::line(f, at),
    }
}

fn expand_ends(st: &mut DiffState, f: usize, gap: usize, first: bool, last: bool) {
    let ends: &[Expand] = match (first, last) {
        (true, _) => &[Expand::Up],
        (_, true) => &[Expand::Down],
        _ => &[Expand::Down, Expand::Up],
    };
    for &how in ends {
        st.expand(f, gap, how);
    }
}

fn toggle_file(st: &mut DiffState, f: usize, sticky: bool, tx: &mut Tx) {
    st.set_collapsed(f, !st.collapsed(f));
    if sticky {
        st.scroll_to_file(f, tx);
    }
}

/// An annotation's widget, keyed by its id so it keeps its state as rows come and go.
fn note(el: El, a: &Annotation) -> El {
    col().key(format!("a.{}", a.id)).w_full().child(el)
}
