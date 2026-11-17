// Rune (vendor/accesskit_macos.patch): a text field too long for its toolkit to publish text
// runs (Rune stops at 64 KiB) keeps its text ranges. The toolkit marks such a field by putting its
// selection on the field itself, as byte offsets into its value, and publishes a single
// GenericContainer (`class_name = "\x1frune-lines"`) holding one GenericContainer child
// (`class_name = "\x1frune-line"`, `author_id = "<start>:<end>"`) per drawn visual line. Ranges
// are answered from the value when a reader asks, and screen rectangles, hit-testing and wrapped
// visual lines are answered in the drawn band without shaping off-screen rows.
// The same file is in accesskit_windows and accesskit_macos; each uses part of it.
#![allow(dead_code)]

use std::cell::RefCell;

use accesskit::{Point, Rect, Role, TextDirection, TextSelection};
use accesskit_consumer::NodeRef;

const LINES_BOX_CLASS: &str = "\x1frune-lines";
const LINE_CLASS: &str = "\x1frune-line";
const CHECKPOINT_STRIDE: usize = 65_536;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unit {
    Character,
    Word,
    Line,
    Paragraph,
    Document,
}

/// One visual line currently drawn on screen for a long field.
#[derive(Clone, Copy)]
pub(crate) struct DrawnLine<'a> {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) node: NodeRef<'a>,
}

/// All drawn-line metadata published on `node` for the current tree snapshot.
pub(crate) struct DrawnState<'a> {
    pub(crate) lines: Vec<DrawnLine<'a>>,
    pub(crate) content_key: u64,
    pub(crate) upstream_caret: bool,
    pub(crate) caret_byte: usize,
}

struct Band {
    start: usize,
    end: usize,
    first: usize,
    count: usize,
}

#[derive(Clone, Copy)]
struct Mark {
    byte: usize,
    utf16: usize,
    newlines: usize,
}

struct Checkpoints {
    content_key: u64,
    len: usize,
    edge: [u8; 32],
    marks: Vec<Mark>,
}

const CHECKPOINT_SLOTS: usize = 4;

struct CheckpointRing {
    slots: [Option<Checkpoints>; CHECKPOINT_SLOTS],
    next: usize,
}

thread_local! {
    static CHECKPOINTS: RefCell<CheckpointRing> = const {
        RefCell::new(CheckpointRing {
            slots: [None, None, None, None],
            next: 0,
        })
    };
}

fn edge_sample(s: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    let b = s.as_bytes();
    let head = b.len().min(16);
    out[..head].copy_from_slice(&b[..head]);
    let tail = b.len().min(16);
    out[16..16 + tail].copy_from_slice(&b[b.len() - tail..]);
    out
}

fn with_checkpoints<R>(s: &str, content_key: u64, f: impl FnOnce(&[Mark]) -> R) -> R {
    if s.len() <= CHECKPOINT_STRIDE {
        return f(&[Mark { byte: 0, utf16: 0, newlines: 0 }]);
    }
    let edge = edge_sample(s);
    CHECKPOINTS.with(|cell| {
        let mut ring = cell.borrow_mut();
        if let Some(hit) = ring.slots.iter().flatten().find(|c| {
            c.content_key == content_key && c.len == s.len() && c.edge == edge
        }) {
            return f(hit.marks.as_slice());
        }
        let mut marks = Vec::with_capacity(s.len() / CHECKPOINT_STRIDE + 2);
        marks.push(Mark { byte: 0, utf16: 0, newlines: 0 });
        let mut utf16 = 0usize;
        let mut newlines = 0usize;
        let mut next_at = CHECKPOINT_STRIDE;
        for (k, ch) in s.char_indices() {
            if k >= next_at {
                marks.push(Mark { byte: k, utf16, newlines });
                next_at = k + CHECKPOINT_STRIDE;
            }
            utf16 += ch.len_utf16();
            if ch == '\n' {
                newlines += 1;
            }
        }
        let idx = ring.next % CHECKPOINT_SLOTS;
        ring.next = ring.next.wrapping_add(1);
        ring.slots[idx] = Some(Checkpoints { content_key, len: s.len(), edge, marks });
        let marks = ring.slots[idx].as_ref().map_or(&[][..], |c| c.marks.as_slice());
        f(marks)
    })
}

/// The value and selection a long field's text ranges are answered from.
pub(crate) fn long_text<'a>(node: &NodeRef<'a>) -> Option<(Text<'a>, &'a TextSelection)> {
    if !node.is_text_input() || node.is_password_input() || node.supports_text_ranges() {
        return None;
    }
    let data = node.data();
    let selection = data.text_selection()?;
    let (id, _) = node.locate();
    if selection.anchor.node != id || selection.focus.node != id {
        return None;
    }
    Some((Text(data.value().unwrap_or("")), selection))
}

/// Drawn visual lines Rune attached to `node` inside its `\x1frune-lines` container, sorted and
/// strictly non-overlapping.
pub(crate) fn drawn_state<'a>(node: &NodeRef<'a>, text: Text<'a>) -> DrawnState<'a> {
    let caret_byte = node
        .data()
        .text_selection()
        .map_or(0, |s| text.clamp(s.focus.character_index));
    let Some(box_node) = node
        .children()
        .find(|c| c.role() == Role::GenericContainer && c.data().class_name() == Some(LINES_BOX_CLASS))
    else {
        return DrawnState {
            lines: Vec::new(),
            content_key: 0,
            upstream_caret: false,
            caret_byte,
        };
    };
    let (content_key, upstream_caret) = box_node
        .author_id()
        .and_then(|s| s.split_once(':'))
        .map_or((0, false), |(k, u)| (u64::from_str_radix(k, 16).unwrap_or(0), u == "1"));

    let mut raw = Vec::new();
    for child in box_node.children() {
        if child.role() != Role::GenericContainer || child.data().class_name() != Some(LINE_CLASS) {
            continue;
        }
        let Some((a, b)) = child.author_id().and_then(|id| id.split_once(':')) else {
            continue;
        };
        let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) else {
            continue;
        };
        let start = text.clamp(a);
        let end = text.clamp(b).max(start);
        raw.push(DrawnLine { start, end, node: child });
    }
    raw.sort_by_key(|d| (d.start, d.end));
    let mut lines: Vec<DrawnLine<'a>> = Vec::new();
    for d in raw {
        if let Some(prev) = lines.last()
            && (d.start < prev.end || (d.start == prev.start && d.end == prev.end))
        {
            continue;
        }
        lines.push(d);
    }
    DrawnState { lines, content_key, upstream_caret, caret_byte }
}

pub(crate) fn drawn_lines<'a>(node: &NodeRef<'a>, text: Text<'a>) -> Vec<DrawnLine<'a>> {
    drawn_state(node, text).lines
}

fn bands(drawn: &[DrawnLine]) -> Vec<Band> {
    let mut out: Vec<Band> = Vec::new();
    for (idx, d) in drawn.iter().enumerate() {
        if let Some(last) = out.last_mut()
            && last.end == d.start
        {
            last.end = d.end;
            last.count += 1;
            continue;
        }
        out.push(Band { start: d.start, end: d.end, first: idx, count: 1 });
    }
    out
}

fn line_is_rtl(line: &NodeRef) -> bool {
    line.data().text_direction() == Some(TextDirection::RightToLeft)
}

fn line_is_hard_end(text: Text, end: usize) -> bool {
    end > 0 && text.0.as_bytes().get(end - 1) == Some(&b'\n')
}

fn caret_x_in_line(line: &NodeRef, rel_byte: usize, trailing_edge: bool) -> f64 {
    let data = line.data();
    let rtl = line_is_rtl(line);
    let w = data.bounds().map_or(0.0, |r| r.width());
    if trailing_edge {
        return if rtl { 0.0 } else { w };
    }
    let lengths = data.character_lengths();
    let positions = data.character_positions().unwrap_or(&[]);
    let widths = data.character_widths().unwrap_or(&[]);
    if lengths.is_empty() || positions.len() < lengths.len() || widths.len() < lengths.len() {
        return if (rel_byte == 0) != rtl { 0.0 } else { w };
    }
    let mut seen = 0usize;
    for (k, &len) in lengths.iter().enumerate() {
        let next = seen + usize::from(len);
        if rel_byte < next || (rel_byte == seen && len == 0) {
            let x0 = f64::from(positions[k]);
            let x1 = x0 + f64::from(widths[k]);
            let cluster_rtl = if k + 1 < lengths.len() && widths[k] > 0.0 && widths[k + 1] > 0.0 {
                positions[k + 1] < positions[k]
            } else if k > 0 && widths[k] > 0.0 && widths[k - 1] > 0.0 {
                positions[k] < positions[k - 1]
            } else {
                rtl
            };
            return if cluster_rtl { x1 } else { x0 };
        }
        seen = next;
    }
    if rtl { 0.0 } else { w }
}

fn span_x_in_line(line: &NodeRef, rel_start: usize, rel_end: usize) -> (f64, f64) {
    let data = line.data();
    let w = data.bounds().map_or(0.0, |r| r.width());
    let lengths = data.character_lengths();
    let positions = data.character_positions().unwrap_or(&[]);
    let widths = data.character_widths().unwrap_or(&[]);
    if lengths.is_empty() || positions.len() < lengths.len() || widths.len() < lengths.len() {
        return (0.0, w);
    }
    let mut seen = 0usize;
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    for (k, &len) in lengths.iter().enumerate() {
        let next = seen + usize::from(len);
        if next > rel_start && seen < rel_end {
            let x0 = f64::from(positions[k]);
            let x1 = x0 + f64::from(widths[k]).max(0.0);
            min_x = min_x.min(x0);
            max_x = max_x.max(x1);
        }
        seen = next;
    }
    if min_x.is_finite() && max_x.is_finite() {
        (min_x, max_x.max(min_x))
    } else {
        let x = caret_x_in_line(line, rel_start, false);
        (x, x)
    }
}

/// Transformed bounding boxes (in root/window coordinates, matching AccessKit's `Range::bounding_boxes`)
/// for the drawn portion of `a..b`. Off-screen bytes have no box, matching Chromium.
pub(crate) fn bounding_boxes(node: &NodeRef, text: Text, a: usize, b: usize) -> Vec<Rect> {
    let (a, b) = (text.clamp(a.min(b)), text.clamp(a.max(b)));
    let state = drawn_state(node, text);
    let drawn = &state.lines;
    if drawn.is_empty() {
        return Vec::new();
    }
    if a == b {
        if state.upstream_caret && a == state.caret_byte {
            if let Some(d) = drawn
                .iter()
                .find(|d| a == d.end && d.start < d.end && !line_is_hard_end(text, d.end))
                && let Some(mut rect) = d.node.data().bounds()
            {
                let x = caret_x_in_line(&d.node, a - d.start, true);
                let left = rect.x0 + x;
                rect.x0 = left;
                rect.x1 = left;
                return vec![d.node.transform().transform_rect_bbox(rect)];
            }
        }
        let hit = drawn.iter().enumerate().find(|&(k, d)| {
            (d.start <= a && a < d.end)
                || (d.start == a && d.end == a)
                || (a == d.end
                    && !line_is_hard_end(text, d.end)
                    && drawn.get(k + 1).is_none_or(|nx| nx.start != a))
        });
        let Some((_, d)) = hit else {
            return Vec::new();
        };
        let Some(mut rect) = d.node.data().bounds() else {
            return Vec::new();
        };
        let x = caret_x_in_line(&d.node, a.saturating_sub(d.start), false);
        let left = rect.x0 + x;
        rect.x0 = left;
        rect.x1 = left;
        return vec![d.node.transform().transform_rect_bbox(rect)];
    }
    let mut out = Vec::new();
    for d in drawn {
        let ov_start = a.max(d.start);
        let ov_end = b.min(d.end);
        if ov_start >= ov_end {
            continue;
        }
        let Some(mut rect) = d.node.data().bounds() else {
            continue;
        };
        if ov_start > d.start || ov_end < d.end {
            let orig_left = rect.x0;
            let (x0, x1) = span_x_in_line(&d.node, ov_start - d.start, ov_end - d.start);
            rect.x0 = orig_left + x0;
            rect.x1 = orig_left + x1;
        }
        out.push(d.node.transform().transform_rect_bbox(rect));
    }
    out
}

/// Byte offset in `text` at `point` (in `node`'s local coordinate space), using containment inside
/// the nearest drawn visual line. When `for_char` is true (macOS `accessibilityRangeForPosition:`),
/// a point past the trailing edge of a soft-wrapped line clamps to that line's last character.
pub(crate) fn byte_at_point(node: &NodeRef, text: Text, point: Point, for_char: bool) -> Option<usize> {
    let drawn = drawn_lines(node, text);
    let d = drawn.iter().min_by(|a, b| {
        let dist = |ln: &DrawnLine| match ln.node.data().bounds() {
            Some(r) if point.y < r.y0 => r.y0 - point.y,
            Some(r) if point.y > r.y1 => point.y - r.y1,
            Some(_) => 0.0,
            None => f64::INFINITY,
        };
        dist(a).total_cmp(&dist(b))
    })?;
    let bounds = d.node.data().bounds()?;
    let rel_x = (point.x - bounds.x0) as f32;
    let data = d.node.data();
    let rtl = line_is_rtl(&d.node);
    let lengths = data.character_lengths();
    let positions = data.character_positions().unwrap_or(&[]);
    let widths = data.character_widths().unwrap_or(&[]);
    if lengths.is_empty() || positions.len() < lengths.len() || widths.len() < lengths.len() {
        return Some(d.start);
    }
    let mut byte_off = 0usize;
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    for k in 0..lengths.len() {
        let w = widths[k];
        let x0 = positions[k];
        let x1 = x0 + w;
        if w > 0.0 {
            min_x = min_x.min(x0);
            max_x = max_x.max(x1);
            if rel_x >= x0 && rel_x < x1 {
                return Some(text.clamp(d.start + byte_off));
            }
        }
        byte_off += usize::from(lengths[k]);
    }
    let trim = if d.end >= d.start + 2 && text.0.as_bytes().get(d.end - 2..d.end) == Some(b"\r\n") {
        2
    } else if d.end > d.start && text.0.as_bytes().get(d.end - 1) == Some(&b'\n') {
        1
    } else {
        0
    };
    let line_end = text.clamp(d.end.saturating_sub(trim)).max(d.start);
    let leading = if rtl { rel_x >= max_x } else { rel_x <= min_x };
    if leading {
        return Some(d.start);
    }
    if for_char && trim == 0 && line_end > d.start {
        return Some(text.prev(line_end, Unit::Character).max(d.start));
    }
    Some(line_end)
}

fn utf16_len(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().map(char::len_utf16).sum() }
}

fn count_newlines_slice(s: &str) -> usize {
    s.as_bytes().iter().filter(|&&b| b == b'\n').count()
}

fn count_newlines_between(s: &str, a: usize, b: usize, marks: &[Mark]) -> usize {
    if a >= b || b > s.len() {
        return 0;
    }
    if b - a <= CHECKPOINT_STRIDE || marks.len() <= 1 {
        return s.get(a..b).map_or(0, count_newlines_slice);
    }
    let ma = marks.partition_point(|m| m.byte <= a).saturating_sub(1);
    let mb = marks.partition_point(|m| m.byte <= b).saturating_sub(1);
    if mb <= ma {
        return s.get(a..b).map_or(0, count_newlines_slice);
    }
    let head_sub = s.get(marks[ma].byte..a).map_or(0, count_newlines_slice);
    let tail_add = s.get(marks[mb].byte..b).map_or(0, count_newlines_slice);
    (marks[mb].newlines + tail_add).saturating_sub(marks[ma].newlines + head_sub)
}

fn nth_newline_after(s: &str, from: usize, limit: usize, nth: usize, marks: &[Mark]) -> Option<usize> {
    let sub = s.get(from..limit)?;
    if limit - from <= CHECKPOINT_STRIDE || marks.len() <= 1 {
        return sub.match_indices('\n').nth(nth).map(|(p, _)| from + p);
    }
    let m_from = marks.partition_point(|m| m.byte <= from).saturating_sub(1);
    let base_nl = marks[m_from].newlines + s.get(marks[m_from].byte..from).map_or(0, count_newlines_slice);
    let target_nl = base_nl + nth + 1;
    let m_hit = marks.partition_point(|m| m.newlines < target_nl).saturating_sub(1);
    let scan_start = marks[m_hit].byte.max(from);
    if scan_start >= limit {
        return None;
    }
    let already = if scan_start == from {
        0
    } else {
        (marks[m_hit].newlines).saturating_sub(base_nl)
    };
    let rem = nth.checked_sub(already)?;
    s.get(scan_start..limit)?
        .match_indices('\n')
        .nth(rem)
        .map(|(p, _)| scan_start + p)
}

/// Byte offsets into a field's value.
#[derive(Clone, Copy)]
pub(crate) struct Text<'a>(pub(crate) &'a str);

impl<'a> Text<'a> {
    pub(crate) fn len(self) -> usize {
        self.0.len()
    }

    /// `i`, within the text and on a character's start.
    pub(crate) fn clamp(self, i: usize) -> usize {
        self.0.floor_char_boundary(i.min(self.0.len()))
    }

    pub(crate) fn slice(self, a: usize, b: usize) -> &'a str {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        self.0.get(a..b).unwrap_or("")
    }

    fn line_start(self, i: usize) -> usize {
        let i = self.clamp(i);
        self.0.get(..i).and_then(|s| s.rfind('\n')).map_or(0, |p| p + 1)
    }

    pub(crate) fn is_start(self, i: usize, unit: Unit) -> bool {
        let i = self.clamp(i);
        let s = self.0;
        match unit {
            Unit::Character => true,
            Unit::Word => {
                i == 0
                    || (s.get(i..).and_then(|r| r.chars().next()).is_some_and(|c| !c.is_whitespace())
                        && s.get(..i).and_then(|l| l.chars().next_back()).is_some_and(char::is_whitespace))
            }
            Unit::Line | Unit::Paragraph => i == 0 || s.as_bytes().get(i - 1) == Some(&b'\n'),
            Unit::Document => i == 0,
        }
    }

    pub(crate) fn is_start_in(self, i: usize, unit: Unit, state: &DrawnState) -> bool {
        let i = self.clamp(i);
        if unit == Unit::Line && !state.lines.is_empty() {
            if state.upstream_caret
                && i == state.caret_byte
                && state
                    .lines
                    .iter()
                    .any(|d| i == d.end && d.start < d.end && !line_is_hard_end(self, d.end))
            {
                return false;
            }
            if let Some(d) = state
                .lines
                .iter()
                .find(|d| (d.start <= i && i < d.end) || (d.start == i && d.end == i))
            {
                return i == d.start;
            }
        }
        self.is_start(i, unit)
    }

    /// The start of the unit holding `i`.
    pub(crate) fn start_of(self, i: usize, unit: Unit) -> usize {
        let i = self.clamp(i);
        if self.is_start(i, unit) { i } else { self.prev(i, unit) }
    }

    pub(crate) fn start_of_in(self, i: usize, unit: Unit, state: &DrawnState) -> usize {
        let i = self.clamp(i);
        if self.is_start_in(i, unit, state) {
            i
        } else {
            self.prev_in(i, unit, &state.lines)
        }
    }

    /// The first unit start after `i`, or the text's end.
    pub(crate) fn next(self, i: usize, unit: Unit) -> usize {
        let i = self.clamp(i);
        let s = self.0;
        let tail = s.get(i..).unwrap_or("");
        match unit {
            Unit::Character => tail.chars().next().map_or(i, |c| i + c.len_utf8()),
            Unit::Word => {
                let mut space = false;
                for (k, c) in tail.char_indices() {
                    if c.is_whitespace() {
                        space = true;
                    } else if space {
                        return i + k;
                    }
                }
                s.len()
            }
            Unit::Line | Unit::Paragraph => tail.find('\n').map_or(s.len(), |p| i + p + 1),
            Unit::Document => s.len(),
        }
    }

    pub(crate) fn next_in(self, i: usize, unit: Unit, drawn: &[DrawnLine]) -> usize {
        let i = self.clamp(i);
        if unit != Unit::Line || drawn.is_empty() {
            return self.next(i, unit);
        }
        if let Some(d) = drawn.iter().find(|d| d.start <= i && i < d.end) {
            return d.end;
        }
        let nl = self.next(i, Unit::Line);
        drawn
            .iter()
            .find(|d| d.start > i && d.start < nl)
            .map_or(nl, |d| d.start)
    }

    /// The last unit start before `i`, or the text's start.
    pub(crate) fn prev(self, i: usize, unit: Unit) -> usize {
        let i = self.clamp(i);
        let s = self.0;
        let head = s.get(..i).unwrap_or("");
        match unit {
            Unit::Character => head.chars().next_back().map_or(0, |c| i - c.len_utf8()),
            Unit::Word => {
                let mut word = None;
                for (k, c) in head.char_indices().rev() {
                    if !c.is_whitespace() {
                        word = Some(k);
                    } else if let Some(k) = word {
                        return k;
                    }
                }
                0
            }
            Unit::Line | Unit::Paragraph => {
                if i == 0 {
                    return 0;
                }
                let at = self.line_start(i);
                if at < i {
                    at
                } else {
                    self.line_start(self.clamp(i - 1))
                }
            }
            Unit::Document => 0,
        }
    }

    pub(crate) fn prev_in(self, i: usize, unit: Unit, drawn: &[DrawnLine]) -> usize {
        let i = self.clamp(i);
        if unit != Unit::Line || drawn.is_empty() || i == 0 {
            return self.prev(i, unit);
        }
        if let Some(d) = drawn.iter().rfind(|d| d.start < i && i <= d.end) {
            return d.start;
        }
        if drawn.iter().any(|d| d.start == i) {
            if let Some(prev_d) = drawn.iter().rfind(|d| d.end == i && d.start < i) {
                return prev_d.start;
            }
            return self.prev(i, Unit::Line);
        }
        let p = self.prev(i, Unit::Line);
        drawn
            .iter()
            .rfind(|d| d.start >= p && d.start < i)
            .map_or(p, |d| d.start)
    }

    /// Moves `i` by `count` unit starts, stopping at either end of the text.
    pub(crate) fn step_in(self, i: usize, unit: Unit, count: i32, drawn: &[DrawnLine]) -> (usize, i32) {
        let mut i = self.clamp(i);
        if count == 0 {
            return (i, 0);
        }
        let mut moved = 0i32;
        let steps = count.unsigned_abs();
        for _ in 0..steps {
            if (count > 0 && i == self.len()) || (count < 0 && i == 0) {
                break;
            }
            let next_i = if count > 0 {
                self.next_in(i, unit, drawn)
            } else {
                self.prev_in(i, unit, drawn)
            };
            if next_i == i {
                break;
            }
            i = next_i;
            moved = moved.saturating_add(1);
        }
        (i, if count < 0 { -moved } else { moved })
    }

    /// The zero-based line holding byte `i`, counting drawn wrapped lines visually.
    pub(crate) fn line_of_in(self, i: usize, state: &DrawnState) -> usize {
        let i = self.clamp(i);
        let s = self.0;
        let drawn = &state.lines;
        let bs = bands(drawn);
        with_checkpoints(s, state.content_key, |marks| {
            if bs.is_empty() {
                return count_newlines_between(s, 0, i, marks);
            }
            let mut lines = 0usize;
            let mut cursor = 0usize;
            for b in &bs {
                if i < b.start {
                    return lines + count_newlines_between(s, cursor, i, marks);
                }
                lines += count_newlines_between(s, cursor, b.start, marks);
                let band_slice = &drawn[b.first..b.first + b.count];
                let has_trailing_empty =
                    band_slice.last().is_some_and(|d| d.start == b.end && d.end == b.end);
                let in_band_at_end = i == b.end
                    && b.end == s.len()
                    && (has_trailing_empty || !line_is_hard_end(self, b.end));
                if i < b.end
                    || in_band_at_end
                    || (state.upstream_caret
                        && i == state.caret_byte
                        && i == b.end
                        && b.start < b.end
                        && !line_is_hard_end(self, b.end))
                {
                    if state.upstream_caret && i == state.caret_byte {
                        if let Some(sub) = band_slice
                            .iter()
                            .position(|d| i == d.end && d.start < d.end && !line_is_hard_end(self, d.end))
                        {
                            return lines + sub;
                        }
                    }
                    let sub = band_slice.iter().rposition(|d| d.start <= i).unwrap_or(0);
                    return lines + sub;
                }
                lines += b.count;
                cursor = b.end;
            }
            lines + count_newlines_between(s, cursor, i, marks)
        })
    }

    /// Line `n`'s bytes, counting drawn wrapped lines visually.
    pub(crate) fn line_in(self, n: usize, state: &DrawnState) -> Option<(usize, usize)> {
        let s = self.0;
        let drawn = &state.lines;
        let bs = bands(drawn);
        with_checkpoints(s, state.content_key, |marks| {
            if bs.is_empty() {
                let start = if n == 0 {
                    0
                } else {
                    nth_newline_after(s, 0, s.len(), n - 1, marks)? + 1
                };
                return Some((start, self.next(start, Unit::Line)));
            }
            let mut rem = n;
            let mut cursor = 0usize;
            for b in &bs {
                let gap_lines = count_newlines_between(s, cursor, b.start, marks);
                if rem < gap_lines {
                    let start = if rem == 0 {
                        cursor
                    } else {
                        nth_newline_after(s, cursor, b.start, rem - 1, marks)? + 1
                    };
                    return Some((start, self.next(start, Unit::Line).min(b.start)));
                }
                rem -= gap_lines;
                if rem < b.count {
                    let d = &drawn[b.first + rem];
                    return Some((d.start, d.end));
                }
                rem -= b.count;
                cursor = b.end;
            }
            let drawn_has_end_empty =
                drawn.last().is_some_and(|d| d.start == s.len() && d.end == s.len());
            if cursor >= s.len() {
                if !drawn_has_end_empty && line_is_hard_end(self, s.len()) && rem == 0 {
                    return Some((s.len(), s.len()));
                }
                return None;
            }
            let start = if rem == 0 {
                cursor
            } else {
                nth_newline_after(s, cursor, s.len(), rem - 1, marks)? + 1
            };
            Some((start, self.next(start, Unit::Line)))
        })
    }

    pub(crate) fn utf16_with(self, i: usize, key: u64) -> usize {
        let i = self.clamp(i);
        with_checkpoints(self.0, key, |marks| {
            let idx = marks.partition_point(|m| m.byte <= i).saturating_sub(1);
            let m = marks.get(idx).copied().unwrap_or(Mark { byte: 0, utf16: 0, newlines: 0 });
            m.utf16 + self.0.get(m.byte..i).map_or(0, utf16_len)
        })
    }

    pub(crate) fn utf16(self, i: usize) -> usize {
        self.utf16_with(i, 0)
    }

    /// Bytes `a..b` as a UTF-16 start and length.
    pub(crate) fn utf16_span_with(self, a: usize, b: usize, key: u64) -> (usize, usize) {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        let start = self.utf16_with(a, key);
        let len = if b - a <= CHECKPOINT_STRIDE {
            self.0.get(a..b).map_or(0, utf16_len)
        } else {
            self.utf16_with(b, key).saturating_sub(start)
        };
        (start, len)
    }

    pub(crate) fn utf16_span(self, a: usize, b: usize) -> (usize, usize) {
        self.utf16_span_with(a, b, 0)
    }

    pub(crate) fn from_utf16_with(self, units: usize, key: u64) -> usize {
        with_checkpoints(self.0, key, |marks| {
            let idx = marks.partition_point(|m| m.utf16 <= units).saturating_sub(1);
            let m = marks.get(idx).copied().unwrap_or(Mark { byte: 0, utf16: 0, newlines: 0 });
            let mut seen = m.utf16;
            let tail = self.0.get(m.byte..).unwrap_or("");
            for (k, c) in tail.char_indices() {
                if seen >= units {
                    return m.byte + k;
                }
                seen += c.len_utf16();
            }
            self.0.len()
        })
    }

    pub(crate) fn from_utf16(self, units: usize) -> usize {
        self.from_utf16_with(units, 0)
    }
}
