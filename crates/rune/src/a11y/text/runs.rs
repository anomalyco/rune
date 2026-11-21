//! Laid-out text runs and visual lines for accessibility: extracting per-character lengths,
//! positions, widths and word starts from Parley layouts, and mapping between drawn byte offsets
//! and `(line, character)` indices.

use crate::text::TextFlow;

/// One laid-out run or visual line: its bytes in the drawn text and each character's place in points.
pub(crate) struct Line {
    pub range: std::ops::Range<usize>,
    pub rect: [f32; 4],
    pub lengths: Vec<u8>,
    pub positions: Vec<f32>,
    pub widths: Vec<f32>,
    pub word_starts: Vec<u8>,
    pub rtl: bool,
    pub continues_line: bool,
}

/// `content`'s BiDi-split `TextRun`s from `layout` (device px at `s`), placed from `origin` in points.
/// Each visual line splits at directional boundaries (`RightToLeft` runs measure positions from the
/// run's right edge) and links sibling runs of the same visual line with `continues_line`.
pub(crate) fn lines(layout: TextFlow<'_>, content: &str, s: f32, origin: [f32; 2]) -> Vec<Line> {
    let mut out = Vec::new();
    let waiting = match layout {
        TextFlow::Paragraphs(pl) => pl.waiting_ascii(),
        TextFlow::Single(_) => None,
    };
    let mut y_acc = 0.0f64;
    for p in layout.pieces() {
        if !p.shaped {
            let Some(ascii) = waiting else { break };
            let (mut at, mut ok) = (p.start, true);
            for raw in content.get(p.start..p.end).unwrap_or_default().split('\n') {
                let words = if at + raw.len() < p.end { raw.strip_suffix('\r').unwrap_or(raw) } else { raw };
                let (ls, le, py) = (at, at + words.len(), y_acc.round() as f32);
                at += raw.len() + 1;
                y_acc += f64::from(ascii.height);
                if !push_waiting_ascii_line(words, (ls, le), py, ascii, content, s, origin, &mut out) {
                    ok = false;
                    break;
                }
            }
            if !ok { break; }
        } else {
            push_piece_runs(p, content, s, origin, &mut out);
            y_acc += f64::from(p.height);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn push_waiting_ascii_line(
    words: &str, (line_start, line_end): (usize, usize), py: f32, ascii: &crate::text::AsciiLine,
    content: &str, s: f32, origin: [f32; 2], out: &mut Vec<Line>,
) -> bool {
    let n = words.len();
    let (mut lengths, mut positions, mut widths) = (Vec::with_capacity(n + 1), Vec::with_capacity(n + 1), Vec::with_capacity(n + 1));
    let (mut cx, mut max_x) = (0.0f32, 0.0f32);
    for b in words.bytes() {
        if !(0x20..0x7f).contains(&b) { return false; }
        let adv = ascii.advance[(b - 0x20) as usize];
        let (vis_left, vis_w) = (cx / s, adv / s);
        max_x = max_x.max(vis_left + vis_w.max(0.0));
        lengths.push(1);
        positions.push(vis_left.max(0.0));
        widths.push(vis_w);
        cx += adv;
    }
    let word_starts = rune_text::segmenter::non_complex_word_starts(words, n);
    let br = if content.as_bytes().get(line_end..line_end + 2) == Some(b"\r\n") {
        2
    } else {
        usize::from(content.as_bytes().get(line_end) == Some(&b'\n'))
    };
    if br > 0 {
        lengths.push(br as u8);
        positions.push(max_x);
        widths.push(0.0);
    }
    out.push(Line {
        range: line_start..(line_end + br),
        rect: [origin[0], origin[1] + py / s, max_x, ascii.height / s],
        lengths, positions, widths, word_starts, rtl: false, continues_line: false,
    });
    true
}

pub(super) fn push_span_chunks(
    content: &str,
    span: std::ops::Range<usize>,
    pos: f32,
    width: f32,
    lengths: &mut Vec<u8>,
    positions: &mut Vec<f32>,
    widths: &mut Vec<f32>,
) {
    let mut cur = span.start.min(content.len());
    let end = span.end.min(content.len());
    let mut first = true;
    while cur < end {
        let rem = end - cur;
        let step = if rem <= 255 {
            rem
        } else {
            let cut = content.floor_char_boundary(cur + 255);
            if cut > cur { cut - cur } else { rem.min(255) }
        };
        lengths.push(step as u8);
        if first {
            positions.push(pos);
            widths.push(width);
            first = false;
        } else {
            positions.push(pos + width);
            widths.push(0.0);
        }
        cur += step;
    }
}

pub(super) fn build_visual_line(
    p: &crate::text::Piece<'_>,
    line: parley::layout::Line<'_, crate::text::Ink>,
    content: &str,
    s: f32,
    origin: [f32; 2],
) -> Option<Line> {
    let m = line.metrics();
    let range = p.content(line.text_range().start)..p.content(line.text_range().end);
    if range.end > content.len() { return None; }
    let mut raw_clusters = Vec::new();
    let line_rtl = p.flow == crate::el::Flow::Rtl;
    let mut max_right = m.offset;
    for run in line.runs() {
        let mut cx = m.inline_min_coord
            + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset);
        for cl in run.visual_clusters() {
            let adv = cl.advance();
            let r = cl.text_range();
            let r = p.content(r.start)..p.content(r.end);
            if r.start < range.end && r.end <= range.end && !r.is_empty() {
                raw_clusters.push((r, (cx - m.offset) / s, adv / s, cl.is_word_boundary()));
            }
            cx += adv;
            max_right = max_right.max(cx);
        }
    }
    raw_clusters.sort_by_key(|(r, _, _, _)| r.start);
    let (mut lengths, mut positions, mut widths, mut word_starts) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut at = range.start;
    for (r, vis_pos, vis_w, wb) in raw_clusters {
        if r.end <= at { continue; }
        let start = r.start.max(at);
        if start > at {
            // Bytes a cluster skipped (an ignorable or BiDi mark) join the character before when
            // they fit in its u8 length, avoiding an extra zero-width character stop.
            let gap = start - at;
            if let Some(last) = lengths.last_mut().filter(|l| usize::from(**l) + gap <= 255) {
                *last += gap as u8;
            } else {
                let prev_end = positions.last().zip(widths.last()).map_or(0.0, |(&px, &pw)| px + pw);
                push_span_chunks(content, at..start, prev_end, 0.0, &mut lengths, &mut positions, &mut widths);
            }
        }
        if wb && !lengths.is_empty() && lengths.len() < 256 {
            word_starts.push(lengths.len() as u8);
        }
        push_span_chunks(content, start..r.end, vis_pos, vis_w, &mut lengths, &mut positions, &mut widths);
        at = r.end;
    }
    if at < range.end {
        let end_x = (max_right - m.offset).max(0.0) / s;
        push_span_chunks(content, at..range.end, end_x, 0.0, &mut lengths, &mut positions, &mut widths);
    }
    word_starts.retain(|&w| (w as usize) < lengths.len());
    if !m.block_min_coord.is_finite() || !m.block_max_coord.is_finite() || m.block_max_coord < m.block_min_coord {
        return None;
    }
    let top = (p.y + m.block_min_coord) / s;
    let height = (m.block_max_coord - m.block_min_coord) / s;
    let rect = [origin[0] + m.offset / s, origin[1] + top, (max_right - m.offset).max(0.0) / s, height];
    Some(Line { range, rect, lengths, positions, widths, word_starts, rtl: line_rtl, continues_line: false })
}

fn push_bidi_line_runs(
    p: &crate::text::Piece<'_>,
    line: parley::layout::Line<'_, crate::text::Ink>,
    li: usize,
    content: &str,
    s: f32,
    origin: [f32; 2],
    out: &mut Vec<Line>,
) -> bool {
    let m = line.metrics();
    if !m.block_min_coord.is_finite() || !m.block_max_coord.is_finite() || m.block_max_coord < m.block_min_coord {
        return false;
    }
    let range = p.content(line.text_range().start)..p.content(line.text_range().end);
    if range.end > content.len() { return false; }
    let top = (p.y + m.block_min_coord) / s;
    let height = (m.block_max_coord - m.block_min_coord) / s;
    let line_x0 = origin[0] + m.offset / s;
    let line_y0 = origin[1] + top;

    let mut raw_clusters = Vec::new();
    let mut max_right = m.offset;
    for run in line.runs() {
        let mut cx = m.inline_min_coord
            + run.visual_clusters().next().and_then(|c| c.visual_offset()).unwrap_or(m.offset);
        for cl in run.visual_clusters() {
            let adv = cl.advance();
            let r = cl.text_range();
            let r = p.content(r.start)..p.content(r.end);
            if r.start < range.end && r.end <= range.end && !r.is_empty() {
                raw_clusters.push((r, (cx - m.offset) / s, adv / s, cl.is_word_boundary(), cl.is_rtl()));
            }
            cx += adv;
            max_right = max_right.max(cx);
        }
    }
    raw_clusters.sort_by_key(|(r, _, _, _, _)| r.start);

    if raw_clusters.is_empty() {
        let (mut lengths, mut positions, mut widths) = (Vec::new(), Vec::new(), Vec::new());
        if !range.is_empty() {
            push_span_chunks(content, range.clone(), 0.0, 0.0, &mut lengths, &mut positions, &mut widths);
        }
        out.push(Line {
            range,
            rect: [line_x0, line_y0, (max_right - m.offset).max(0.0) / s, height],
            lengths,
            positions,
            widths,
            word_starts: Vec::new(),
            rtl: p.flow == crate::el::Flow::Rtl,
            continues_line: false,
        });
        return true;
    }

    let mut cur_byte = range.start;
    let mut si = 0usize;
    let mut idx = 0usize;
    while idx < raw_clusters.len() {
        let run_rtl = raw_clusters[idx].4;
        let mut end_idx = idx + 1;
        while end_idx < raw_clusters.len() && raw_clusters[end_idx].4 == run_rtl {
            end_idx += 1;
        }
        let slice = &raw_clusters[idx..end_idx];
        let is_last_slice = end_idx == raw_clusters.len();
        let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
        for &(_, vis_left, vis_w, _, _) in slice {
            min_x = min_x.min(vis_left);
            max_x = max_x.max(vis_left + vis_w.max(0.0));
        }
        if !min_x.is_finite() { min_x = 0.0; max_x = 0.0; }
        let run_w = (max_x - min_x).max(0.0);

        let run_start = cur_byte;
        let (mut lengths, mut positions, mut widths, mut word_starts) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (r, vis_left, vis_w, wb, _) in slice {
            if r.end <= cur_byte { continue; }
            let pos = if run_rtl {
                (max_x - (vis_left + vis_w.max(0.0))).max(0.0)
            } else {
                (vis_left - min_x).max(0.0)
            };
            let mut span_start = r.start.max(cur_byte);
            if span_start > cur_byte {
                let gap = span_start - cur_byte;
                if let Some(last) = lengths.last_mut().filter(|l| usize::from(**l) + gap <= 255) {
                    *last += gap as u8;
                } else if lengths.is_empty() && r.end - cur_byte <= 255 {
                    span_start = cur_byte;
                } else {
                    let prev_end = positions.last().zip(widths.last()).map_or(0.0, |(&px, &pw)| px + pw);
                    push_span_chunks(content, cur_byte..span_start, prev_end, 0.0, &mut lengths, &mut positions, &mut widths);
                }
            }
            if *wb && (si > 0 || li > 0 || !lengths.is_empty()) && lengths.len() < 256 {
                word_starts.push(lengths.len() as u8);
            }
            push_span_chunks(content, span_start..r.end, pos, *vis_w, &mut lengths, &mut positions, &mut widths);
            cur_byte = r.end;
        }
        if is_last_slice && cur_byte < range.end {
            let tail = range.end - cur_byte;
            if let Some(last) = lengths.last_mut().filter(|l| usize::from(**l) + tail <= 255) {
                *last += tail as u8;
            } else {
                push_span_chunks(content, cur_byte..range.end, run_w, 0.0, &mut lengths, &mut positions, &mut widths);
            }
            cur_byte = range.end;
        }
        word_starts.retain(|&w| (w as usize) < lengths.len());
        out.push(Line {
            range: run_start..cur_byte,
            rect: [line_x0 + min_x, line_y0, run_w, height],
            lengths,
            positions,
            widths,
            word_starts,
            rtl: run_rtl,
            continues_line: si > 0,
        });
        si += 1;
        idx = end_idx;
    }
    true
}

fn push_piece_runs(
    p: crate::text::Piece<'_>,
    content: &str,
    s: f32,
    origin: [f32; 2],
    out: &mut Vec<Line>,
) {
    let first = out.len();
    if !p.is_blank() {
        for (li, ln) in p.layout.lines().enumerate() {
            if !push_bidi_line_runs(&p, ln, li, content, s, origin, out) {
                break;
            }
        }
    }
    if !p.is_whole() && out.len() == first {
        out.push(Line {
            range: p.start..p.start,
            rect: [origin[0], origin[1] + p.y / s, 0.0, p.height / s],
            lengths: Vec::new(),
            positions: Vec::new(),
            widths: Vec::new(),
            word_starts: Vec::new(),
            rtl: p.flow == crate::el::Flow::Rtl,
            continues_line: false,
        });
    }
    finish_piece_newline(&p, content, first, true, true, out);
}

fn finish_piece_newline(
    p: &crate::text::Piece<'_>,
    content: &str,
    first: usize,
    included_last: bool,
    right_edge_rtl: bool,
    out: &mut Vec<Line>,
) {
    if p.is_whole() || !included_last { return; }
    let br = if content.as_bytes().get(p.end..p.end + 2) == Some(b"\r\n") {
        2
    } else if content.as_bytes().get(p.end) == Some(&b'\n') {
        1
    } else {
        return;
    };
    // Parley treats a `\r` at the end of a `\n`-split paragraph (`CRLF`) as a forced break that
    // adds a trailing empty `p.end..p.end` visual line; drop that spurious empty line before `\n`.
    if p.end > p.start
        && content.as_bytes().get(p.end - 1) == Some(&b'\r')
        && out.len() > first + 1
        && out.last().is_some_and(|l| l.range == (p.end..p.end))
    {
        out.pop();
    }
    if let Some(l) = out.last_mut() {
        l.range.end += br;
        l.lengths.push(br as u8);
        // On BiDi-split `TextRun`s (`right_edge_rtl = true`), RTL positions are measured from
        // `rect.x1` leftward so logical end is at `l.rect[2]`; on long-field `\x1frune-line`s
        // (`right_edge_rtl = false`), positions are measured from `rect.x0` rightward (`0.0`).
        l.positions.push(if l.rtl && !right_edge_rtl { 0.0 } else { l.rect[2] });
        l.widths.push(0.0);
    }
}

pub(super) fn push_piece_lines_in(
    p: crate::text::Piece<'_>,
    content: &str,
    s: f32,
    origin: [f32; 2],
    band: Option<(f32, f32)>,
    max_out: usize,
    out: &mut Vec<Line>,
) {
    if out.len() >= max_out { return; }
    let first = out.len();
    if p.is_blank() {
        if band.is_none_or(|(top_px, bot_px)| p.y + p.height >= top_px && p.y <= bot_px) {
            out.push(Line {
                range: p.start..p.start,
                rect: [origin[0], origin[1] + p.y / s, 0.0, p.height / s],
                lengths: Vec::new(),
                positions: Vec::new(),
                widths: Vec::new(),
                word_starts: Vec::new(),
                rtl: p.flow == crate::el::Flow::Rtl,
                continues_line: false,
            });
            finish_piece_newline(&p, content, first, true, false, out);
        }
        return;
    }
    let total = p.layout.len();
    let lo = match band {
        Some((top_px, _)) => {
            let (mut l, mut h) = (0usize, total);
            while l < h {
                let mid = l + (h - l) / 2;
                if p.layout.get(mid).is_some_and(|ln| p.y + ln.metrics().block_max_coord < top_px) {
                    l = mid + 1;
                } else {
                    h = mid;
                }
            }
            l
        }
        None => 0,
    };
    let mut last_built = None;
    for li in lo..total {
        if out.len() >= max_out { break; }
        let Some(ln) = p.layout.get(li) else { break };
        if let Some((_, bot_px)) = band
            && p.y + ln.metrics().block_min_coord > bot_px
        {
            break;
        }
        if let Some(built) = build_visual_line(&p, ln, content, s, origin) {
            out.push(built);
            last_built = Some(li);
        } else {
            break;
        }
    }
    if !p.is_whole() && total == 0 && out.len() == first && out.len() < max_out {
        if band.is_none_or(|(top_px, bot_px)| p.y + p.height >= top_px && p.y <= bot_px) {
            out.push(Line {
                range: p.start..p.start,
                rect: [origin[0], origin[1] + p.y / s, 0.0, p.height / s],
                lengths: Vec::new(),
                positions: Vec::new(),
                widths: Vec::new(),
                word_starts: Vec::new(),
                rtl: p.flow == crate::el::Flow::Rtl,
                continues_line: false,
            });
            finish_piece_newline(&p, content, first, true, false, out);
            return;
        }
    }
    finish_piece_newline(&p, content, first, total > 0 && last_built == Some(total - 1), false, out);
}

/// End byte in `content` of `layout`'s last laid-out line (mapped through [`Piece::content`],
/// plus any paragraph-ending line break), in O(1) without building every line.
pub(super) fn layout_end_byte(layout: TextFlow<'_>, content: &str) -> Option<usize> {
    // Parley shapes an empty text as one space, so its caret has a line: that line ends at
    // byte 1 of nothing.
    if content.is_empty() || matches!(layout, TextFlow::Paragraphs(pl) if pl.paragraphs.is_empty()) {
        return Some(0);
    }
    let p = layout.piece_at(usize::MAX);
    let end = if !p.shaped {
        p.end
    } else if p.is_blank() || p.layout.len() == 0 {
        if p.is_whole() { 0 } else { p.start }
    } else {
        let last_ln = p.layout.get(p.layout.len() - 1)?;
        p.content(last_ln.text_range().end)
    };
    if end > content.len() { return None; }
    let br = if p.is_whole() {
        0
    } else if content.as_bytes().get(p.end..p.end + 2) == Some(b"\r\n") {
        2
    } else if content.as_bytes().get(p.end) == Some(&b'\n') {
        1
    } else {
        0
    };
    let total = end + br;
    (total <= content.len()).then_some(total)
}

/// The line and character index of drawn byte `byte`.
pub(super) fn position(lines: &[Line], byte: usize) -> Option<(usize, usize)> {
    let last = lines.len().checked_sub(1)?;
    let k = lines.iter().position(|l| byte < l.range.end).unwrap_or(last);
    let l = &lines[k];
    let mut at = l.range.start;
    for (c, len) in l.lengths.iter().enumerate() {
        if byte < at + *len as usize { return Some((k, c)); }
        at += *len as usize;
    }
    Some((k, l.lengths.len()))
}

/// Drawn byte at line `k`'s character `c`.
pub(crate) fn byte_at(lines: &[Line], k: usize, c: usize) -> Option<usize> {
    let l = lines.get(k)?;
    Some(l.range.start + l.lengths.iter().take(c).map(|&n| n as usize).sum::<usize>())
}
