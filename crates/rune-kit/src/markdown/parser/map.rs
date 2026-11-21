//! Source-to-rendered byte mappings parallel to [`Block`] and inline code spans.

use super::super::*;

/// Maps source UTF-8 byte ranges to rendered text UTF-8 byte ranges.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::markdown) struct SpanMap(pub(in crate::markdown) Vec<(Range<usize>, Range<usize>)>);

impl SpanMap {
    pub(super) fn push(&mut self, src: Range<usize>, out: Range<usize>) {
        if src.start >= src.end || out.start >= out.end {
            return;
        }
        if let Some((last_src, last_out)) = self.0.last_mut()
            && last_src.end == src.start
            && last_out.end == out.start
            && last_src.len() == last_out.len()
            && src.len() == out.len()
        {
            last_src.end = src.end;
            last_out.end = out.end;
            return;
        }
        self.0.push((src, out));
    }

    pub(in crate::markdown) fn truncate_out(&mut self, len: usize) {
        self.0.retain_mut(|(src, out)| {
            if out.start >= len {
                return false;
            }
            if out.end > len {
                let cut = out.end - len;
                if src.len() == out.len() {
                    src.end = src.end.saturating_sub(cut);
                }
                out.end = len;
            }
            src.start < src.end && out.start < out.end
        });
    }

    pub(in crate::markdown) fn shift_src(&mut self, offset: usize) {
        if offset == 0 {
            return;
        }
        for (src, _) in &mut self.0 {
            src.start += offset;
            src.end += offset;
        }
    }
}

/// Source-to-rendered byte mappings parallel to [`Block`], kept beside `Block` so `Block` keeps
/// its shape.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::markdown) enum BlockMap {
    Inline(SpanMap),
    Code(SpanMap),
    DeepTail { lines: usize, map: SpanMap },
    List(Vec<Vec<BlockMap>>),
    Quote(Vec<BlockMap>),
    Table { head: Vec<SpanMap>, rows: Vec<Vec<SpanMap>> },
    #[default]
    None,
}

impl BlockMap {
    pub(in crate::markdown) fn shift_src(mut self, offset: usize) -> Self {
        if offset == 0 {
            return self;
        }
        match &mut self {
            BlockMap::Inline(m) | BlockMap::Code(m) | BlockMap::DeepTail { map: m, .. } => m.shift_src(offset),
            BlockMap::List(items) => {
                for item in items {
                    for b in item {
                        *b = std::mem::take(b).shift_src(offset);
                    }
                }
            }
            BlockMap::Quote(inner) => {
                for b in inner {
                    *b = std::mem::take(b).shift_src(offset);
                }
            }
            BlockMap::Table { head, rows } => {
                for m in head {
                    m.shift_src(offset);
                }
                for row in rows {
                    for m in row {
                        m.shift_src(offset);
                    }
                }
            }
            BlockMap::None => {}
        }
        self
    }
}

/// Source byte range of the inner code text inside a backtick code span `src[range]`.
pub(super) fn code_span_content_range(src: &str, range: Range<usize>, rendered: &str) -> Range<usize> {
    let Some(raw) = src.get(range.clone()) else { return range };
    let open = raw.bytes().take_while(|&b| b == b'`').count();
    let close = raw[open..].bytes().rev().take_while(|&b| b == b'`').count();
    let mut start = range.start + open;
    let mut end = range.end.saturating_sub(close);
    if !rendered.chars().all(|c| c == ' ')
        && src.as_bytes().get(start).is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n'))
        && src.as_bytes().get(end.saturating_sub(1)).is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n'))
    {
        start += 1;
        end -= 1;
    }
    if start < end { start..end } else { range }
}

/// Code spans normalize newlines and omit continuation-line container prefixes. Map the
/// actual surviving characters, not a ratio of the source and displayed byte lengths.
pub(super) fn map_code_span(src: &str, range: Range<usize>, rendered: &str, offset: usize, map: &mut SpanMap) {
    if &src[range.clone()] == rendered {
        map.push(range, offset..offset + rendered.len());
        return;
    }
    let mut out = 0;
    let mut at = range.start;
    for line in src[range].split_inclusive(['\r', '\n']) {
        let body = line.trim_end_matches(['\r', '\n']);
        let remaining = &rendered[out..];
        let matched = body.char_indices().find_map(|(skip, _)| {
            let mut chars = body[skip..].char_indices().peekable();
            let mut consumed = 0;
            let mut pairs = Vec::new();
            while let Some((i, mut c)) = chars.next() {
                let mut end = i + c.len_utf8();
                if c == '\\' && chars.peek().is_some_and(|(_, c)| *c == '|') && remaining[consumed..].starts_with('|') {
                    let (j, next) = chars.next().unwrap();
                    c = next;
                    end = j + c.len_utf8();
                }
                if !remaining[consumed..].starts_with(c) { return None; }
                pairs.push((at + skip + i..at + skip + end, offset + out + consumed..offset + out + consumed + c.len_utf8()));
                consumed += c.len_utf8();
            }
            Some((consumed, pairs))
        });
        if let Some((consumed, pairs)) = matched {
            for (src, dest) in pairs { map.push(src, dest); }
            out += consumed;
        }
        if line.ends_with(['\r', '\n']) && rendered[out..].starts_with(' ') {
            map.push(at + body.len()..at + line.len(), offset + out..offset + out + 1);
            out += 1;
        }
        at += line.len();
    }
}
