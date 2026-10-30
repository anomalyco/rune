//! GFM tables: columns sized from their content with [`rune::text::measure`], then shared out by
//! flex growth so every row's cells line up without a grid.

use super::*;
use super::style::{TableBody, TableFrame};

/// Estimated bytes of column widths kept beyond the tables on screen.
const WIDTHS_BYTES: usize = 1 << 20;

/// A setter's points: zero when negative, NaN or infinite, so a width never goes non-finite.
fn pts(x: f32) -> f32 {
    if x.is_finite() { x.max(0.0) } else { 0.0 }
}

/// Per column: the content's width on one line and its longest word (capped), padding included.
#[derive(Debug)]
struct Columns {
    natural: Vec<f32>,
    min: Vec<f32>,
}

thread_local! {
    static WIDTHS: RefCell<Budget<u64, Rc<Columns>>> = RefCell::new(Budget::new(WIDTHS_BYTES));
}

pub(super) fn font_key(f: TextStyle) -> (u32, u32, u32, u32, String) {
    (f.size.to_bits(), f.weight.to_bits(), f.line_height.to_bits(), f.tracking.to_bits(), format!("{:?}{}{}{}", f.family, f.variations, f.features, f.italic))
}

/// `features` with tabular figures on, interned so the result is `'static` like a
/// [`TextStyle`]'s features (a style has few distinct feature lists).
fn with_tnum(features: &'static str) -> &'static str {
    if features.is_empty() {
        return "'tnum' 1";
    }
    if features.contains("tnum") {
        return features;
    }
    static INTERNED: std::sync::Mutex<Vec<(&'static str, &'static str)>> = std::sync::Mutex::new(Vec::new());
    let mut interned = INTERNED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&(_, out)) = interned.iter().find(|(f, _)| *f == features) {
        return out;
    }
    let out: &'static str = Box::leak(format!("{features}, 'tnum' 1").into_boxed_str());
    interned.push((features, out));
    out
}

impl MarkdownStyle {
    /// A table's outline and row rules, and its header row's fill.
    pub fn table(mut self, border: Color, header_fill: Color) -> Self {
        self.table_outline = Some(border);
        self.table_frame = None;
        self.table_rule = border;
        self.table_head_fill = header_fill;
        self
    }
    /// The rounded outline around a table, or `None` for none; a rule then runs under the
    /// last row too, as on a web page. A [`Self::table_frame`] draws in its place.
    pub fn table_outline(mut self, outline: Option<Color>) -> Self {
        self.table_outline = outline;
        self.table_frame = None;
        self
    }
    /// The hairline under the header and between rows.
    pub fn table_rules(mut self, c: Color) -> Self {
        self.table_rule = c;
        self
    }
    /// The header row's text colour (the body text colour by default).
    pub fn table_header_text(mut self, c: Color) -> Self {
        self.table_head_text = c;
        self
    }
    /// The header row's fill alone (`table` sets it with the border).
    pub fn table_header_fill(mut self, c: Color) -> Self {
        self.table_head_fill = c;
        self
    }
    /// The hairline under the header row, when it should stand out from the row rules, as a
    /// web `th { border-bottom }` in a stronger colour than the `td`s'. The row rule colour
    /// by default.
    pub fn table_header_rule(mut self, c: Color) -> Self {
        self.table_head_rule = Some(c);
        self
    }
    /// The header row's weight (the strong weight by default), for a medium-weight `th` beside
    /// bold `strong` text.
    pub fn table_header_weight(mut self, w: f32) -> Self {
        self.table_head_weight = Some(w);
        self
    }
    /// Whether cells set figures at one width (OpenType `tnum`) so numbers line up down a
    /// column. On by default; only digits change.
    pub fn table_tabular_figures(mut self, on: bool) -> Self {
        self.table_tabular = on;
        self
    }
    /// The narrowest a column shrinks to when its longest word is wider (120 points by default).
    pub fn table_min_column(mut self, points: f32) -> Self {
        self.table_min_column = pts(points);
        self
    }
    /// The cells' font (the body font by default). The header takes it at the strong weight,
    /// and inline code scales from its size. A web table at `font-size: 0.93em` is the body
    /// style at 0.93 of its size.
    pub fn table_font(mut self, f: TextStyle) -> Self {
        self.table_font = Some(f);
        self
    }
    /// Each cell's padding: `x` on the left and right, `y` above and below (10 and 6 points by
    /// default).
    pub fn table_cell_padding(mut self, x: f32, y: f32) -> Self {
        self.table_pad = [pts(x), pts(y)];
        self
    }
    /// Whether the first column starts at the table's left edge, with no left padding, so its
    /// text lines up with the paragraphs around it, as a web table whose first cells have
    /// `padding-left: 0`. Off by default.
    pub fn table_flush_left(mut self, on: bool) -> Self {
        self.table_flush_left = on;
        self
    }
    /// Tables that read like a data [`Table`](crate::Table) beside them: its cell font, cell
    /// padding (sized so a one-line row is the data table's row height), rules and muted header
    /// text. The outline, when there is one, takes the rule colour.
    pub fn table_like(mut self, t: &crate::TableStyle) -> Self {
        self.table_font = Some(t.font);
        self.table_pad = [t.cell_pad, ((t.row_height - t.font.line_height) / 2.0).max(0.0)];
        self.table_rule = t.rule;
        self.table_head_text = t.header_text;
        self.table_outline = self.table_outline.map(|_| t.rule);
        self.table_frame = None;
        self.table_body = None;
        self.table_head_pad = None;
        self
    }
    /// A rounded frame around the table in place of the outline: filled `fill`, with a
    /// half-point `ring` drawn inside its edge and `pad` points between the edge and the rows.
    pub fn table_frame(mut self, fill: Color, ring: Color, pad: f32) -> Self {
        self.table_frame = Some(TableFrame { fill, ring, pad: pts(pad) });
        self
    }
    /// The body rows on a card of their own inside the frame: filled `fill`, rounded `radius`,
    /// with `shadows` under it (an elevation, e.g. the kit's `elevation.raised`). The header
    /// then sits on the frame with no fill and no rule under it, and rules run between body
    /// rows only.
    pub fn table_body(mut self, fill: Color, radius: f32, shadows: &[rune::Shadow]) -> Self {
        self.table_body = Some(TableBody { fill, radius: pts(radius), shadows: shadows.to_vec() });
        self
    }
    /// The header cells' padding, `x` on the left and right and `y` above and below (the body
    /// cells' [`Self::table_cell_padding`] by default).
    pub fn table_header_pad(mut self, x: f32, y: f32) -> Self {
        self.table_head_pad = Some([pts(x), pts(y)]);
        self
    }
    /// Whether a table fills the text's width (a web table's `width: 100%`), the extra space
    /// shared out to columns in proportion to their content. Off by default: a narrow table is
    /// as wide as its content. A table wider than the text shrinks or scrolls either way.
    pub fn table_full_width(mut self, on: bool) -> Self {
        self.table_full_width = on;
        self
    }
    /// The space above and below a table (12 and 16 points by default). Like the other blocks'
    /// margins, it collapses with the space a neighbour wants, taking the larger.
    pub fn table_margin(mut self, top: f32, bottom: f32) -> Self {
        self.table_margin = [pts(top), pts(bottom)];
        self
    }
    /// A data table on a card, from the kit's tokens: the body rows on a raised `bg_base` card
    /// inset 2 points in a `layer_01` frame that also holds the faint, medium-weight header;
    /// 13-point cells at weight 440 with tabular figures, padded 12 by 10 (the header 12 by 8),
    /// filling the text's width, 16 points above and below.
    pub fn table_card(self, k: &Kit) -> Self {
        let c = &k.c;
        let raised: Vec<rune::Shadow> = k.shadow_layers(&k.elevation.raised).collect();
        self.table_frame(c.layer_01, c.border_muted, 2.0)
            .table_body(c.bg_base, 4.0, &raised)
            .table_rules(c.border_muted)
            .table_header_text(c.text_faint)
            .table_header_weight(500.0)
            .table_header_pad(12.0, 8.0)
            .table_cell_padding(12.0, 10.0)
            .table_font(TextStyle { weight: 440.0, tracking: -0.04, ..k.ty.ui_compact })
            .table_tabular_figures(true)
            .table_full_width(true)
            .table_margin(16.0, 16.0)
    }
}

impl Renderer {
    fn head_font(&self) -> TextStyle {
        self.cell_font(TextStyle { weight: self.s.table_head_weight.unwrap_or(self.s.strong_weight), ..self.s.table_font.unwrap_or(self.s.body) })
    }

    fn body_font(&self) -> TextStyle {
        self.cell_font(self.s.table_font.unwrap_or(self.s.body))
    }

    /// Column `j`'s left padding in a row padded `x` on either side.
    fn pad_left(&self, j: usize, x: f32) -> f32 {
        if j == 0 && self.s.table_flush_left { 0.0 } else { x }
    }

    fn head_pad(&self) -> [f32; 2] {
        self.s.table_head_pad.unwrap_or(self.s.table_pad)
    }

    fn cell_font(&self, f: TextStyle) -> TextStyle {
        if self.s.table_tabular { TextStyle { features: with_tnum(f.features), ..f } } else { f }
    }

    pub(super) fn table(&self, t: &Table, head_maps: &[SpanMap], row_maps: &[Vec<SpanMap>]) -> El {
        let s = &self.s;
        let n = t.align.len().max(t.head.len()).max(1);
        let cols = self.columns(t, n);
        let total: f32 = cols.natural.iter().sum();
        // Without an outline the corners stay square, so the rules run the full width, and no
        // border inset moves the cells off the text around the table. Either way columns too
        // wide to fit stop at the frame.
        let (ring, fill, inset) = match (s.table_frame, s.table_outline) {
            (Some(f), _) => (Some(f.ring), Some(f.fill), f.pad.max(0.5)),
            (None, Some(outline)) => (Some(outline), None, 0.5),
            (None, None) => (None, None, 0.0),
        };
        let mut frame = col().w_full().padding(inset);
        if !s.table_full_width {
            frame = frame.max_w(total + 2.0 * inset);
        }
        if let Some(fill) = fill {
            frame = frame.radius(6.0).fill(fill);
        }
        let head = self.table_row(&t.head, head_maps, t, &cols, self.head_font(), s.table_head_text, self.head_pad()).key("th");
        let rule = |key: String| el().key(key).w_full().h(0.5).fill(s.table_rule);
        let row_at = |i: usize, cells: &[Inline]| {
            let rmap = row_maps.get(i).map_or(&[][..], Vec::as_slice);
            self.table_row(cells, rmap, t, &cols, self.body_font(), s.text, s.table_pad).key(format!("r{i}"))
        };
        if let Some(body) = &s.table_body {
            // The frame doesn't clip, so the card's shadow shows past it; the header row and
            // the card clip their own overflow instead.
            if let Some(ring) = ring {
                frame = frame.radius(6.0).border(0.5, ring);
            }
            // The header's overflow stops at the frame's rounded top corners.
            let head = if ring.is_some() || fill.is_some() {
                let r = (6.0 - inset).max(0.0);
                head.clip_corners([r, r, 0.0, 0.0])
            } else {
                head.clip_soft(0.0)
            };
            frame = frame.child(head);
            if !t.rows.is_empty() {
                let mut rows = col().key("tbc").w_full().clip_rounded(body.radius);
                for (i, cells) in t.rows.iter().enumerate() {
                    if i > 0 {
                        rows = rows.child(rule(format!("s{i}")));
                    }
                    rows = rows.child(row_at(i, cells));
                }
                let card = col().key("tb").w_full().radius(body.radius).fill(body.fill);
                frame = frame.child(body.shadows.iter().fold(card, |c, sh| c.box_shadow(*sh)).child(rows));
            }
        } else {
            frame = match ring {
                Some(ring) => frame.radius(6.0).border(0.5, ring).clip_rounded(6.0),
                None => frame.clip_soft(0.0),
            };
            // The header's square fill would paint over the outline where it curves in at the
            // frame's corners, so it rounds by the radius left inside the inset.
            let head = head.fill(s.table_head_fill);
            let head = if ring.is_some() {
                let r = (6.0 - inset).max(0.0);
                let b = if t.rows.is_empty() { r } else { 0.0 };
                head.clip_corners([r, r, b, b])
            } else {
                head
            };
            frame = frame.child(head);
            for (i, cells) in t.rows.iter().enumerate() {
                let sep = rule(format!("s{i}"));
                let sep = match s.table_head_rule { Some(c) if i == 0 => sep.fill(c), _ => sep };
                frame = frame.child(sep).child(row_at(i, cells));
            }
            if ring.is_none() {
                let end = rule("s-end".into());
                frame = frame.child(match s.table_head_rule { Some(c) if t.rows.is_empty() => end.fill(c), _ => end });
            }
        }
        if self.scroll_wide {
            // Every column at its natural width, scrolling when the table is wider than the text.
            // Full width: the viewport makes `w_full` content its natural width, or its own if wider.
            // Capped at the natural width either way, so a row still streaming in can't widen
            // the frame in the viewport's max-content pass and shift every column.
            let frame = if s.table_full_width { frame.max_w(total + 2.0 * inset) } else { frame.w(total + 2.0 * inset) };
            return col().key("scroll").w_full().scroll_x().child(frame);
        }
        frame
    }

    #[allow(clippy::too_many_arguments)]
    fn table_row(&self, cells: &[Inline], maps: &[SpanMap], t: &Table, cols: &Columns, font: TextStyle, color: Color, [pad_x, pad_y]: [f32; 2]) -> El {
        let empty = Inline::default();
        let total: f32 = cols.natural.iter().sum();
        let mut r = row().w_full();
        for (j, (natural, min)) in cols.natural.iter().zip(&cols.min).enumerate() {
            let inl = cells.get(j).unwrap_or(&empty);
            let align = t.align.get(j).copied().unwrap_or_default();
            let content = rich(inl.text.clone(), self.spans(inl, font.size))
                .font(font)
                .color(color)
                .align(align)
                .reveal(self.reveal)
                .w_full();
            // Full width: each column's share of the row is its share of the natural width, so
            // a wider row widens columns in proportion and a narrower one shrinks them as
            // without it. A table that scrolls never shrinks a column below its content.
            let cell = col().key(format!("c{j}"));
            let cell = match (self.s.table_full_width && total > 0.0, self.scroll_wide) {
                (false, _) => cell.w(*natural).shrink().min_w(*min),
                (true, false) => cell.w_frac(natural / total).shrink().min_w(*min),
                (true, true) => cell.w_frac(natural / total).shrink().min_w(*natural),
            };
            r = r.child(
                // Clipped: a word wider than a shrunk column (or a row still streaming in) must
                // not run into the next column.
                cell.pl(self.pad_left(j, pad_x)).pr(pad_x).py(pad_y).clip_soft(0.0).child(self.select_inline(content, inl, maps.get(j))),
            );
        }
        r
    }

    /// Column widths from the header and every finished row, cached by content and font. A last
    /// row without a newline is still arriving only while the text streams; otherwise the
    /// document is whole and it counts too.
    fn columns(&self, t: &Table, n: usize) -> Rc<Columns> {
        let (head, body) = (self.head_font(), self.body_font());
        let settled = if t.open && self.streaming { t.rows.len().saturating_sub(1) } else { t.rows.len() };
        let rows = &t.rows[..settled];
        let cap = self.s.table_min_column;
        let link_styles: Vec<_> = if let Some(look) = &self.link_look {
            t.head
                .iter()
                .chain(rows.iter().flatten())
                .flat_map(|inl| &inl.links)
                .map(|(_, _, url)| look(url).map(|(sp, _)| (sp.weight.map(f32::to_bits), sp.size.map(f32::to_bits), sp.mono, sp.italic)))
                .collect()
        } else {
            Vec::new()
        };
        let key = hash_of((
            n,
            &t.head,
            rows,
            font_key(head),
            font_key(body),
            cap.to_bits(),
            self.s.inline_code_scale.to_bits(),
            self.s.inline_code_weight.to_bits(),
            self.s.table_pad.map(f32::to_bits),
            self.head_pad().map(f32::to_bits),
            self.s.table_flush_left,
            (link_styles, rune::text::font_epoch()),
        ));
        if let Some(hit) = WIDTHS.with_borrow_mut(|c| c.get(&key).cloned()) {
            return hit;
        }
        // Each line's widest cell per column, the header's apart from the body's: their
        // padding can differ.
        let widths = |lines: &mut dyn Iterator<Item = &Vec<Inline>>, font: TextStyle| {
            let (mut natural, mut min) = (vec![0.0f32; n], vec![0.0f32; n]);
            for cells in lines {
                for (j, inl) in cells.iter().take(n).enumerate() {
                    if inl.text.is_empty() {
                        continue;
                    }
                    let spans = self.spans(inl, font.size);
                    natural[j] = natural[j].max(rune::text::measure(&inl.text, &spans, font, None).0);
                    min[j] = min[j].max(rune::text::measure(&inl.text, &spans, font, Some(0.0)).0.min(cap));
                }
            }
            (natural, min)
        };
        let (head_natural, head_min) = widths(&mut std::iter::once(&t.head), head);
        let (body_natural, body_min) = widths(&mut rows.iter(), body);
        let [head_x, body_x] = [self.head_pad()[0], self.s.table_pad[0]];
        let has_body = !rows.is_empty();
        let padded = |h: &[f32], b: &[f32]| -> Vec<f32> {
            (0..n)
                .map(|j| {
                    let head = h[j] + self.pad_left(j, head_x) + head_x;
                    if has_body { head.max(b[j] + self.pad_left(j, body_x) + body_x) } else { head }
                })
                .collect()
        };
        let cols = Rc::new(Columns { natural: padded(&head_natural, &body_natural), min: padded(&head_min, &body_min) });
        WIDTHS.with_borrow_mut(|c| c.insert(key, cols.clone(), 8 * n + 64));
        cols
    }
}
