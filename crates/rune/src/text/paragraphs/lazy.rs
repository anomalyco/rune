//! A long paste's lines that wait to be shaped: printable ASCII measured once per style, the
//! records that wait at their exact heights, and the fill that shapes them a few a frame, taking
//! a run apart where it shapes.
use std::hash::{Hash, Hasher};

use super::*;

/// What shapes the paragraphs a long paste left waiting ([`TextSystem::fill_paragraphs`]). A
/// waiting paragraph is plain printable ASCII that fits on one line, so its height is the
/// style's one line exactly and nothing moves when its layout lands.
pub(crate) struct Lazy {
    pub(super) content: Rc<str>,
    pub(super) style: TextStyle,
    pub(super) scale: f32,
    pub(super) wrap_anywhere: bool,
    /// Its layout's entry in the cache, which a fill replaces.
    pub(super) cache_key: u64,
    /// What keys a run's lines when a fill takes it apart (`key_paragraphs`).
    pub(super) style_key: u64,
    pub(super) forced: Option<Flow>,
    pub(super) start_flow: Flow,
    pub(super) ascii: Rc<AsciiLine>,
}

/// One line of printable ASCII in a style: each character's advance and the line's height
/// (device px), read from one shaped line, and an empty layout to stand in for a paragraph not
/// shaped yet.
pub(crate) struct AsciiLine {
    pub(super) advance: [f32; 95],
    /// The widest character's advance: a line this many times its length that fits needs no sum.
    pub(super) widest: f32,
    pub(super) height: f32,
    pub(super) empty: Rc<Layout<Ink>>,
}

/// Whether every byte of `s` is printable ASCII (`0x20..0x7f`), eight at a time.
fn printable(s: &[u8]) -> bool {
    let mut chunks = s.chunks_exact(8);
    for c in &mut chunks {
        let w = u64::from_le_bytes(c.try_into().unwrap());
        // A byte under 0x20 borrows when 0x20 is taken from it; one at or over 0x7f has its top
        // bit set once 1 is added to it.
        let low = w.wrapping_sub(0x2020_2020_2020_2020) & !w;
        let high = w.wrapping_add(0x0101_0101_0101_0101) | w;
        if (low | high) & 0x8080_8080_8080_8080 != 0 { return false; }
    }
    chunks.remainder().iter().all(|b| (0x20..0x7f).contains(b))
}

impl AsciiLine {
    /// `words`' height and width when they're printable ASCII that fits on one line at
    /// `max_width`, with room to spare for what kerning and ligatures do to the sum. A line no
    /// wider than `floor` however wide its characters (a wider line beside it, whose width only
    /// a maximum reads) reports `floor`, summing none of them.
    pub(super) fn one_line(&self, words: &str, max_width: Option<f32>, floor: f32) -> Option<(f32, f32)> {
        let bound = words.len() as f32 * self.widest;
        if bound <= floor && max_width.is_none_or(|m| bound * 1.03 + 2.0 <= m) && printable(words.as_bytes()) {
            return Some((self.height, floor));
        }
        let mut w = 0.0f32;
        for b in words.bytes() {
            if !(0x20..0x7f).contains(&b) { return None; }
            w += self.advance[(b - 0x20) as usize];
        }
        max_width.is_none_or(|m| w * 1.03 + 2.0 <= m).then_some((self.height, w))
    }

    /// Whether `max_width` is a measure of the narrowest the text can be (Taffy's min-content
    /// size, under a line's height wide): no line it breaks there is shown.
    pub(super) fn narrow(&self, max_width: Option<f32>) -> bool {
        max_width.is_some_and(|m| m < self.height)
    }

    /// `words` broken at their narrowest, estimated: a line per space-separated word, as wide as
    /// the widest. Only a min-content measure asks, for the width. A word no wider than `floor`
    /// however wide its characters isn't summed, as `one_line` doesn't.
    pub(super) fn narrowest(&self, words: &str, floor: f32) -> Option<(f32, f32)> {
        let b = words.as_bytes();
        let (mut lines, mut widest, mut start) = (0usize, floor, 0usize);
        let mut word = |s: usize, e: usize| {
            if e == s { return; }
            lines += 1;
            if (e - s) as f32 * self.widest > widest {
                widest = widest.max(b[s..e].iter().map(|&c| self.advance[(c - 0x20) as usize]).sum());
            }
        };
        // Eight bytes at a time: whether all are printable, and where the spaces are.
        let mut chunks = b.chunks_exact(8);
        let mut at = 0;
        for c in &mut chunks {
            let w = u64::from_le_bytes(c.try_into().unwrap());
            let (low, high) = (w.wrapping_sub(0x2020_2020_2020_2020) & !w, w.wrapping_add(0x0101_0101_0101_0101) | w);
            if (low | high) & 0x8080_8080_8080_8080 != 0 { return None; }
            // A space is a zero byte of `w ^ 0x20..`, flagged exactly (no borrow between bytes).
            let x = w ^ 0x2020_2020_2020_2020;
            let mut spaces = !((x & 0x7f7f_7f7f_7f7f_7f7f).wrapping_add(0x7f7f_7f7f_7f7f_7f7f) | x | 0x7f7f_7f7f_7f7f_7f7f);
            while spaces != 0 {
                let j = at + (spaces.trailing_zeros() / 8) as usize;
                word(start, j);
                start = j + 1;
                spaces &= spaces - 1;
            }
            at += 8;
        }
        for (k, &c) in chunks.remainder().iter().enumerate() {
            if !(0x20..0x7f).contains(&c) { return None; }
            if c == b' ' {
                word(start, at + k);
                start = at + k + 1;
            }
        }
        word(start, b.len());
        Some((self.height * lines.max(1) as f32, widest))
    }
}
impl TextSystem {
    /// Printable ASCII in `style` at `scale`, measured once: None when its characters don't all
    /// come from one font, whose one line is then not every ASCII line's.
    pub(super) fn ascii_line(&mut self, style: TextStyle, scale: f32, wrap_anywhere: bool) -> Option<Rc<AsciiLine>> {
        let key = {
            let mut h = hash::KeyHasher::default();
            (hash::text_hash("", &[], style, scale, None), wrap_anywhere).hash(&mut h);
            h.finish()
        };
        if let Some(m) = self.ascii.get(&key) { return m.clone(); }
        let sample: String = (0x21u8..0x7f).flat_map(|b| [b as char, ' ']).collect();
        let l = self.layout_keyed(None, &sample, &[], &[], &[], style, scale, None, Alignment::Start, wrap_anywhere, false, Keep::Owned, Flow::Ltr.base());
        let empty = self.layout_keyed(None, "", &[], &[], &[], style, scale, None, Alignment::Start, wrap_anywhere, false, Keep::Owned, Flow::Ltr.base());
        let measured = (|| {
            let line = l.lines().next().filter(|_| l.lines().count() == 1)?;
            let mut runs = line.runs();
            let run = runs.next().filter(|_| runs.next().is_none())?;
            let mut advance = [f32::NAN; 95];
            for cluster in run.clusters() {
                let r = cluster.text_range();
                let b = *sample.as_bytes().get(r.start)?;
                if r.len() != 1 || !(0x20..0x7f).contains(&b) { return None; }
                advance[(b - 0x20) as usize] = cluster.advance();
            }
            advance.iter().all(|a| a.is_finite()).then(|| Rc::new(AsciiLine { advance, widest: advance.iter().copied().fold(0.0, f32::max), height: l.height(), empty }))
        })();
        self.ascii.insert(key, measured.clone());
        measured
    }

    /// Shapes waiting paragraphs of `laid` for up to `budget`, those meeting device rows `rows`
    /// first, then the nearest, none further than `reach` from them: the layout with them shaped
    /// (cached in its place), and whether a height moved (it shouldn't: a waiting height is
    /// exact). A run is taken apart into its lines, each keyed as keying would, before its first
    /// line is shaped. None when none were waiting.
    pub(crate) fn fill_paragraphs(&mut self, laid: &ParagraphLayout, budget: std::time::Duration, rows: (f32, f32), reach: f32) -> Option<(Rc<ParagraphLayout>, bool)> {
        let lazy = laid.lazy.clone()?;
        let start = std::time::Instant::now();
        let distance = |p: &Paragraph| if p.y + p.height >= rows.0 && p.y <= rows.1 { 0.0 } else { (rows.0 - p.y - p.height).max(p.y - rows.1) };
        let mut paragraphs = laid.paragraphs.clone();
        let (mut moved, mut shaped_any) = (false, false);
        loop {
            if shaped_any && start.elapsed() >= budget { break; }
            // The nearest waiting record: out from where the rows start, both ways.
            let mid = paragraphs.partition_point(|p| p.y + p.height < rows.0);
            let up = paragraphs[mid..].iter().position(|p| !p.shaped).map(|i| mid + i);
            let down = paragraphs[..mid].iter().rposition(|p| !p.shaped);
            let k = match (up, down) {
                (Some(u), Some(d)) => if distance(&paragraphs[u]) <= distance(&paragraphs[d]) { u } else { d },
                (Some(k), None) | (None, Some(k)) => k,
                (None, None) => break,
            };
            if distance(&paragraphs[k]) > reach { break; }
            if paragraphs[k].run {
                let lines = split_run(&lazy, laid.max_width, &paragraphs, k, rows);
                paragraphs.splice(k..k + 1, lines);
                continue;
            }
            let p = &mut paragraphs[k];
            let words = &lazy.content[p.start..p.end];
            let l = self.layout_keyed(Some(p.key), words, &[], &[], &[], lazy.style, lazy.scale, laid.max_width, laid.align, lazy.wrap_anywhere, false, Keep::Owned, p.flow.base());
            moved |= l.height() != p.height;
            (p.height, p.widths, p.layout, p.shaped) = (l.height(), [l.width(), l.full_width()], l, true);
            shaped_any = true;
        }
        let (mut y, mut width, mut full_width) = (0.0f64, 0.0f32, 0.0f32);
        for p in &mut paragraphs {
            p.y = y.round() as f32;
            y += f64::from(p.height);
            (width, full_width) = (width.max(p.widths[0]), full_width.max(p.widths[1]));
        }
        let left = paragraphs.iter().any(|p| !p.shaped);
        let keyed = laid.keyed.as_ref().map(|k| Keyed { content: k.content.clone(), style_key: k.style_key, start_flow: k.start_flow });
        let built = Rc::new(ParagraphLayout {
            paragraphs, width, full_width, height: y as f32, max_width: laid.max_width, align: laid.align, whole: laid.whole,
            newline: laid.newline, keyed, lazy: left.then_some(lazy.clone()),
        });
        let g = self.generation;
        self.paragraphs.insert(lazy.cache_key, Entry::new(built.clone(), g, built.paragraphs.len() * size_of::<Paragraph>() / 64, built.whole));
        if self.last_paragraphs.as_ref().is_some_and(|l| std::ptr::eq(&**l, laid)) { self.last_paragraphs = Some(built.clone()); }        Some((built, moved))
    }
}

/// How many lines a fill takes out of a run at once, at least: those it shapes next and the rows
/// around them. The rest stay runs, so the frames after a long paste hold a few records, not one
/// per line.
const SPLIT_LINES: usize = 64;

/// Run `k` taken apart where `rows` meet it, or at its end nearest them: its lines there as
/// waiting paragraphs of their own, keyed as `key_paragraphs` keys a line, between the runs its
/// lines before and after them stay, each as wide as its widest line.
fn split_run(lazy: &Lazy, max_width: Option<f32>, paragraphs: &[Paragraph], k: usize, rows: (f32, f32)) -> Vec<Paragraph> {
    let r = &paragraphs[k];
    let (n, h) = (r.lines as usize, lazy.ascii.height);
    let pick = |y: f32| ((f64::from(y) - f64::from(r.y)) / f64::from(h)).floor().clamp(0.0, n as f64) as usize;
    let (top, bottom) = (pick(rows.0), (pick(rows.1) + 1).min(n));
    let room = SPLIT_LINES.saturating_sub(bottom.saturating_sub(top)).div_ceil(2);
    let a = top.saturating_sub(room).min(n.saturating_sub(SPLIT_LINES));
    let b = (bottom + room).max(a + SPLIT_LINES).min(n);
    let mut out = Vec::with_capacity(b - a + 2);
    let mut flow = k.checked_sub(1).map_or(lazy.start_flow, |i| paragraphs[i].flow);
    let mut y = f64::from(r.y);
    // The run gathered before `a` or after `b`: its first byte, its lines and its widest line's width.
    let mut run: Option<(usize, u32, f32)> = None;
    let mut at = r.start;
    for (j, words) in lazy.content[r.start..r.end].split('\n').enumerate() {
        let (s, e) = (at, at + words.len());
        at = e + 1;
        // Printable ASCII, as every line of a run is: its letters are its strong characters.
        flow = lazy.forced.or_else(|| words.bytes().any(|c| c.is_ascii_alphabetic()).then_some(Flow::Ltr)).unwrap_or(flow);
        if j < a || j >= b {
            let floor = run.map_or(0.0, |g| g.2);
            let w = lazy.ascii.one_line(words, max_width, floor).map_or(floor, |(_, w)| w);
            let (start, lines, widest) = run.map_or((s, 1, w), |(start, lines, widest)| (start, lines + 1, widest.max(w)));
            run = Some((start, lines, widest));
            if j + 1 == a || j + 1 == n {
                run = None;
                let mut hs = hash::KeyHasher::default();
                (&lazy.content[start..e], lazy.style_key, flow == Flow::Rtl, lines, "run").hash(&mut hs);
                let height = (f64::from(lines) * f64::from(h)) as f32;
                out.push(Paragraph { start, end: e, y: y.round() as f32, height, widths: [widest, widest], flow, key: hs.finish(), isolates: Box::default(), layout: r.layout.clone(), shaped: false, lines, run: true });
                y += f64::from(height);
            }
            continue;
        }
        let mut hs = hash::KeyHasher::default();
        (words, lazy.style_key, flow == Flow::Rtl, Block::Paragraph).hash(&mut hs);
        let (height, w) = lazy.ascii.one_line(words, max_width, 0.0).unwrap_or((h, 0.0));
        out.push(Paragraph { start: s, end: e, y: y.round() as f32, height, widths: [w, w], flow, key: hs.finish(), isolates: Box::default(), layout: r.layout.clone(), shaped: false, lines: 1, run: false });
        y += f64::from(height);
    }
    out
}