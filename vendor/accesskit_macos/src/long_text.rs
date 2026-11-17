// Rune (vendor/accesskit_macos.patch): a text field too long for its toolkit to publish text
// runs (Rune stops at 64 KiB) keeps its text ranges. The toolkit marks such a field by putting its
// selection on the field itself, as byte offsets into its value, and publishes one GenericContainer
// child (`rune-line:<start>:<end>`) per drawn visual line. Ranges are answered from the value when
// a reader asks, and screen rectangles, hit-testing and wrapped visual lines are answered in the
// drawn band without shaping off-screen rows.
// The same file is in accesskit_windows and accesskit_macos; each uses part of it.
#![allow(dead_code)]

use accesskit::{Point, Rect, TextSelection};
use accesskit_consumer::NodeRef;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unit {
    Character,
    Word,
    Line,
    Document,
}

/// One visual line currently drawn on screen for a long field.
#[derive(Clone, Copy)]
pub(crate) struct DrawnLine<'a> {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) node: NodeRef<'a>,
}

struct Band {
    start: usize,
    end: usize,
    first: usize,
    count: usize,
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

/// Drawn visual lines Rune attached to `node` (`rune-line:<start>:<end>`), in document order.
pub(crate) fn drawn_lines<'a>(node: &NodeRef<'a>, text: Text<'a>) -> Vec<DrawnLine<'a>> {
    let mut out = Vec::new();
    for child in node.children() {
        let Some(rest) = child.author_id().and_then(|id| id.strip_prefix("rune-line:")) else {
            continue;
        };
        let Some((a, b)) = rest.split_once(':') else {
            continue;
        };
        let (Ok(a), Ok(b)) = (a.parse::<usize>(), b.parse::<usize>()) else {
            continue;
        };
        let start = text.clamp(a);
        let end = text.clamp(b).max(start);
        out.push(DrawnLine { start, end, node: child });
    }
    out.sort_by_key(|d| (d.start, d.end));
    out
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

fn char_x_in_line(line: &NodeRef, rel_byte: usize, end_edge: bool) -> (f64, f64) {
    let data = line.data();
    let lengths = data.character_lengths();
    let positions = data.character_positions().unwrap_or(&[]);
    let widths = data.character_widths().unwrap_or(&[]);
    if lengths.is_empty() || positions.len() < lengths.len() || widths.len() < lengths.len() {
        let w = data.bounds().map_or(0.0, |r| r.width());
        let x = if rel_byte == 0 && !end_edge { 0.0 } else { w };
        return (x, x);
    }
    let mut seen = 0usize;
    for (k, &len) in lengths.iter().enumerate() {
        let next = seen + usize::from(len);
        if rel_byte < next || (rel_byte == seen && !end_edge) {
            let x0 = f64::from(positions[k]);
            let x1 = x0 + f64::from(widths[k]);
            return (x0, x1);
        }
        seen = next;
    }
    let last = lengths.len() - 1;
    let end = f64::from(positions[last] + widths[last]);
    (end, end)
}

/// Transformed bounding boxes (in root/window coordinates, matching AccessKit's `Range::bounding_boxes`)
/// for the drawn portion of `a..b`. Off-screen bytes have no box, matching Chromium.
pub(crate) fn bounding_boxes(node: &NodeRef, text: Text, a: usize, b: usize) -> Vec<Rect> {
    let (a, b) = (text.clamp(a.min(b)), text.clamp(a.max(b)));
    let drawn = drawn_lines(node, text);
    if drawn.is_empty() {
        return Vec::new();
    }
    if a == b {
        let hit = drawn.iter().enumerate().find(|&(k, d)| {
            (d.start <= a && a < d.end)
                || (d.start == a && d.end == a)
                || (a == d.end && (a == text.len() || drawn.get(k + 1).is_none_or(|nx| nx.start != a)))
        });
        let Some((_, d)) = hit else {
            return Vec::new();
        };
        let Some(mut rect) = d.node.data().bounds() else {
            return Vec::new();
        };
        let (x, _) = char_x_in_line(&d.node, a.saturating_sub(d.start), false);
        let left = rect.x0 + x;
        rect.x0 = left;
        rect.x1 = left;
        return vec![d.node.transform().transform_rect_bbox(rect)];
    }
    let mut out = Vec::new();
    for d in &drawn {
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
            let (x0, _) = char_x_in_line(&d.node, ov_start - d.start, false);
            let (_, x1) = char_x_in_line(&d.node, ov_end.saturating_sub(1) - d.start, true);
            rect.x0 = orig_left + x0;
            rect.x1 = orig_left + x1.max(x0);
        }
        out.push(d.node.transform().transform_rect_bbox(rect));
    }
    out
}

/// Byte offset in `text` at `point` (in `node`'s local coordinate space), when a drawn line covers it.
pub(crate) fn byte_at_point(node: &NodeRef, text: Text, point: Point) -> Option<usize> {
    let drawn = drawn_lines(node, text);
    let d = drawn.iter().find(|d| {
        d.node
            .data()
            .bounds()
            .is_some_and(|r| point.y >= r.y0 && point.y <= r.y1)
    })?;
    let bounds = d.node.data().bounds()?;
    let rel_x = (point.x - bounds.x0) as f32;
    let data = d.node.data();
    let lengths = data.character_lengths();
    let positions = data.character_positions().unwrap_or(&[]);
    let widths = data.character_widths().unwrap_or(&[]);
    if lengths.is_empty() || positions.len() < lengths.len() || widths.len() < lengths.len() {
        return Some(d.start);
    }
    let mut byte_off = 0usize;
    for k in 0..lengths.len() {
        let mid = positions[k] + widths[k] * 0.5;
        if rel_x <= mid {
            return Some(text.clamp(d.start + byte_off));
        }
        byte_off += usize::from(lengths[k]);
    }
    let trim_nl = usize::from(d.end > d.start && text.0.as_bytes().get(d.end - 1) == Some(&b'\n'));
    Some(text.clamp(d.end.saturating_sub(trim_nl)))
}

fn utf16_len(s: &str) -> usize {
    if s.is_ascii() { s.len() } else { s.chars().map(char::len_utf16).sum() }
}

fn count_newlines(s: &str) -> usize {
    s.as_bytes().iter().filter(|&&b| b == b'\n').count()
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
        &self.0[a..b]
    }

    fn line_start(self, i: usize) -> usize {
        self.0[..i].rfind('\n').map_or(0, |p| p + 1)
    }

    pub(crate) fn is_start(self, i: usize, unit: Unit) -> bool {
        let s = self.0;
        match unit {
            Unit::Character => true,
            Unit::Word => {
                i == 0
                    || (s[i..].chars().next().is_some_and(|c| !c.is_whitespace())
                        && s[..i].chars().next_back().is_some_and(char::is_whitespace))
            }
            Unit::Line => i == 0 || s.as_bytes()[i - 1] == b'\n',
            Unit::Document => i == 0,
        }
    }

    pub(crate) fn is_start_in(self, i: usize, unit: Unit, drawn: &[DrawnLine]) -> bool {
        if unit == Unit::Line && !drawn.is_empty() {
            if let Some(d) = drawn.iter().find(|d| (d.start <= i && i < d.end) || (d.start == i && d.end == i)) {
                return i == d.start;
            }
        }
        self.is_start(i, unit)
    }

    /// The start of the unit holding `i`.
    pub(crate) fn start_of(self, i: usize, unit: Unit) -> usize {
        if self.is_start(i, unit) { i } else { self.prev(i, unit) }
    }

    pub(crate) fn start_of_in(self, i: usize, unit: Unit, drawn: &[DrawnLine]) -> usize {
        if self.is_start_in(i, unit, drawn) { i } else { self.prev_in(i, unit, drawn) }
    }

    /// The first unit start after `i`, or the text's end.
    pub(crate) fn next(self, i: usize, unit: Unit) -> usize {
        let s = self.0;
        match unit {
            Unit::Character => s[i..].chars().next().map_or(i, |c| i + c.len_utf8()),
            Unit::Word => {
                let mut space = false;
                for (k, c) in s[i..].char_indices() {
                    if c.is_whitespace() {
                        space = true;
                    } else if space {
                        return i + k;
                    }
                }
                s.len()
            }
            Unit::Line => s[i..].find('\n').map_or(s.len(), |p| i + p + 1),
            Unit::Document => s.len(),
        }
    }

    pub(crate) fn next_in(self, i: usize, unit: Unit, drawn: &[DrawnLine]) -> usize {
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
        let s = self.0;
        match unit {
            Unit::Character => s[..i].chars().next_back().map_or(0, |c| i - c.len_utf8()),
            Unit::Word => {
                let mut word = None;
                for (k, c) in s[..i].char_indices().rev() {
                    if !c.is_whitespace() {
                        word = Some(k);
                    } else if let Some(k) = word {
                        return k;
                    }
                }
                0
            }
            Unit::Line => {
                if i == 0 {
                    return 0;
                }
                let at = self.line_start(i);
                if at < i { at } else { self.line_start(i - 1) }
            }
            Unit::Document => 0,
        }
    }

    pub(crate) fn prev_in(self, i: usize, unit: Unit, drawn: &[DrawnLine]) -> usize {
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
            return self.line_start(i - 1);
        }
        let p = self.prev(i, Unit::Line);
        drawn
            .iter()
            .rfind(|d| d.start >= p && d.start < i)
            .map_or(p, |d| d.start)
    }

    /// Moves `i` by `count` unit starts (a unit ends where the next starts), stopping at either
    /// end of the text. Returns where it got to and how many units it moved.
    pub(crate) fn step_in(self, mut i: usize, unit: Unit, count: i32, drawn: &[DrawnLine]) -> (usize, i32) {
        let mut moved = 0;
        for _ in 0..count.unsigned_abs() {
            if (count > 0 && i == self.len()) || (count < 0 && i == 0) {
                break;
            }
            i = if count > 0 {
                self.next_in(i, unit, drawn)
            } else {
                self.prev_in(i, unit, drawn)
            };
            moved += 1;
        }
        (i, if count < 0 { -moved } else { moved })
    }

    /// The zero-based line holding byte `i`, counting drawn wrapped lines visually.
    pub(crate) fn line_of_in(self, i: usize, drawn: &[DrawnLine]) -> usize {
        let s = self.0;
        let bs = bands(drawn);
        if bs.is_empty() {
            return count_newlines(&s[..i]);
        }
        let mut lines = 0usize;
        let mut cursor = 0usize;
        for b in &bs {
            if i < b.start {
                return lines + count_newlines(&s[cursor..i]);
            }
            lines += count_newlines(&s[cursor..b.start]);
            if i < b.end || (i == b.end && b.end == s.len() && (b.start == b.end || s.as_bytes()[b.end - 1] != b'\n')) {
                let sub = drawn[b.first..b.first + b.count]
                    .iter()
                    .rposition(|d| d.start <= i)
                    .unwrap_or(0);
                return lines + sub;
            }
            lines += b.count;
            cursor = b.end;
        }
        lines + count_newlines(&s[cursor..i])
    }

    /// Line `n`'s bytes, counting drawn wrapped lines visually.
    pub(crate) fn line_in(self, n: usize, drawn: &[DrawnLine]) -> Option<(usize, usize)> {
        let s = self.0;
        let bs = bands(drawn);
        if bs.is_empty() {
            let start = if n == 0 { 0 } else { s.match_indices('\n').nth(n - 1)?.0 + 1 };
            return Some((start, self.next(start, Unit::Line)));
        }
        let mut rem = n;
        let mut cursor = 0usize;
        for b in &bs {
            let gap_lines = count_newlines(&s[cursor..b.start]);
            if rem < gap_lines {
                let start = if rem == 0 {
                    cursor
                } else {
                    cursor + s[cursor..b.start].match_indices('\n').nth(rem - 1)?.0 + 1
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
        if cursor == s.len() && (s.is_empty() || s.as_bytes()[s.len() - 1] != b'\n') {
            return None;
        }
        let start = if rem == 0 {
            cursor
        } else {
            cursor + s[cursor..].match_indices('\n').nth(rem - 1)?.0 + 1
        };
        Some((start, self.next(start, Unit::Line)))
    }

    pub(crate) fn utf16(self, i: usize) -> usize {
        utf16_len(&self.0[..i])
    }

    /// Bytes `a..b` as a UTF-16 start and length.
    pub(crate) fn utf16_span(self, a: usize, b: usize) -> (usize, usize) {
        let (a, b) = (self.clamp(a.min(b)), self.clamp(a.max(b)));
        (self.utf16(a), utf16_len(&self.0[a..b]))
    }

    pub(crate) fn from_utf16(self, units: usize) -> usize {
        if self.0.is_ascii() {
            return units.min(self.0.len());
        }
        let mut seen = 0;
        for (k, c) in self.0.char_indices() {
            if seen >= units {
                return k;
            }
            seen += c.len_utf16();
        }
        self.0.len()
    }
}
