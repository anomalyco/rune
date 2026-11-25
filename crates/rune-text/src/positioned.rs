//! App-positioned text geometry ([`PositionedText`]) for invisible selectable, copyable, and
//! accessible text layers placed over externally painted glyphs (such as a PDF page bitmap).
//!
//! Lines are split strictly on `'\n'` (treating `"\r\n"` as one trailing line break excluded from
//! visual characters) so multi-column layouts never merge across columns. Each Unicode scalar
//! value (`char`) maps 1:1 to a sanitized `[x, y, w, h]` box in source coordinates: empty or
//! non-finite boxes are skipped visually while keeping byte and character indices aligned, and
//! identical or heavily overlapping adjacent boxes (ligatures such as `"fi"` or `"ffl"`) are
//! evenly subdivided across their shared horizontal span once on construction.

use std::cmp::Ordering as Cmp;
use std::fmt;
use std::mem::size_of;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Maximum AccessKit characters per `Role::TextRun` chunk (`word_starts` stores `u8` indices).
const A11Y_MAX_CHARS: usize = 255;

/// Immutable text plus one `[x, y, w, h]` character box per Unicode scalar value, shared across
/// view builds through an `Arc`.
///
/// Build a `PositionedText` once per page and clone the handle cheaply on each frame. Attach an
/// optional [`PositionedText::view_box`] so zooming or resizing the element scales character boxes
/// linearly without rebuilding or copying the page's geometry.
///
/// ```
/// use rune_text::PositionedText;
///
/// let text = PositionedText::new(
///     "Hi\n",
///     vec![[10.0, 20.0, 8.0, 12.0], [18.0, 20.0, 4.0, 12.0], [0.0; 4]],
/// )
/// .view_box(612.0, 792.0);
///
/// assert_eq!(text.text(), "Hi\n");
/// assert_eq!(text.len(), 3);
/// assert_eq!(text.char_count(), 3);
/// assert!(!text.is_empty());
/// assert_eq!(text.bounds(), [10.0, 20.0, 12.0, 12.0]);
/// assert!(text.byte_size() > 0);
/// ```
#[derive(Clone)]
pub struct PositionedText {
    inner: Arc<PositionedTextInner>,
    view_box: Option<[f32; 2]>,
}

struct PositionedTextInner {
    id: u64,
    text: Box<str>,
    /// UTF-8 byte offset of each `char`, plus a sentinel `text.len()` at the end.
    char_bytes: Box<[u32]>,
    /// Sanitized (and ligature-subdivided) `[x, y, w, h]` rects, one per `char` (`[0.0; 4]` = empty).
    rects: Box<[[f32; 4]]>,
    /// Logical lines split strictly on `'\n'`, in reading order.
    lines: Box<[PositionedLine]>,
    /// Visual lines sorted by `(y0, x0, line_idx)` with monotonic running prefix `max_y1`.
    y_order: Box<[YLineEntry]>,
    /// Tight visual ink bounds `[x, y, w, h]` across the whole page (`[0.0; 4]` when empty).
    bounds: [f32; 4],
}

#[derive(Clone, Copy)]
struct PositionedLine {
    byte_start: u32,
    byte_end: u32,
    vis_byte_end: u32,
    char_start: u32,
    vis_char_end: u32,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    visual_count: u32,
    first_visual: u32,
    last_visual: u32,
}

impl PositionedLine {
    fn empty(bs: usize, be: usize, vbe: usize, cs: usize, vce: usize) -> Self {
        Self {
            byte_start: bs as u32, byte_end: be as u32, vis_byte_end: vbe as u32,
            char_start: cs as u32, vis_char_end: vce as u32,
            x0: 0.0, y0: 0.0, x1: 0.0, y1: 0.0, visual_count: 0, first_visual: 0, last_visual: 0,
        }
    }
}

#[derive(Clone, Copy)]
struct YLineEntry { line_idx: u32, y0: f32, y1: f32, max_y1: f32 }

/// One AccessKit-sized (`<= 255` characters) chunk of a [`PositionedText`] line in source
/// coordinates. Hidden internal geometry used by `rune`'s accessibility tree.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq)]
pub struct PositionedA11yChunk {
    pub line_idx: usize,
    pub range: Range<usize>,
    /// Chunk bounding box `[x, y, w, h]` in source coordinates.
    pub rect: [f32; 4],
    /// UTF-8 byte length of each AccessKit character (trailing `"\r\n"` is one 2-byte character).
    pub lengths: Vec<u8>,
    /// Left offset of each character relative to `rect[0]` in source coordinates.
    pub positions: Vec<f32>,
    /// Width of each character in source coordinates.
    pub widths: Vec<f32>,
    /// True when this chunk continues the same `'\n'`-delimited line as the previous chunk.
    pub continues_line: bool,
}

/// Lazy iterator over [`PositionedA11yChunk`]s of a [`PositionedText`].
#[doc(hidden)]
pub struct PositionedA11yLines<'a> {
    text: &'a PositionedText,
    line_idx: usize,
    char_cursor: usize,
    emitted_for_line: bool,
}

impl fmt::Debug for PositionedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PositionedText")
            .field("id", &self.inner.id)
            .field("len", &self.inner.text.len())
            .field("char_count", &self.char_count())
            .field("lines", &self.inner.lines.len())
            .field("bounds", &self.inner.bounds)
            .field("view_box", &self.view_box)
            .finish()
    }
}

#[inline]
fn sanitize_rect(r: [f32; 4]) -> [f32; 4] {
    let ok = r[0].is_finite() && r[1].is_finite() && r[2].is_finite() && r[3].is_finite()
        && r[2] > 0.0 && r[3] > 0.0 && (r[0] + r[2]).is_finite() && (r[1] + r[3]).is_finite();
    if ok { r } else { [0.0; 4] }
}

#[inline] fn is_visual(r: [f32; 4]) -> bool { r[2] > 0.0 && r[3] > 0.0 }
#[inline] fn horiz_dist(x0: f32, x1: f32, x: f32) -> f32 { (x0 - x).max(x - x1).max(0.0) }
#[inline] fn vert_dist(y0: f32, y1: f32, y: f32) -> f32 { (y0 - y).max(y - y1).max(0.0) }
#[inline] fn rect_contains(r: [f32; 4], x: f32, y: f32) -> bool { x >= r[0] && x <= r[0] + r[2] && y >= r[1] && y <= r[1] + r[3] }
#[inline] fn rect_dist_sq(r: [f32; 4], x: f32, y: f32) -> f32 {
    let (dx, dy) = (horiz_dist(r[0], r[0] + r[2], x), vert_dist(r[1], r[1] + r[3], y));
    dx * dx + dy * dy
}
#[inline] fn cmp2(a: (f32, f32), b: (f32, f32)) -> Cmp { a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)) }

/// Evenly subdivides identical or heavily overlapping adjacent visual rects (ligatures such as
/// `"fi"` or `"ffl"`) along their shared horizontal span within one line's visual char range.
fn subdivide_ligatures(rects: &mut [[f32; 4]], start: usize, end: usize) {
    let mut i = start;
    while i < end {
        if !is_visual(rects[i]) { i += 1; continue; }
        let r0 = rects[i];
        let (mut gx0, mut gy0, mut gx1, mut gy1) = (r0[0], r0[1], r0[0] + r0[2], r0[1] + r0[3]);
        let mut j = i + 1;
        while j < end && is_visual(rects[j]) {
            let c = rects[j];
            let (cx0, cy0, cx1, cy1) = (c[0], c[1], c[0] + c[2], c[1] + c[3]);
            if gy1.min(cy1) <= gy0.max(cy0) { break; }
            let h_overlap = gx1.min(cx1) - gx0.max(cx0);
            let min_w = (gx1 - gx0).min(c[2]);
            let same_span = (cx0 - gx0).abs() <= 0.01 && (cx1 - gx1).abs() <= 0.01;
            if !(same_span || h_overlap > 0.6 * min_w) { break; }
            gx0 = gx0.min(cx0); gy0 = gy0.min(cy0); gx1 = gx1.max(cx1); gy1 = gy1.max(cy1);
            j += 1;
        }
        let count = j - i;
        if count > 1 {
            let (gh, step) = (gy1 - gy0, (gx1 - gx0) / count as f32);
            for m in 0..count {
                let sx0 = gx0 + m as f32 * step;
                let sx1 = if m + 1 == count { gx1 } else { gx0 + (m + 1) as f32 * step };
                rects[i + m] = [sx0, gy0, (sx1 - sx0).max(f32::MIN_POSITIVE), gh];
            }
        }
        i = j;
    }
}

impl PositionedText {
    /// Builds immutable positioned text once per page: one `[x, y, w, h]` rect in logical points
    /// per Unicode scalar value (`char`) of `text`.
    ///
    /// Empty or non-finite rects (`w <= 0.0 || h <= 0.0`) are allowed for `'\n'` and non-visual
    /// characters and are skipped visually while keeping character and byte indices aligned.
    /// Fewer rects than characters pad trailing characters with empty rects; extra rects are
    /// ignored. Identical or heavily overlapping adjacent rects (ligatures) are evenly subdivided
    /// across their shared horizontal span.
    ///
    /// ```
    /// use rune_text::PositionedText;
    ///
    /// let pt = PositionedText::new("AB", vec![[0.0, 0.0, 8.0, 12.0], [8.0, 0.0, 8.0, 12.0]]);
    /// assert_eq!(pt.char_count(), 2);
    /// assert_eq!(pt.bounds(), [0.0, 0.0, 16.0, 12.0]);
    /// ```
    pub fn new(text: impl Into<String>, rects: impl Into<Vec<[f32; 4]>>) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let text: String = text.into();
        let mut rects: Vec<[f32; 4]> = rects.into();
        let text_slice = &text[..text.floor_char_boundary(text.len().min(u32::MAX as usize))];

        let mut char_bytes = Vec::with_capacity(text_slice.len().saturating_add(1));
        for (b, _) in text_slice.char_indices() { char_bytes.push(b as u32); }
        let char_count = char_bytes.len();
        char_bytes.push(text_slice.len() as u32);

        rects.truncate(char_count);
        for r in &mut rects { *r = sanitize_rect(*r); }
        if rects.len() < char_count { rects.resize(char_count, [0.0; 4]); }

        // Split lines strictly on '\n', treating "\r\n" as one trailing break excluded from
        // visual characters.
        let bytes = text_slice.as_bytes();
        let mut lines = Vec::new();
        let (mut lbs, mut lcs) = (0usize, 0usize);
        for ci in 0..char_count {
            let b = char_bytes[ci] as usize;
            if bytes[b] == b'\n' {
                let has_cr = ci > lcs && b > lbs && bytes[b - 1] == b'\r';
                let (vbe, vce) = if has_cr { (b - 1, ci - 1) } else { (b, ci) };
                rects[ci] = [0.0; 4];
                if has_cr { rects[ci - 1] = [0.0; 4]; }
                lines.push(PositionedLine::empty(lbs, b + 1, vbe, lcs, vce));
                lbs = b + 1; lcs = ci + 1;
            }
        }
        if lbs < text_slice.len() {
            lines.push(PositionedLine::empty(lbs, text_slice.len(), text_slice.len(), lcs, char_count));
        }

        let (mut px0, mut py0, mut px1, mut py1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        let mut y_order = Vec::with_capacity(lines.len());
        for (li, line) in lines.iter_mut().enumerate() {
            let (cs, ve) = (line.char_start as usize, line.vis_char_end as usize);
            subdivide_ligatures(&mut rects, cs, ve);
            let (mut lx0, mut ly0, mut lx1, mut ly1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
            let (mut count, mut first, mut last) = (0u32, 0u32, 0u32);
            for (ci, &r) in rects.iter().enumerate().take(ve).skip(cs) {
                if is_visual(r) {
                    if count == 0 { first = ci as u32; }
                    last = ci as u32;
                    count += 1;
                    lx0 = lx0.min(r[0]); ly0 = ly0.min(r[1]);
                    lx1 = lx1.max(r[0] + r[2]); ly1 = ly1.max(r[1] + r[3]);
                }
            }
            if count > 0 {
                line.x0 = lx0; line.y0 = ly0; line.x1 = lx1; line.y1 = ly1;
                line.visual_count = count; line.first_visual = first; line.last_visual = last;
                px0 = px0.min(lx0); py0 = py0.min(ly0); px1 = px1.max(lx1); py1 = py1.max(ly1);
                y_order.push(YLineEntry { line_idx: li as u32, y0: ly0, y1: ly1, max_y1: 0.0 });
            }
        }

        y_order.sort_by(|a, b| {
            a.y0.total_cmp(&b.y0)
                .then_with(|| lines[a.line_idx as usize].x0.total_cmp(&lines[b.line_idx as usize].x0))
                .then_with(|| a.line_idx.cmp(&b.line_idx))
        });
        let mut run_max = f32::NEG_INFINITY;
        for e in &mut y_order { run_max = run_max.max(e.y1); e.max_y1 = run_max; }

        let bounds = if px0.is_finite() {
            [px0, py0, (px1 - px0).max(0.0), (py1 - py0).max(0.0)]
        } else {
            [0.0; 4]
        };
        let boxed_text: Box<str> = if text_slice.len() == text.len() {
            text.into_boxed_str()
        } else {
            text_slice.to_owned().into_boxed_str()
        };
        PositionedText {
            inner: Arc::new(PositionedTextInner {
                id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
                text: boxed_text,
                char_bytes: char_bytes.into_boxed_slice(),
                rects: rects.into_boxed_slice(),
                lines: lines.into_boxed_slice(),
                y_order: y_order.into_boxed_slice(),
                bounds,
            }),
            view_box: None,
        }
    }

    /// Sets the source coordinate space `[0..width, 0..height]` (matching `Shape::view_box`).
    ///
    /// Character boxes scale linearly onto the element's content box so zooming or resizing never
    /// rebuilds or copies the underlying text or geometry. Non-finite or non-positive dimensions
    /// clear the view box.
    ///
    /// ```
    /// use rune_text::PositionedText;
    ///
    /// let page = PositionedText::new("A", vec![[10.0, 20.0, 8.0, 12.0]]).view_box(612.0, 792.0);
    /// assert_eq!(page.text(), "A");
    /// ```
    pub fn view_box(mut self, width: f32, height: f32) -> Self {
        self.view_box = (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0).then_some([width, height]);
        self
    }

    /// The full UTF-8 text in reading order.
    ///
    /// ```
    /// use rune_text::PositionedText;
    ///
    /// let pt = PositionedText::new("hello", vec![[0.0, 0.0, 6.0, 12.0]; 5]);
    /// assert_eq!(pt.text(), "hello");
    /// ```
    pub fn text(&self) -> &str { &self.inner.text }

    /// Length of [`Self::text`] in UTF-8 bytes.
    ///
    /// ```
    /// use rune_text::PositionedText;
    ///
    /// let pt = PositionedText::new("é", vec![[0.0, 0.0, 7.0, 12.0]]);
    /// assert_eq!(pt.len(), 2);
    /// assert_eq!(pt.char_count(), 1);
    /// ```
    pub fn len(&self) -> usize { self.inner.text.len() }

    /// Returns `true` when [`Self::text`] has zero bytes.
    ///
    /// ```
    /// use rune_text::PositionedText;
    ///
    /// assert!(PositionedText::new("", Vec::new()).is_empty());
    /// ```
    pub fn is_empty(&self) -> bool { self.inner.text.is_empty() }

    /// Number of Unicode scalar values (`char`s) in [`Self::text`].
    ///
    /// ```
    /// use rune_text::PositionedText;
    ///
    /// let pt = PositionedText::new("a🦀", vec![[0.0, 0.0, 8.0, 12.0], [8.0, 0.0, 12.0, 12.0]]);
    /// assert_eq!(pt.char_count(), 2);
    /// ```
    pub fn char_count(&self) -> usize { self.inner.rects.len() }

    /// Tight visual bounding box `[x, y, w, h]` of all non-empty visual characters in source
    /// coordinates, or `[0.0; 4]` when there are no visual boxes.
    ///
    /// ```
    /// use rune_text::PositionedText;
    ///
    /// let pt = PositionedText::new("AB", vec![[10.0, 20.0, 8.0, 12.0], [18.0, 20.0, 9.0, 14.0]]);
    /// assert_eq!(pt.bounds(), [10.0, 20.0, 17.0, 14.0]);
    /// ```
    pub fn bounds(&self) -> [f32; 4] { self.inner.bounds }

    /// Exact resident memory footprint in bytes of the shared backing allocation (struct plus text,
    /// character offsets, rects, line records, and spatial index).
    ///
    /// ```
    /// use rune_text::PositionedText;
    ///
    /// let small = PositionedText::new("A", vec![[0.0, 0.0, 8.0, 12.0]]);
    /// let bigger = PositionedText::new("A".repeat(100), vec![[0.0, 0.0, 8.0, 12.0]; 100]);
    /// assert!(bigger.byte_size() > small.byte_size());
    /// ```
    pub fn byte_size(&self) -> usize {
        size_of::<PositionedTextInner>()
            + self.inner.text.len()
            + self.inner.char_bytes.len() * size_of::<u32>()
            + self.inner.rects.len() * size_of::<[f32; 4]>()
            + self.inner.lines.len() * size_of::<PositionedLine>()
            + self.inner.y_order.len() * size_of::<YLineEntry>()
    }

    // -----------------------------------------------------------------------------------------
    // Hidden runtime & accessibility geometry helpers (consumed by `crates/rune`)
    // -----------------------------------------------------------------------------------------

    #[doc(hidden)] pub fn id(&self) -> u64 { self.inner.id }
    #[doc(hidden)] pub fn view_box_size(&self) -> Option<[f32; 2]> { self.view_box }
    #[doc(hidden)] pub fn line_count(&self) -> usize { self.inner.lines.len() }

    /// Source-to-element scale factors `[sx, sy]` for an element whose content box is
    /// `content_w` × `content_h` logical points. Returns `[1.0, 1.0]` when no view box is set.
    #[doc(hidden)]
    pub fn scale_for(&self, content_w: f32, content_h: f32) -> [f32; 2] {
        match self.view_box {
            Some([vw, vh]) => [
                if content_w.is_finite() && content_w > 0.0 { content_w / vw } else { 0.0 },
                if content_h.is_finite() && content_h > 0.0 { content_h / vh } else { 0.0 },
            ],
            None => [1.0, 1.0],
        }
    }

    /// Visual lines in reading order as `(visual_byte_range, [x, y, w, h])` in source coordinates.
    #[doc(hidden)]
    pub fn visual_lines(&self) -> impl Iterator<Item = (Range<usize>, [f32; 4])> + '_ {
        self.inner.lines.iter().filter(|l| l.visual_count > 0).map(|l| {
            (l.byte_start as usize..l.vis_byte_end as usize, [l.x0, l.y0, (l.x1 - l.x0).max(0.0), (l.y1 - l.y0).max(0.0)])
        })
    }

    /// Hit-tests source point `(x, y)` to `(caret_byte, cluster_byte)`.
    ///
    /// Checks exact character containment first, then line-box containment, then y-band matching
    /// (preferring a vertically nearby column-aligned line horizontally disjoint from the y-band
    /// line in a two-column paragraph gap, while staying on the short line when clicking right of
    /// a short line in the same column), and finally branch-and-bound nearest visual line.
    #[doc(hidden)]
    pub fn hit(&self, x: f32, y: f32) -> (usize, Option<usize>) {
        let y_order = &self.inner.y_order;
        if y_order.is_empty() || !x.is_finite() || !y.is_finite() { return (0, None); }
        let (lines, rects, char_bytes) = (&self.inner.lines, &self.inner.rects, &self.inner.char_bytes);

        // Stab y in O(log L + k) using sorted y0 and monotonic running prefix max_y1.
        let hi = y_order.partition_point(|e| e.y0 <= y);
        let lo = y_order[..hi].partition_point(|e| e.max_y1 < y);

        // 1. Exact character rect containment.
        let mut exact_char: Option<(f32, f32, usize, usize)> = None;
        for e in &y_order[lo..hi] {
            if e.y1 < y { continue; }
            let li = e.line_idx as usize;
            let l = &lines[li];
            if x < l.x0 || x > l.x1 { continue; }
            for ci in l.first_visual as usize..=l.last_visual as usize {
                let r = rects[ci];
                if !is_visual(r) || !rect_contains(r, x, y) { continue; }
                let cand = ((y - (r[1] + r[3] * 0.5)).abs(), (x - (r[0] + r[2] * 0.5)).abs(), li, ci);
                if exact_char.is_none_or(|b| cmp2((cand.0, cand.1), (b.0, b.1)).then((li, ci).cmp(&(b.2, b.3))).is_lt()) {
                    exact_char = Some(cand);
                }
            }
        }
        if let Some((_, _, _, ci)) = exact_char {
            let r = rects[ci];
            let (start, end) = (char_bytes[ci] as usize, char_bytes[ci + 1] as usize);
            return (if x <= r[0] + r[2] * 0.5 { start } else { end }, Some(start));
        }

        // 2. Exact line bounding-box containment, while tracking closest horizontal y-band line.
        let mut exact_line: Option<(f32, usize)> = None;
        let mut best_band: Option<(f32, f32, usize)> = None;
        for e in &y_order[lo..hi] {
            if e.y1 < y { continue; }
            let li = e.line_idx as usize;
            let l = &lines[li];
            if x >= l.x0 && x <= l.x1 {
                let d2 = self.min_char_dist_sq(li, x, y);
                if exact_line.is_none_or(|b| d2.total_cmp(&b.0).then(li.cmp(&b.1)).is_lt()) {
                    exact_line = Some((d2, li));
                }
            } else {
                let cand = (horiz_dist(l.x0, l.x1, x), (y - (l.y0 + l.y1) * 0.5).abs(), li);
                if best_band.is_none_or(|b| cmp2((cand.0, cand.1), (b.0, b.1)).then(li.cmp(&b.2)).is_lt()) {
                    best_band = Some(cand);
                }
            }
        }
        if let Some((_, li)) = exact_line { return self.hit_on_line(li, x, y); }

        // 3. Y-band match: distinguish a two-column paragraph gap from clicking right of a short
        // line in the same column.
        if let Some((band_dx, _, band_li)) = best_band {
            let band = &lines[band_li];
            let ref_h = (band.y1 - band.y0).clamp(10.0, 32.0);
            let search_y = (ref_h * 4.0).max(48.0);
            let s_hi = y_order.partition_point(|e| e.y0 <= y + search_y);
            let s_lo = y_order[..s_hi].partition_point(|e| e.max_y1 < y - search_y);

            let mut col_cand: Option<(f32, f32, usize)> = None;
            for e in &y_order[s_lo..s_hi] {
                if e.y1 < y - search_y { continue; }
                let ci = e.line_idx as usize;
                let c = &lines[ci];
                if x < c.x0 || x > c.x1 || (c.x0 < band.x1 && band.x0 < c.x1) { continue; }
                let dy = vert_dist(c.y0, c.y1, y);
                if dy > search_y || (dy > band_dx && dy > ref_h * 2.5) { continue; }
                let cand = (dy, (x - (c.x0 + c.x1) * 0.5).abs(), ci);
                if col_cand.is_none_or(|b| cmp2((cand.0, cand.1), (b.0, b.1)).then(ci.cmp(&b.2)).is_lt()) {
                    col_cand = Some(cand);
                }
            }
            if let Some((_, _, col_li)) = col_cand {
                let col = &lines[col_li];
                // Stay on a same-column y-band line if one already exists (e.g. far right of a
                // very short line in column 1 beside column 2).
                let mut same_col: Option<(f32, usize)> = None;
                for e in &y_order[lo..hi] {
                    if e.y1 < y { continue; }
                    let bi = e.line_idx as usize;
                    let b = &lines[bi];
                    if b.x0 < col.x1 && col.x0 < b.x1 {
                        let dx = horiz_dist(b.x0, b.x1, x);
                        if same_col.is_none_or(|p| dx.total_cmp(&p.0).then(bi.cmp(&p.1)).is_lt()) {
                            same_col = Some((dx, bi));
                        }
                    }
                }
                return self.hit_on_line(same_col.map_or(col_li, |(_, bi)| bi), x, y);
            }
            return self.hit_on_line(band_li, x, y);
        }

        // 4. Nearest visual line fallback via exact branch-and-bound on `y_order`.
        let mut best = (f32::INFINITY, f32::INFINITY, f32::INFINITY, y_order[0].line_idx as usize);
        let consider = |best: &mut (f32, f32, f32, usize), li: usize, dy: f32| {
            let l = &lines[li];
            let dx = horiz_dist(l.x0, l.x1, x);
            let d2 = dx * dx + dy * dy;
            let ord = d2.total_cmp(&best.0).then_with(|| cmp2((dy, dx), (best.1, best.2))).then(li.cmp(&best.3));
            if ord.is_lt() { *best = (d2, dy, dx, li); }
        };
        for e in &y_order[hi..] {
            let dy = (e.y0 - y).max(0.0);
            if dy * dy > best.0 { break; }
            consider(&mut best, e.line_idx as usize, dy);
        }
        for e in y_order[..hi].iter().rev() {
            let min_dy = (y - e.max_y1).max(0.0);
            if min_dy * min_dy > best.0 { break; }
            let dy = (y - e.y1).max(0.0);
            if dy * dy <= best.0 { consider(&mut best, e.line_idx as usize, dy); }
        }
        self.hit_on_line(best.3, x, y)
    }

    /// Merged highlight rectangles `[x, y, w, h]` in source coordinates covering byte range `a..b`.
    ///
    /// Contiguous same-line visual characters merge into crisp gap-free highlight rectangles per
    /// line (skipping empty rects), never bridging across line breaks or large column/tab gaps.
    #[doc(hidden)]
    pub fn range_boxes(&self, a: usize, b: usize) -> Vec<[f32; 4]> {
        let mut out = Vec::new();
        self.for_each_range_box(a, b, |r| out.push(r));
        out
    }

    /// Allocation-free visitor for [`Self::range_boxes`], used on hot paint and hit paths.
    #[doc(hidden)]
    pub fn for_each_range_box(&self, a: usize, b: usize, mut f: impl FnMut([f32; 4])) {
        let len = self.inner.text.len();
        let a = self.inner.text.floor_char_boundary(a.min(len));
        let b = self.inner.text.ceil_char_boundary(b.min(len));
        if a >= b || self.inner.lines.is_empty() { return; }
        let (lines, rects, char_bytes) = (&self.inner.lines, &self.inner.rects, &self.inner.char_bytes);
        let first_line = lines.partition_point(|l| (l.byte_end as usize) <= a);

        for line in &lines[first_line..] {
            if (line.byte_start as usize) >= b { break; }
            let (ls, lve) = (line.byte_start as usize, line.vis_byte_end as usize);
            if line.visual_count == 0 || a >= lve || b <= ls { continue; }
            let c_start = if a <= ls { line.first_visual as usize } else { self.char_at_byte(a).clamp(line.first_visual as usize, line.last_visual as usize + 1) };
            let c_end = if b >= lve { line.last_visual as usize + 1 } else { self.char_at_byte(b).min(line.last_visual as usize + 1) };
            let mut cur: Option<[f32; 4]> = None;
            for ci in c_start..c_end {
                let (cs, ce) = (char_bytes[ci] as usize, char_bytes[ci + 1] as usize);
                let r = rects[ci];
                if ce <= a || cs >= b || !is_visual(r) { continue; }
                let (rx0, ry0, rx1, ry1) = (r[0], r[1], r[0] + r[2], r[1] + r[3]);
                match cur {
                    None => cur = Some([rx0, ry0, rx1, ry1]),
                    Some([cx0, cy0, cx1, cy1]) => {
                        let v_overlap = cy1.min(ry1) > cy0.max(ry0);
                        let h_gap = (rx0 - cx1).max(cx0 - rx1);
                        let max_h = (cy1 - cy0).max(ry1 - ry0);
                        if v_overlap && h_gap <= max_h * 2.0 {
                            cur = Some([cx0.min(rx0), cy0.min(ry0), cx1.max(rx1), cy1.max(ry1)]);
                        } else {
                            f([cx0, cy0, (cx1 - cx0).max(0.0), (cy1 - cy0).max(0.0)]);
                            cur = Some([rx0, ry0, rx1, ry1]);
                        }
                    }
                }
            }
            if let Some([cx0, cy0, cx1, cy1]) = cur {
                f([cx0, cy0, (cx1 - cx0).max(0.0), (cy1 - cy0).max(0.0)]);
            }
        }
    }

    /// Caret vertical bar `[x, y, 0.0, h]` in source coordinates at UTF-8 byte offset `byte`,
    /// or `None` when the page contains no visual rectangles.
    #[doc(hidden)]
    pub fn caret(&self, byte: usize) -> Option<[f32; 4]> {
        if self.inner.y_order.is_empty() || self.inner.lines.is_empty() { return None; }
        let byte = self.inner.text.floor_char_boundary(byte.min(self.inner.text.len()));
        let li = self.line_index_for_byte(byte);
        let line = &self.inner.lines[li];
        if line.visual_count == 0 { return self.blank_line_rect(li); }
        let (ci, first, last, rects) = (self.char_at_byte(byte), line.first_visual as usize, line.last_visual as usize, &self.inner.rects);
        if ci <= first { let r = rects[first]; return Some([r[0], r[1], 0.0, r[3]]); }
        if ci > last { let r = rects[last]; return Some([r[0] + r[2], r[1], 0.0, r[3]]); }
        if is_visual(rects[ci]) { let r = rects[ci]; return Some([r[0], r[1], 0.0, r[3]]); }
        for k in (first..ci).rev() {
            if is_visual(rects[k]) { let r = rects[k]; return Some([r[0] + r[2], r[1], 0.0, r[3]]); }
        }
        let r = rects[first];
        Some([r[0], r[1], 0.0, r[3]])
    }

    /// First (`!end`) or last visual (`end`) byte offset on the `'\n'`-delimited line holding `byte`.
    #[doc(hidden)]
    pub fn line_edge(&self, byte: usize, end: bool) -> usize {
        if self.inner.lines.is_empty() { return 0; }
        let byte = self.inner.text.floor_char_boundary(byte.min(self.inner.text.len()));
        let line = &self.inner.lines[self.line_index_for_byte(byte)];
        if end { line.vis_byte_end as usize } else { line.byte_start as usize }
    }

    /// Moves one `'\n'`-delimited line down (`down`) or up (`!down`) in reading order, preserving
    /// `goal_x` (in source coordinates). Returns `None` past the first or last line.
    #[doc(hidden)]
    pub fn move_line_strict(&self, byte: usize, down: bool, goal_x: Option<f32>) -> Option<(usize, f32)> {
        let lines = &self.inner.lines;
        if lines.is_empty() { return None; }
        let byte = self.inner.text.floor_char_boundary(byte.min(self.inner.text.len()));
        let li = self.line_index_for_byte(byte);
        let target_li = if down { (li + 1 < lines.len()).then_some(li + 1)? } else { li.checked_sub(1)? };
        let x = goal_x.filter(|v| v.is_finite()).unwrap_or_else(|| self.caret(byte).map_or(0.0, |c| c[0]));
        let target = &lines[target_li];
        if target.visual_count == 0 { return Some((target.byte_start as usize, x)); }
        Some((self.hit_on_line(target_li, x, (target.y0 + target.y1) * 0.5).0, x))
    }

    /// Hit-tests source `x` on the first (`first`) or last (`!first`) line.
    #[doc(hidden)]
    pub fn edge_line_hit(&self, x: f32, first: bool) -> usize {
        let lines = &self.inner.lines;
        if lines.is_empty() { return 0; }
        let li = if first { 0 } else { lines.len() - 1 };
        let line = &lines[li];
        if line.visual_count == 0 || !x.is_finite() { return line.byte_start as usize; }
        self.hit_on_line(li, x, (line.y0 + line.y1) * 0.5).0
    }

    /// Lazy iterator over `<= 255`-character AccessKit chunks covering every line in reading order.
    #[doc(hidden)]
    pub fn a11y_lines(&self) -> PositionedA11yLines<'_> {
        PositionedA11yLines {
            text: self,
            line_idx: 0,
            char_cursor: self.inner.lines.first().map_or(0, |l| l.char_start as usize),
            emitted_for_line: false,
        }
    }

    fn char_at_byte(&self, byte: usize) -> usize {
        self.inner.char_bytes.partition_point(|&b| (b as usize) < byte)
    }

    fn line_index_for_byte(&self, byte: usize) -> usize {
        let lines = &self.inner.lines;
        lines.partition_point(|l| (l.byte_end as usize) <= byte).min(lines.len() - 1)
    }

    fn min_char_dist_sq(&self, line_idx: usize, x: f32, y: f32) -> f32 {
        let l = &self.inner.lines[line_idx];
        let mut best = f32::INFINITY;
        for &r in &self.inner.rects[l.first_visual as usize..=l.last_visual as usize] {
            if is_visual(r) { best = best.min(rect_dist_sq(r, x, y)); }
        }
        best
    }

    fn hit_on_line(&self, line_idx: usize, x: f32, y: f32) -> (usize, Option<usize>) {
        let l = &self.inner.lines[line_idx];
        if l.visual_count == 0 { return (l.byte_start as usize, None); }
        let (rects, char_bytes) = (&self.inner.rects, &self.inner.char_bytes);
        let (first, last) = (l.first_visual as usize, l.last_visual as usize);
        if x <= l.x0 { return (l.byte_start as usize, None); }
        if x >= l.x1 { return (l.vis_byte_end as usize, Some(char_bytes[last] as usize)); }

        let mut best: Option<(f32, f32, f32, usize)> = None;
        for ci in first..=last {
            let r = rects[ci];
            if !is_visual(r) { continue; }
            let cand = (horiz_dist(r[0], r[0] + r[2], x), vert_dist(r[1], r[1] + r[3], y), (x - (r[0] + r[2] * 0.5)).abs(), ci);
            if best.is_none_or(|b| cmp2((cand.0, cand.1), (b.0, b.1)).then_with(|| cand.2.total_cmp(&b.2)).then(ci.cmp(&b.3)).is_lt()) {
                best = Some(cand);
            }
        }
        let Some((_, _, _, ci)) = best else { return (l.byte_start as usize, None); };
        let r = rects[ci];
        let (start, end) = (char_bytes[ci] as usize, char_bytes[ci + 1] as usize);
        (if x <= r[0] + r[2] * 0.5 { start } else { end }, Some(start))
    }

    fn blank_line_rect(&self, line_idx: usize) -> Option<[f32; 4]> {
        let lines = &self.inner.lines;
        let prev = lines[..line_idx].iter().rev().find(|l| l.visual_count > 0);
        let next = lines[line_idx + 1..].iter().find(|l| l.visual_count > 0);
        match (prev, next) {
            (Some(p), Some(n)) if n.y0 > p.y1 => Some([p.x0, p.y1, 0.0, (n.y0 - p.y1).min((p.y1 - p.y0).max(1.0)).max(1.0)]),
            (Some(p), _) => Some([p.x0, p.y1, 0.0, (p.y1 - p.y0).max(1.0)]),
            (None, Some(n)) => { let nh = (n.y1 - n.y0).max(1.0); Some([n.x0, (n.y0 - nh).max(0.0), 0.0, nh]) }
            (None, None) => None,
        }
    }
}

impl Iterator for PositionedA11yLines<'_> {
    type Item = PositionedA11yChunk;

    fn next(&mut self) -> Option<Self::Item> {
        let inner = &self.text.inner;
        let line = inner.lines.get(self.line_idx)?;
        let li = self.line_idx;
        let vis_end = line.vis_char_end as usize;
        let break_bytes = (line.byte_end - line.vis_byte_end) as usize;
        let has_break = break_bytes > 0;
        let continues_line = self.emitted_for_line;

        let rem_vis = vis_end.saturating_sub(self.char_cursor);
        let take_vis = if rem_vis + usize::from(has_break) <= A11Y_MAX_CHARS {
            rem_vis
        } else if rem_vis == A11Y_MAX_CHARS && has_break {
            A11Y_MAX_CHARS - 1
        } else {
            A11Y_MAX_CHARS
        };
        let chunk_vis_end = self.char_cursor + take_vis;
        let include_break = has_break && chunk_vis_end == vis_end;
        let byte_start = inner.char_bytes[self.char_cursor] as usize;
        let byte_end = if include_break { line.byte_end as usize } else { inner.char_bytes[chunk_vis_end] as usize };

        let cap = take_vis + usize::from(include_break);
        let (mut lengths, mut positions, mut widths) = (Vec::with_capacity(cap), Vec::with_capacity(cap), Vec::with_capacity(cap));
        let (mut cx0, mut cy0, mut cx1, mut cy1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for &r in &inner.rects[self.char_cursor..chunk_vis_end] {
            if is_visual(r) {
                cx0 = cx0.min(r[0]); cy0 = cy0.min(r[1]);
                cx1 = cx1.max(r[0] + r[2]); cy1 = cy1.max(r[1] + r[3]);
            }
        }
        let rect = if cx0.is_finite() {
            [cx0, cy0, (cx1 - cx0).max(0.0), (cy1 - cy0).max(0.0)]
        } else {
            self.text.blank_line_rect(li).unwrap_or([0.0; 4])
        };

        let mut last_end = 0.0f32;
        for ci in self.char_cursor..chunk_vis_end {
            lengths.push((inner.char_bytes[ci + 1] - inner.char_bytes[ci]) as u8);
            let r = inner.rects[ci];
            if is_visual(r) {
                let pos = (r[0] - rect[0]).max(0.0);
                positions.push(pos); widths.push(r[2]);
                last_end = last_end.max(pos + r[2]);
            } else {
                positions.push(last_end); widths.push(0.0);
            }
        }
        if include_break {
            lengths.push(break_bytes as u8);
            positions.push(rect[2]);
            widths.push(0.0);
        }

        if chunk_vis_end < vis_end {
            self.char_cursor = chunk_vis_end;
            self.emitted_for_line = true;
        } else {
            self.line_idx += 1;
            self.char_cursor = inner.lines.get(self.line_idx).map_or(0, |l| l.char_start as usize);
            self.emitted_for_line = false;
        }

        Some(PositionedA11yChunk { line_idx: li, range: byte_start..byte_end, rect, lengths, positions, widths, continues_line })
    }
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<PositionedText>();
};

#[cfg(test)]
#[path = "positioned_tests.rs"]
mod tests;
