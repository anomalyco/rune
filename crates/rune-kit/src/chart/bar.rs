use std::rc::Rc;

use rune::{Color, El, Shape, canvas, col, row, text};

use super::{Format, Frame, Hover, Scale, VIEW, band_x, hover, label_font, plain, series_color, show};
use crate::Kit;

/// Categories compared by one value each: upright bars over a value axis (the default), or
/// [`horizontal`](Self::horizontal) rows of name, bar and value. Bars grow from zero, so
/// negative values point the other way. Non-finite values draw no bar.
///
/// ```
/// # use rune_kit::{BarChart, Kit};
/// # let k = Kit::neutral(false);
/// # let _ =
/// BarChart::new("langs")
///     .label("Files by language")
///     .bar("Rust", 412.0)
///     .bar("TypeScript", 180.0)
///     .bar("Markdown", 64.0)
///     .horizontal()
///     .build(&k)
/// # ;
/// ```
pub struct BarChart {
    key: String,
    label: Option<String>,
    bars: Vec<(String, f32)>,
    horizontal: bool,
    range: Option<[f32; 2]>,
    height: f32,
    color: Option<Color>,
    format: Format,
    hovered: Option<usize>,
    hover: Option<Hover>,
}

impl BarChart {
    /// `key` names the chart's elements; give each chart on screen its own.
    pub fn new(key: impl Into<String>) -> BarChart {
        BarChart {
            key: key.into(),
            label: None,
            bars: Vec::new(),
            horizontal: false,
            range: None,
            height: 160.0,
            color: None,
            format: Rc::new(plain),
            hovered: None,
            hover: None,
        }
    }
    /// The accessible name (what the chart shows).
    pub fn label(mut self, l: impl Into<String>) -> Self {
        self.label = Some(l.into());
        self
    }
    /// Adds a category and its value.
    pub fn bar(mut self, name: impl Into<String>, value: f32) -> Self {
        self.bars.push((name.into(), value));
        self
    }
    /// Adds categories and their values.
    pub fn bars<I: IntoIterator<Item = (T, f32)>, T: Into<String>>(mut self, bars: I) -> Self {
        self.bars.extend(bars.into_iter().map(|(n, v)| (n.into(), v)));
        self
    }
    /// One row per category: its name, its bar growing to the right and its value at the end.
    pub fn horizontal(mut self) -> Self {
        self.horizontal = true;
        self
    }
    /// The value axis's ends; by default the data's, taking in zero and rounded out.
    pub fn range(mut self, lo: f32, hi: f32) -> Self {
        self.range = Some([lo, hi]);
        self
    }
    /// Upright: the plot's height in points (160 by default). Horizontal: ignored, each row
    /// is a label line tall.
    pub fn height(mut self, h: f32) -> Self {
        self.height = h.max(1.0);
        self
    }
    /// The bars' colour; the theme's accent by default.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
    /// How ticks, values and the readout write values (two decimals at most by default).
    pub fn format(mut self, f: impl Fn(f32) -> String + 'static) -> Self {
        self.format = Rc::new(f);
        self
    }
    /// The category under the pointer, from [`Self::on_hover`]: the others dim and, upright,
    /// its name and value read out above the plot.
    pub fn hovered(mut self, i: Option<usize>) -> Self {
        self.hovered = i;
        self
    }
    /// Called with the category the pointer enters, and `None` when it leaves the chart.
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S, Option<usize>) + 'static) -> Self {
        self.hover = Some(hover(f));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let ink = self.color.unwrap_or_else(|| series_color(k, 0));
        let hovered = self.hovered.filter(|&i| i < self.bars.len());
        let tint = |i: usize| if hovered.is_some_and(|h| h != i) { ink.alpha(ink.a * 0.45) } else { ink };
        let scale = Scale::new(self.bars.iter().map(|b| b.1), self.range);
        let mut root = col().key(self.key.clone()).role("group").w_full().gap(6.0);
        if let Some(l) = &self.label {
            root = root.label(l.clone());
        }
        if self.horizontal {
            return root.child(self.rows(k, &scale, &tint));
        }
        let n = self.bars.len();
        let zero = (1.0 - scale.frac(0.0)) * VIEW;
        let half = 0.35 * VIEW / n.max(1) as f32;
        let mut marks = Shape::new();
        for (i, (_, v)) in self.bars.iter().enumerate() {
            if !v.is_finite() {
                continue;
            }
            let (x, y) = (band_x(i, n), (1.0 - scale.frac(*v)) * VIEW);
            marks = marks.fill(vec![[x - half, zero], [x + half, zero], [x + half, y], [x - half, y]], tint(i));
        }
        if self.hover.is_some() {
            let mut head = row().key("readout").h(16.0).gap(6.0).center_y();
            if let Some((name, v)) = hovered.and_then(|i| self.bars.get(i)) {
                head = head
                    .child(text(name.clone()).font(label_font(k)).color(k.c.text_weak).single_line())
                    .child(text(show(&self.format, *v)).font(label_font(k)).color(k.c.text_base).single_line());
            }
            root = root.child(head);
        }
        let labels: Vec<String> = self.bars.iter().map(|b| b.0.clone()).collect();
        let frame = Frame {
            key: &self.key,
            scale: &scale,
            labels: &labels,
            items: self.bars.iter().map(|(name, v)| format!("{name}: {}", show(&self.format, *v))).collect(),
            bands: n,
            height: self.height,
            format: &self.format,
            hover: self.hover.as_ref(),
        };
        root.child(frame.build(k, marks))
    }

    /// Horizontal bars: a name column, the bars from a shared zero, and each value at its row's end.
    fn rows(&self, k: &Kit, scale: &Scale, tint: &dyn Fn(usize) -> Color) -> El {
        let font = label_font(k);
        let zero = scale.frac(0.0) * VIEW;
        let mut names = col().key("names").gap(4.0).a11y_hidden();
        let mut bars = col().key("bands").role("list").grow(1.0).min_w(0.0).gap(4.0);
        for (i, (name, v)) in self.bars.iter().enumerate() {
            names = names.child(text(name.clone()).font(font).color(k.c.text_weak).single_line());
            let mut shape = Shape::new();
            if v.is_finite() {
                let x = scale.frac(*v) * VIEW;
                shape = shape.fill(vec![[zero, 15.0], [x, 15.0], [x, 85.0], [zero, 85.0]], tint(i));
            }
            let value = show(&self.format, *v);
            let mut line = row()
                .key(format!("{}-band-{i}", self.key))
                .role("listitem")
                .label(format!("{name}: {value}"))
                .position_in_set(i + 1, self.bars.len())
                .h(font.line_height)
                .gap(6.0)
                .center_y()
                .child(canvas(shape.view_box(VIEW, VIEW)).grow(1.0).min_w(0.0).h_full().a11y_hidden())
                .child(text(value).font(font).color(k.c.text_base).single_line().a11y_hidden());
            if let Some(h) = &self.hover {
                line = h(line, Some(i));
            }
            bars = bars.child(line);
        }
        let mut body = row().key(format!("{}-plot", self.key)).w_full().gap(8.0).items_start().child(names).child(bars);
        if let Some(h) = &self.hover {
            body = h(body, None);
        }
        body
    }
}
