//! GFM tables: columns sized from their content with [`rune::text::measure`], then shared out by
//! flex growth so every row's cells line up without a grid.

use super::*;
use super::style::{TableBody, TableFrame};

/// Estimated bytes of column widths kept beyond the tables on screen.
const WIDTHS_BYTES: usize = 1 << 20;

/// A setter's points: zero when negative, NaN or infinite, so a width never goes non-finite.
pub(super) fn pts(x: f32) -> f32 {
    if x.is_finite() { x.max(0.0) } else { 0.0 }
}

/// Per column: the content's width on one line and its longest word (capped, then whole),
/// padding included.
#[derive(Debug)]
struct Columns {
    natural: Vec<f32>,
    min: Vec<f32>,
    fit: Vec<f32>,
}

/// A browser's automatic table layout (`table-layout: auto`) in `avail` points: columns in
/// proportion to their widest content when it all fits (`full`, a `width: 100%` table) or at
/// it (an auto-width one); between their narrowest and widest, by how much each can give, when
/// only the narrowest fits; and at their narrowest otherwise, overflowing.
fn auto_layout(natural: &[f32], fit: &[f32], avail: f32, full: bool) -> Vec<f32> {
    let (widest, narrowest): (f32, f32) = (natural.iter().sum(), fit.iter().sum());
    if widest <= avail {
        let k = if full && widest > 0.0 { avail / widest } else { 1.0 };
        return natural.iter().map(|w| w * k).collect();
    }
    if narrowest >= avail || widest <= narrowest {
        return fit.to_vec();
    }
    let k = (avail - narrowest) / (widest - narrowest);
    natural.iter().zip(fit).map(|(w, f)| f + (w - f).max(0.0) * k).collect()
}

thread_local! {
    static WIDTHS: RefCell<Budget<u64, Rc<Columns>>> = RefCell::new(Budget::new(WIDTHS_BYTES));
}

fn font_key(f: TextStyle) -> (u32, u32, u32, u32, String) {
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
    /// Whether cells follow their column's `:-:` and `--:` alignment (on by default). Off, every
    /// cell starts at its column's left, as a web table whose CSS sets `text-align: start`.
    pub fn table_column_align(mut self, on: bool) -> Self {
        self.table_align = on;
        self
    }
    /// The space above and below a table (12 and 16 points by default). Like the other blocks'
    /// margins, it collapses with the space a neighbour wants, taking the larger.
    pub fn table_margin(mut self, top: f32, bottom: f32) -> Self {
        self.table_margin = [pts(top), pts(bottom)];
        self
    }
    /// A table that scrolls sideways ([`Markdown::scroll_wide`] with [`Self::text_width`]) gets a
    /// scroll bar in an 8 point strip under its rows: `@opencode/ui`'s `scrollbar.css` thumb
    /// ([`ScrollBarStyle::native`](crate::ScrollBarStyle::native)), `thumb` while the pointer is
    /// over the table and `thumb_hover` while it's over the thumb. None by default.
    pub fn table_scroll_bar(mut self, thumb: Color, thumb_hover: Color) -> Self {
        self.table_bar = Some([thumb, thumb_hover]);
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
        // A frame's ring is `box-shadow: inset 0 0 0 0.5px`, which keeps its fractional width;
        // an outline is a border, which snaps to whole device pixels.
        let framed = s.table_frame.is_some();
        let ringed = |e: El, c: Color| if framed {
            e.box_shadow(rune::Shadow { spread: 0.5, color: c, inset: true, ..rune::Shadow::default() })
        } else {
            e.border(0.5, c)
        };
        // Sized exactly in the known text width, the rows scroll inside a frame that stays put,
        // as a web page's `overflow-x: auto` wrapper keeps its fill and ring while the table
        // inside it scrolls.
        let fixed = s.text_width.filter(|_| self.scroll_wide).map(|w| {
            let avail = (w - self.flow.get().inset - 2.0 * inset).max(0.0);
            let widths = auto_layout(&cols.natural, &cols.fit, avail, s.table_full_width);
            let over = widths.iter().sum::<f32>() > avail + 0.5;
            (widths, over)
        });
        let mut frame = col().role(if fixed.is_some() { "" } else { "table" }).w_full().padding(inset);
        let mut decor = col().role("table").w_full();
        if !s.table_full_width {
            frame = frame.max_w(total + 2.0 * inset);
        }
        if let Some(fill) = fill {
            match fixed {
                Some(_) => decor = decor.radius(6.0).fill(fill),
                None => frame = frame.radius(6.0).fill(fill),
            }
        }
        let widths = fixed.as_ref().map(|(w, _)| w.as_slice());
        let head = self.table_row(&t.head, head_maps, t, &cols, widths, self.head_font(), s.table_head_text, self.head_pad(), "columnheader").key("th");
        let rule = |key: String| el().key(key).w_full().h(0.5).fill(s.table_rule).a11y_hidden();
        let row_at = |i: usize, cells: &[Inline]| {
            let rmap = row_maps.get(i).map_or(&[][..], Vec::as_slice);
            self.table_row(cells, rmap, t, &cols, widths, self.body_font(), s.text, s.table_pad, "cell").key(format!("r{i}"))
        };
        if let Some(body) = &s.table_body {
            // The frame doesn't clip, so the card's shadow shows past it; the header row and
            // the card clip their own overflow instead.
            if let Some(ring) = ring {
                match fixed {
                    Some(_) => decor = ringed(decor.radius(6.0), ring),
                    None => frame = ringed(frame.radius(6.0), ring),
                }
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
                    // `tbody tr:not(:last-child) td { border-bottom: 0.5px }`: a border that
                    // takes a whole device pixel of the row, as Chromium lays it out.
                    let row = row_at(i, cells);
                    rows = rows.child(if i + 1 < t.rows.len() { row.border_sides([0.0, 0.0, 0.5, 0.0], s.table_rule).border_layout() } else { row });
                }
                let card = col().key("tb").w_full().radius(body.radius).fill(body.fill);
                frame = frame.child(body.shadows.iter().fold(card, |c, sh| c.box_shadow(*sh)).child(rows));
            }
        } else {
            match (ring, fixed.is_some()) {
                (Some(ring), true) => decor = ringed(decor.radius(6.0), ring),
                (Some(ring), false) => frame = ringed(frame.radius(6.0), ring).clip_rounded(6.0),
                (None, true) => {}
                (None, false) => frame = frame.clip_soft(0.0),
            }
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
        if let Some((widths, over)) = fixed {
            let total: f32 = widths.iter().sum();
            return self.wide_frame(decor, frame.w(total + 2.0 * inset), over);
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

    /// `rows`, a table sized to the text width, scrolling sideways in `decor` when `over` it, with
    /// the style's scroll bar in room under it, as a web page's `overflow-x: auto` wrapper
    /// takes a classic scroll bar's height.
    fn wide_frame(&self, decor: El, rows: El, over: bool) -> El {
        let viewport = col().key("scroll").w_full().scroll_x().clip_rounded(6.0).child(rows);
        let Some([thumb, hover]) = self.s.table_bar.filter(|_| over) else { return decor.child(viewport) };
        let bar = crate::ScrollBarStyle::native(thumb, hover).horizontal();
        decor.child(crate::ScrollBar::scroll_bar_styled(viewport, bar).pb(bar.strip()))
    }

    #[allow(clippy::too_many_arguments)]
    fn table_row(&self, cells: &[Inline], maps: &[SpanMap], t: &Table, cols: &Columns, widths: Option<&[f32]>, font: TextStyle, color: Color, [pad_x, pad_y]: [f32; 2], cell_role: &'static str) -> El {
        let empty = Inline::default();
        let total: f32 = cols.natural.iter().sum();
        let mut r = row().role("row").w_full();
        for (j, (natural, min)) in cols.natural.iter().zip(&cols.min).enumerate() {
            let inl = cells.get(j).unwrap_or(&empty);
            let align = if self.s.table_align { t.align.get(j).copied().unwrap_or_default() } else { Align::Start };
            let content = self
                .with_reveal(
                    rich(inl.text.clone(), self.spans(inl, font))
                        .font(font)
                        .color(color)
                        .align(align),
                )
                .w_full();
            // Full width: each column's share of the row is its share of the natural width, so
            // a wider row widens columns in proportion and a narrower one shrinks them as
            // without it. A table that scrolls never shrinks a column below its content.
            let cell = col().key(format!("c{j}")).role(cell_role);
            let cell = match (widths.and_then(|w| w.get(j)), self.s.table_full_width && total > 0.0, self.scroll_wide) {
                (Some(w), ..) => cell.w(*w),
                (None, false, _) => cell.w(*natural).shrink().min_w(*min),
                (None, true, false) => cell.w_frac(natural / total).shrink().min_w(*min),
                (None, true, true) => cell.w_frac(natural / total).shrink().min_w(*natural),
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
            (self.s.inline_code_weight.to_bits(), self.s.inline_code_pad.to_bits()),
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
            let (mut natural, mut fit) = (vec![0.0f32; n], vec![0.0f32; n]);
            for cells in lines {
                for (j, inl) in cells.iter().take(n).enumerate() {
                    if inl.text.is_empty() {
                        continue;
                    }
                    let spans = self.spans(inl, font);
                    // Unrounded, as a browser's table layout sums them; a hair over, so a cell at
                    // its column's natural width never wraps its last word.
                    let (min, max) = rune::text::content_widths(&inl.text, &spans, font);
                    // Inline code's padding is white space set around it, which a line leaves
                    // out of its width where it ends the line; a page counts an inline box's
                    // padding in the cell's widths.
                    let ends_code = |end: usize| inl.text[end..].chars().next().is_none_or(char::is_whitespace);
                    let pad = |hit: bool| if hit { self.s.inline_code_pad } else { 0.0 };
                    let pads = inl.spans.iter().filter(|s| s.2 == Style::CodePad && inl.spans.iter().any(|c| c.2 == Style::Code && c.1 == s.0));
                    let (last, word) = pads.fold((false, false), |(last, word), s| (last || s.1 == inl.text.len(), word || ends_code(s.1)));
                    natural[j] = natural[j].max(max + pad(last) + 0.01);
                    fit[j] = fit[j].max(min + pad(word) + 0.01);
                }
            }
            let min = fit.iter().map(|w| w.min(cap)).collect::<Vec<_>>();
            (natural, min, fit)
        };
        let (head_natural, head_min, head_fit) = widths(&mut std::iter::once(&t.head), head);
        let (body_natural, body_min, body_fit) = widths(&mut rows.iter(), body);
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
        let cols = Rc::new(Columns { natural: padded(&head_natural, &body_natural), min: padded(&head_min, &body_min), fit: padded(&head_fit, &body_fit) });
        WIDTHS.with_borrow_mut(|c| c.insert(key, cols.clone(), 12 * n + 64));
        cols
    }
}
