//! A terminal's screen as one element ([`grid`]): rows of cells on a fixed monospace grid,
//! painted directly instead of as an element per row and per wide cell.
//!
//! A row is its narrow cells' text with styled spans (one shaped line), the cells that must sit
//! exactly on their column (wide CJK, emoji, glyphs of other widths), background and selection
//! runs, and a vector layer over the text (box drawing, curly underlines). It draws exactly as
//! that row drew built from elements (`rich` line, a `text` per pinned cell, `el` fills, a
//! `canvas`): the same layouts, glyph masks and pixel snapping. A row is shaped once and keeps
//! its layout while the app hands back the same [`GridRow`], so a screen where one row changed
//! shapes that row only.

use std::cell::RefCell;
use std::rc::Rc;

use rune_taffy::prelude::*;

use super::{El, TextSpec, base};
use crate::color::Color;
use crate::shape::Shape;
use parley::Layout;

use crate::text::{Ink, Span};
use crate::tok::TextStyle;

/// One row of a [`grid`]. Build it once per row the terminal shows and hand the same `Rc` back
/// while the row is unchanged: it keeps its shaped layout.
pub struct GridRow {
    pub(crate) line: TextSpec,
    pub(crate) cells: Vec<GridCell>,
    pub(crate) fills: Vec<GridFill>,
    pub(crate) over: Option<Shape>,
    /// The row's shaped line and cells at the scale and width they were shaped for.
    pub(crate) laid: RefCell<Option<LaidRow>>,
    /// What the row drew last, to draw again while nothing it depends on changed.
    pub(crate) drawn: RefCell<Option<DrawnRow>>,
}

pub(crate) struct GridCell {
    pub(crate) col: u16,
    pub(crate) width: u16,
    pub(crate) spec: TextSpec,
}

#[derive(Clone, Copy)]
pub(crate) struct GridFill {
    pub(crate) start: u16,
    pub(crate) end: u16,
    pub(crate) color: Color,
}

/// A row's instances as it last painted them, with the key of everything they depend on (its
/// place, clip, opacity and the text system's `emit_key`) and the atlas pages its glyphs
/// sample, with their texels, for the frame that draws them again to keep resident.
pub(crate) struct DrawnRow {
    pub(crate) key: u64,
    pub(crate) instances: Vec<crate::render::Instance>,
    pub(crate) pages: Vec<(u16, u32)>,
    pub(crate) glyphs: usize,
}

pub(crate) struct LaidRow {
    pub(crate) scale: u32,
    pub(crate) width: u32,
    pub(crate) line: Rc<Layout<Ink>>,
    pub(crate) cells: Vec<Rc<Layout<Ink>>>,
}

impl GridRow {
    /// A row whose narrow cells read `text` (a space for each empty or pinned cell), styled by
    /// `spans` (colour, weight, italic, single underline) over `style`.
    pub fn new(text: impl Into<String>, spans: Vec<Span>, style: TextStyle) -> GridRow {
        let line = super::rich(text, spans).font(style).truncate_fade_width(0.0);
        GridRow { line: spec_of(line), cells: Vec::new(), fills: Vec::new(), over: None, laid: RefCell::new(None), drawn: RefCell::new(None) }
    }

    /// A cell drawn on its own at column `col`, `width` columns wide: a wide or fallback glyph
    /// that mustn't move the columns after it. The line holds a space for each of its columns.
    pub fn cell(mut self, col: u16, width: u16, text: impl Into<String>, style: TextStyle, color: Color) -> GridRow {
        self.cells.push(GridCell { col, width: width.max(1), spec: spec_of(super::text(text).font(style).color(color)) });
        self
    }

    /// Fills columns `start..end` (a background run, or the selection over one) under the text.
    pub fn fill(mut self, start: u16, end: u16, color: Color) -> GridRow {
        if end > start {
            self.fills.push(GridFill { start, end, color });
        }
        self
    }

    /// Vector strokes and fills over the text, in the row's points (box drawing, curly and
    /// double underlines, strikethrough).
    pub fn shape(mut self, shape: Shape) -> GridRow {
        self.over = (!shape.is_empty()).then_some(shape);
        self
    }

    /// What a reader reads of the row: the line with each pinned cell's text in its columns,
    /// trailing spaces dropped, and for each character the column it starts in and how many it
    /// spans.
    pub(crate) fn reading(&self) -> (String, Vec<(u16, u16)>) {
        let mut cols: Vec<(String, u16)> = self.line.content.chars().map(|c| (c.to_string(), 1)).collect();
        for c in &self.cells {
            let (at, w) = (usize::from(c.col), usize::from(c.width));
            if cols.len() < at + w {
                cols.resize(at + w, (" ".to_owned(), 1));
            }
            cols[at] = (c.spec.content.clone(), c.width);
            cols[at + 1..at + w].iter_mut().for_each(|s| *s = (String::new(), 0));
        }
        let end = cols.iter().rposition(|(s, _)| !s.trim().is_empty()).map_or(0, |i| i + 1);
        let (mut text, mut places) = (String::new(), Vec::new());
        for (col, (s, w)) in cols.into_iter().take(end).enumerate() {
            for (k, ch) in s.chars().enumerate() {
                text.push(ch);
                // Marks combining onto a cell's first character take no room of their own.
                places.push((col as u16, if k == 0 { w } else { 0 }));
            }
        }
        (text, places)
    }
}

/// `e`'s text, laid out left to right as a terminal's cells are (`paint_grid` lays it out again
/// in an element whose flow is right to left).
fn spec_of(e: El) -> TextSpec {
    let mut spec = e.text.map(crate::cold::SharedBox::into_inner).expect("a text element has a spec");
    spec.dir = crate::el::step_text_dir(spec.dir, crate::el::TextDirEvent::Flatten(Some(crate::el::Flow::Ltr)));
    spec
}

/// What [`grid`] paints.
pub(crate) struct Grid {
    pub(crate) cols: u16,
    pub(crate) cell: [f32; 2],
    pub(crate) rows: Vec<Rc<GridRow>>,
    /// The cursor's cell (column, row), as the accessibility tree's caret.
    pub(crate) caret: Option<(u16, u16)>,
}

impl std::fmt::Debug for Grid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grid").field("cell", &self.cell).field("rows", &self.rows.len()).finish()
    }
}

/// A terminal's screen: `rows` top to bottom, each `cell_h` points tall and `cols` columns of`
/// `cell_w` points wide from the element's left edge. Size the element like any box; rows past its
/// height are clipped by it as any content is. Accessibility reads it as a terminal with a
/// text line per row.
pub fn grid(cols: u16, cell_w: f32, cell_h: f32, rows: Vec<Rc<GridRow>>) -> El {
    let mut e = base(FlexDirection::Row);
    e.vis.cold_mut().grid = Some(Rc::new(Grid { cols, cell: [cell_w, cell_h], rows, caret: None }));
    e.set_role_name("terminal");
    e
}

impl El {
    /// The cell (`col`, `row`) a [`grid`]'s cursor is on, which a screen reader follows as its
    /// caret: it reads the character there and moves from it by character, word and line. It
    /// draws nothing; leave it unset while the program hides its cursor.
    pub fn grid_caret(mut self, col: u16, row: u16) -> Self {
        if let Some(g) = self.vis.cold_mut().grid.as_mut().and_then(Rc::get_mut) {
            g.caret = Some((col, row));
        }
        self
    }
}

impl GridRow {
    /// The index in [`Self::reading`]'s text of the character at column `col`: the one that
    /// starts there or covers it, or the end of the text past it.
    pub(crate) fn char_at(places: &[(u16, u16)], col: u16) -> usize {
        places.iter().position(|&(c, w)| c + w.max(1) > col).unwrap_or(places.len())
    }
}
