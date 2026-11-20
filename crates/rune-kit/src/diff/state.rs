//! What a diff view shows and where: the rows files, hunks and expanded ranges make, the
//! selection, annotations, the side text selection copies, and the reading position.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::copy::CopySide;
use super::model::{FileDiff, Side};
use crate::line_selection::{Column, LineEvent, LinePoint, LineSelection, Lines};
use crate::syntax::Highlighter;

mod layout;
mod nav;

use layout::{Chunk, gaps};

/// How a diff lays out its two sides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DiffMode {
    /// One column, removed lines above the added lines that replace them.
    #[default]
    Unified,
    /// Old on the left, new on the right, changed lines side by side. An added or deleted
    /// file shows its one side as a single column.
    Split,
}

/// A line of one side of a file: what annotations and selections point at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineRef {
    pub side: Side,
    /// 1-based.
    pub line: u32,
}

impl LineRef {
    pub fn new(side: Side, line: u32) -> LineRef {
        LineRef { side, line }
    }
}

/// Selected lines of one file, from where the selection started to where it ends (either
/// way round), as a click, shift-click or drag down the line numbers makes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineRange {
    pub file: usize,
    pub start: LineRef,
    pub end: LineRef,
}

impl LineRange {
    /// One line.
    pub fn line(file: usize, at: LineRef) -> LineRange {
        LineRange { file, start: at, end: at }
    }

    /// The range as the line-selection gesture keeps it.
    pub fn lines(self) -> Lines<DiffLine> {
        Lines { start: DiffLine { file: self.file, at: self.start }, end: DiffLine { file: self.file, at: self.end } }
    }
}

impl From<Lines<DiffLine>> for LineRange {
    fn from(l: Lines<DiffLine>) -> LineRange {
        LineRange { file: l.start.file, start: l.start.at, end: l.end.at }
    }
}

/// A line of one file of a diff: what the line-selection gesture runs between. A selection
/// stays in one file and may cross from one side to the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiffLine {
    pub file: usize,
    pub at: LineRef,
}

impl LinePoint for DiffLine {
    fn joins(&self, other: &DiffLine) -> bool {
        self.file == other.file
    }
}

/// A widget the app shows under a line, such as a review comment or a composer. The app
/// builds it from `id` (see [`DiffView::annotation`](super::DiffView::annotation)).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Annotation {
    pub file: usize,
    pub at: LineRef,
    pub id: String,
}

impl Annotation {
    pub fn new(file: usize, at: LineRef, id: impl Into<String>) -> Annotation {
        Annotation { file, at, id: id.into() }
    }
}

/// Which way an unchanged range between hunks opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expand {
    /// Twenty lines up from the hunk below it.
    Up,
    /// Twenty lines down from the hunk above it.
    Down,
    /// All of it.
    All,
}

/// Lines one expand step reveals.
pub const EXPAND_STEP: u32 = 20;

/// Row heights in points. Lines are fixed unless they wrap or carry annotations.
pub(crate) const LINE_H: f32 = 20.0;
pub(crate) const HEADER_H: f32 = 44.0;
pub(crate) const GAP_H: f32 = 32.0;
pub(crate) const NOTE_H: f32 = 40.0;

/// One row of the virtual list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// A file's header.
    File(u32),
    /// A file with no lines to show: binary, or only renamed or re-moded.
    Note(u32),
    /// The unchanged range before hunk `gap` (the last one: after the last hunk), with its
    /// expand buttons and the hunk's header.
    Gap { f: u32, gap: u32 },
    /// An unchanged line revealed from a range, by its old and new numbers.
    Ctx { f: u32, old: u32, new: u32 },
    /// Line `i` of hunk `h`. In split mode, one without a partner on the other side.
    Line { f: u32, h: u32, i: u32 },
    /// Split mode: removed line `del` beside added line `add`, both of hunk `h`.
    Pair { f: u32, h: u32, del: u32, add: u32 },
}

impl Row {
    pub(crate) fn file(self) -> usize {
        match self {
            Row::File(f) | Row::Note(f) | Row::Gap { f, .. } | Row::Ctx { f, .. } | Row::Line { f, .. } | Row::Pair { f, .. } => f as usize,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct FileUi {
    collapsed: bool,
    /// Lines revealed at the top and bottom of each unchanged range (`hunks.len() + 1` of them).
    revealed: Vec<(u32, u32)>,
}

/// An unchanged range before a hunk: its first old and new line numbers and its length
/// (`None` when unknown: after the last hunk without the whole text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Gap {
    pub(crate) old: u32,
    pub(crate) new: u32,
    pub(crate) len: Option<u32>,
}

/// A diff and everything about how it is shown: the app keeps one in its state and
/// [`DiffView`](super::DiffView) draws it.
pub struct DiffState {
    files: Vec<FileDiff>,
    ui: Vec<FileUi>,
    mode: DiffMode,
    wrap: bool,
    annotations: Vec<Annotation>,
    by_line: HashMap<(u32, Side, u32), Vec<u32>>,
    /// The selected lines and the gesture on the line numbers making them.
    lines: LineSelection<DiffLine>,
    /// The side text selection copies, which follows the pointer.
    copy: CopySide,
    highlighter: Highlighter,
    chunks: RefCell<HashMap<(u32, Side, u32), Rc<Chunk>>>,
    /// Per file, per hunk, per side: the index in the hunk of each of its lines on that side.
    side_lines: Vec<Vec<[Vec<u32>; 2]>>,
    gaps: Vec<Vec<Gap>>,
    /// Per file, the digits its line numbers take (at least three).
    digits: Vec<u32>,
    /// The widest line of any file, in columns.
    columns: u32,
    pub(crate) rows: Vec<Row>,
    pub(crate) keys: Vec<String>,
    row_of: HashMap<(u32, Side, u32), u32>,
    file_rows: Vec<u32>,
    hunk_rows: Vec<u32>,
    /// Bumped whenever the rows change, so built rows rebuild.
    pub(crate) rev: u64,
    /// Changes with `keys`, unique across states so a swapped-in state never matches.
    pub(crate) keys_rev: u64,
    visible_rows: std::ops::Range<usize>,
    view_w: std::cell::Cell<f32>,
    visible: usize,
    /// The visible file last reported to the app.
    reported: Option<usize>,
    nav: Option<u32>,
}

impl std::fmt::Debug for DiffState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiffState").field("files", &self.files.len()).field("rows", &self.rows.len()).field("mode", &self.mode).finish()
    }
}

impl Default for DiffState {
    fn default() -> DiffState {
        DiffState::new(Vec::new())
    }
}

impl DiffState {
    pub fn new(files: Vec<FileDiff>) -> DiffState {
        let mut s = DiffState {
            files: Vec::new(),
            ui: Vec::new(),
            mode: DiffMode::Unified,
            wrap: false,
            annotations: Vec::new(),
            by_line: HashMap::new(),
            lines: LineSelection::default(),
            copy: CopySide::default(),
            highlighter: Highlighter::builtin(),
            chunks: RefCell::new(HashMap::new()),
            side_lines: Vec::new(),
            gaps: Vec::new(),
            digits: Vec::new(),
            columns: 0,
            rows: Vec::new(),
            keys: Vec::new(),
            row_of: HashMap::new(),
            file_rows: Vec::new(),
            hunk_rows: Vec::new(),
            rev: 0,
            keys_rev: 0,
            visible_rows: 0..0,
            view_w: std::cell::Cell::new(0.0),
            visible: 0,
            reported: None,
            nav: None,
        };
        s.set_files(files);
        s
    }

    /// Show `files` instead, every range collapsed again and the selection cleared.
    /// Annotations stay; drop or remap them with [`DiffState::set_annotations`].
    pub fn set_files(&mut self, files: Vec<FileDiff>) {
        self.ui = files.iter().map(|f| FileUi { collapsed: false, revealed: vec![(0, 0); f.hunks.len() + 1] }).collect();
        self.side_lines = files
            .iter()
            .map(|f| {
                f.hunks
                    .iter()
                    .map(|h| {
                        let on = |side: Side| (0..h.lines.len() as u32).filter(|&i| h.lines[i as usize].number(side).is_some()).collect();
                        [on(Side::Old), on(Side::New)]
                    })
                    .collect()
            })
            .collect();
        self.gaps = files.iter().map(gaps).collect();
        self.digits = files.iter().map(|f| f.last_line().max(1).ilog10() + 1).map(|d| d.max(3)).collect();
        self.columns = files.iter().map(FileDiff::columns).max().unwrap_or(0);
        self.files = files;
        self.set_selection(None);
        self.chunks.borrow_mut().clear();
        self.visible = 0;
        self.reported = None;
        self.visible_rows = 0..0;
        self.index_annotations();
        self.rebuild();
    }

    pub fn files(&self) -> &[FileDiff] {
        &self.files
    }

    /// The index of the file at `path` (its new path, or old for a deleted file).
    pub fn file_index(&self, path: &str) -> Option<usize> {
        self.files.iter().position(|f| f.path() == path || f.old_path.as_deref() == Some(path))
    }

    pub fn mode(&self) -> DiffMode {
        self.mode
    }

    /// Switch between unified and split. Rows keep their keys where they can (unchanged and
    /// removed lines), so a switch made in a handler that calls `Tx::motion` springs them to
    /// their new places.
    pub fn set_mode(&mut self, mode: DiffMode) {
        if self.mode != mode {
            self.mode = mode;
            self.rebuild();
        }
    }

    /// In split mode, the one side `file` shows across the whole width, as a single column:
    /// an added file's new side or a deleted file's old side ([`FileDiff::sole_side`]).
    /// `None` in unified mode and for a file with both sides.
    pub fn single_column(&self, file: usize) -> Option<Side> {
        if self.mode != DiffMode::Split {
            return None;
        }
        self.files.get(file)?.sole_side()
    }

    pub fn wrap(&self) -> bool {
        self.wrap
    }

    /// Whether long lines wrap as drawn: split always wraps, each side half the width.
    pub(crate) fn wraps(&self) -> bool {
        self.wrap || self.mode == DiffMode::Split
    }

    /// Wrap long lines (rows grow), or keep them on one line and scroll sideways (the default).
    /// Split wraps either way, as side-by-side review does, so both sides stay in view.
    pub fn set_wrap(&mut self, wrap: bool) {
        if self.wrap != wrap {
            self.wrap = wrap;
            self.rev += 1;
        }
    }

    /// Highlight code with `h` instead of the built-in highlighter.
    pub fn set_highlighter(&mut self, h: Highlighter) {
        self.highlighter = h;
        self.chunks.borrow_mut().clear();
        self.rev += 1;
    }

    pub fn collapsed(&self, file: usize) -> bool {
        self.ui.get(file).is_some_and(|u| u.collapsed)
    }

    /// Show only `file`'s header, or its lines again.
    pub fn set_collapsed(&mut self, file: usize, collapsed: bool) {
        if let Some(u) = self.ui.get_mut(file).filter(|u| u.collapsed != collapsed) {
            u.collapsed = collapsed;
            self.rebuild();
        }
    }

    /// Lines of `file`'s unchanged range before hunk `gap` still hidden, when known.
    pub fn hidden(&self, file: usize, gap: usize) -> Option<u32> {
        let g = self.gaps.get(file)?.get(gap)?;
        let (top, bottom) = self.ui[file].revealed[gap];
        g.len.map(|n| n.saturating_sub(top + bottom))
    }

    /// Reveal more of `file`'s unchanged range before hunk `gap` (`hunks.len()`: after the
    /// last). Does nothing without the file's whole text.
    pub fn expand(&mut self, file: usize, gap: usize, how: Expand) {
        let Some(hidden) = self.hidden(file, gap) else { return };
        if hidden == 0 || !self.files[file].expandable() {
            return;
        }
        let last = gap == self.files[file].hunks.len();
        let r = &mut self.ui[file].revealed[gap];
        match how {
            // Nothing below the first range and nothing above the last to grow from.
            Expand::All => r.0 += hidden,
            Expand::Up if gap > 0 || last => {
                if last { r.0 += hidden.min(EXPAND_STEP) } else { r.1 += hidden.min(EXPAND_STEP) }
            }
            Expand::Up => r.1 += hidden.min(EXPAND_STEP),
            Expand::Down if last => r.0 += hidden.min(EXPAND_STEP),
            Expand::Down => r.0 += hidden.min(EXPAND_STEP),
        }
        self.rebuild();
    }

    pub fn annotations(&self) -> &[Annotation] {
        &self.annotations
    }

    /// Show these annotations under their lines, in order where several share a line.
    pub fn set_annotations(&mut self, annotations: Vec<Annotation>) {
        self.annotations = annotations;
        self.index_annotations();
        self.rev += 1;
    }

    /// The annotations under `file`'s line `at`.
    pub(crate) fn annotations_at(&self, file: usize, at: LineRef) -> impl Iterator<Item = &Annotation> {
        self.by_line.get(&(file as u32, at.side, at.line)).into_iter().flatten().map(|&i| &self.annotations[i as usize])
    }

    fn index_annotations(&mut self) {
        self.by_line.clear();
        for (i, a) in self.annotations.iter().enumerate() {
            self.by_line.entry((a.file as u32, a.at.side, a.at.line)).or_default().push(i as u32);
        }
    }

    pub fn selection(&self) -> Option<LineRange> {
        self.lines.selected().map(LineRange::from)
    }

    /// Select `sel`, ending any press on the line numbers.
    pub fn set_selection(&mut self, sel: Option<LineRange>) {
        let _ = self.lines.step(LineEvent::Set(sel.map(LineRange::lines)), |_| None);
    }

    /// The selected lines' text on the side the selection started on, one per line, without
    /// numbers or markers: what copying the selection gives.
    pub fn selected_text(&self) -> Option<String> {
        let sel = self.selection()?;
        let side = sel.start.side;
        let (a, b) = self.selected_rows(&sel)?;
        let mut out = Vec::new();
        for r in a..=b {
            if let Some(n) = self.row_line(self.rows[r as usize], side) {
                out.push(self.side_line(sel.file, side, n).unwrap_or_default().to_owned());
            }
        }
        Some(out.join("\n"))
    }

    /// The selection's first and last rows.
    fn selected_rows(&self, sel: &LineRange) -> Option<(u32, u32)> {
        let a = *self.row_of.get(&(sel.file as u32, sel.start.side, sel.start.line))?;
        let b = *self.row_of.get(&(sel.file as u32, sel.end.side, sel.end.line))?;
        Some((a.min(b), a.max(b)))
    }

    /// Whether line `at` is in `sel`.
    pub(crate) fn contains(&self, sel: &LineRange, at: LineRef) -> bool {
        let Some((a, b)) = self.selected_rows(sel) else { return false };
        self.row_of.get(&(sel.file as u32, at.side, at.line)).is_some_and(|r| (a..=b).contains(r))
    }

    /// Whether row `r`'s line on `side` is selected.
    pub(crate) fn row_selected(&self, r: usize, side: Option<Side>) -> bool {
        let Some(sel) = self.selection() else { return false };
        let Some((a, b)) = self.selected_rows(&sel) else { return false };
        if self.rows[r].file() != sel.file || !(a as usize..=b as usize).contains(&r) {
            return false;
        }
        side.is_none_or(|s| s == sel.start.side || s == sel.end.side)
    }

    /// A press on line `at`'s number: a new selection, or with `shift` one extended to it.
    /// Pressing the one selected line clears it on release, unless the press drags.
    pub(crate) fn press_line(&mut self, file: usize, at: LineRef, shift: bool) {
        let event = LineEvent::Press { at: DiffLine { file, at }, column: Column::Numbers, shift };
        let row_of = &self.row_of;
        let _ = self.lines.step(event, |p| row_of.get(&(p.file as u32, p.at.side, p.at.line)).copied());
    }

    /// The pointer, dragging from a line number, reached line `at`.
    pub(crate) fn drag_to(&mut self, file: usize, at: LineRef) {
        let _ = self.lines.step(LineEvent::Move { at: DiffLine { file, at } }, |_| None);
    }

    /// The press on the line numbers ended: the selection it leaves.
    pub(crate) fn release_line(&mut self) -> Option<LineRange> {
        let _ = self.lines.step(LineEvent::Release, |_| None);
        self.selection()
    }

    pub(crate) fn cancel_line(&mut self) {
        let _ = self.lines.step(LineEvent::Cancel, |_| None);
    }

    pub(crate) fn copy_side(&self) -> Side {
        self.copy.side()
    }

    /// The pointer entered or left a line's text on `side`: text selection then copies that
    /// side. A held press keeps the cell it started in entered, so dragging across to the
    /// other side doesn't switch.
    /// Notes the pointer entering or leaving a line's code; returns whether that changed the
    /// side a selection copies, the only part the view reads.
    pub(crate) fn hover_text(&mut self, side: Side, cell: &str, inside: bool) -> bool {
        self.copy.step(side, cell, inside)
    }

    /// The digits `file`'s line numbers take.
    pub(crate) fn digits(&self, file: usize) -> u32 {
        self.digits.get(file).copied().unwrap_or(3)
    }

    /// The widest line of any file, in columns.
    pub(crate) fn columns(&self) -> u32 {
        self.columns
    }
}

#[cfg(test)]
mod tests;
