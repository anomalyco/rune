//! Data charts drawn from app values: [`LineChart`] (one or more series over shared x labels)
//! and [`BarChart`] (categories, upright or on their side).
//!
//! Charts read only kit tokens, so they follow the theme's light and dark palettes: series take
//! the accent and then the status colours in turn, gridlines `border_weak`, labels `text_weak`.
//! Points sit at the centres of equal x bands, which also serve as hover targets. Hover is the
//! app's state, as a [`crate::Tooltip`]'s `open` is: pass it back with `.hovered(i)` and update
//! it from `.on_hover(|s, i| ..)`; the chart then draws a guide and a readout of that index.

use std::rc::Rc;

use rune::{Color, El, Shape, TextStyle, canvas, col, el, row, text};

use crate::Kit;

mod bar;
mod line;

pub use bar::BarChart;
pub use line::LineChart;

/// Attaches the app's hover handler to a band (`Some(i)`, on entry) or to the plot (`None`,
/// on leaving it).
pub(crate) type Hover = Rc<dyn Fn(El, Option<usize>) -> El>;

/// Formats a value for axis ticks and readouts.
pub(crate) type Format = Rc<dyn Fn(f32) -> String>;

pub(crate) fn hover<S: 'static>(f: impl Fn(&mut S, Option<usize>) + 'static) -> Hover {
    let f = Rc::new(f);
    Rc::new(move |e: El, at: Option<usize>| {
        let f = f.clone();
        match at {
            Some(i) => e.on_hover(move |s: &mut S| f(s, Some(i))),
            None => e.on_hover_change(move |s: &mut S, inside: bool, _| {
                if !inside {
                    f(s, None)
                }
            }),
        }
    })
}

/// A round step near `raw`: 1, 2 or 5 times a power of ten.
pub(crate) fn nice(raw: f32) -> f32 {
    let p = 10f32.powf(raw.max(1e-9).log10().floor());
    let m = raw / p;
    p * if m <= 1.0 {
        1.0
    } else if m <= 2.0 {
        2.0
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    }
}

/// A value axis: its ends, rounded out to whole steps and taking in zero, and the ticks between.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Scale {
    pub lo: f32,
    pub hi: f32,
    pub ticks: Vec<f32>,
}

impl Scale {
    /// The scale over the finite `values` (zero and one when there are none), or over `range`.
    pub fn new(values: impl IntoIterator<Item = f32>, range: Option<[f32; 2]>) -> Scale {
        let (lo, hi, step) = match range {
            Some([a, b]) if a.is_finite() && b.is_finite() && a != b => {
                let (lo, hi) = (a.min(b), a.max(b));
                (lo, hi, nice((hi - lo) / 4.0))
            }
            _ => {
                let (lo, hi) = values
                    .into_iter()
                    .filter(|v| v.is_finite())
                    .fold((0.0f32, 0.0f32), |(a, b), v| (a.min(v), b.max(v)));
                let hi = if hi - lo < 1e-9 { lo + 1.0 } else { hi };
                let step = nice((hi - lo) / 4.0);
                ((lo / step).floor() * step, (hi / step).ceil() * step, step)
            }
        };
        let first = (lo / step - 1e-3).ceil();
        let ticks = (0..12)
            .map(|k| (first + k as f32) * step)
            .take_while(|t| *t <= hi + step * 1e-3)
            .map(|t| if t.abs() < step * 1e-3 { 0.0 } else { t })
            .collect();
        Scale { lo, hi, ticks }
    }

    /// Where `v` falls between the ends, 0 at `lo` and 1 at `hi` (clamped).
    pub fn frac(&self, v: f32) -> f32 {
        ((v - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0)
    }
}

/// Series `i`'s colour: the accent, then success, warning, danger and info, then round again.
pub(crate) fn series_color(k: &Kit, i: usize) -> Color {
    let c = &k.c;
    [c.icon_accent, c.success.icon, c.warning.icon, c.danger.icon, c.info.icon][i % 5]
}

/// Up to two decimals, without trailing zeros (`1.5`, `20`, `-0.25`).
pub(crate) fn plain(v: f32) -> String {
    if !v.is_finite() {
        return "–".to_owned();
    }
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

/// `v` through the app's format, or a dash for a gap (a non-finite value), which no format sees.
pub(crate) fn show(format: &Format, v: f32) -> String {
    if v.is_finite() { format(v) } else { "–".to_owned() }
}

pub(crate) fn label_font(k: &Kit) -> TextStyle {
    let t = k.ty.small.lh(16.0);
    if t.features.is_empty() { t.features("'tnum' 1") } else { t }
}

/// Every how many bands an x label shows, so at most `max` of `n` do.
pub(crate) fn label_stride(n: usize, max: usize) -> usize {
    n.div_ceil(max.max(1)).max(1)
}

/// A coloured key and its text: `● name` or `● name value`.
pub(crate) fn key_item(k: &Kit, color: Color, words: String) -> El {
    row()
        .gap(4.0)
        .center_y()
        .child(el().size(8.0, 8.0).radius(2.0).fill(color).a11y_hidden())
        .child(text(words).font(label_font(k)).color(k.c.text_base).single_line())
}

/// The frame shared by line and upright bar charts: y ticks on the left, gridlines and `marks`
/// in the plot, x labels under their bands, and a band per x label. Assistive technology reads
/// the bands as a list whose items say their label and values (`items`); with a hover handler
/// they are also the hover targets.
pub(crate) struct Frame<'a> {
    pub key: &'a str,
    pub scale: &'a Scale,
    pub labels: &'a [String],
    pub items: Vec<String>,
    pub bands: usize,
    pub height: f32,
    pub format: &'a Format,
    pub hover: Option<&'a Hover>,
}

/// The plot's coordinates: `[0, 100]` both ways, y down.
pub(crate) const VIEW: f32 = 100.0;

impl Frame<'_> {
    pub fn build(self, k: &Kit, marks: Shape) -> El {
        let c = &k.c;
        let font = label_font(k);
        let lh = font.line_height;
        let s = self.scale;
        let mut grid = Shape::new();
        for &t in &s.ticks {
            let y = (1.0 - s.frac(t)) * VIEW;
            let ink = if t == 0.0 && s.lo < 0.0 { c.border_base } else { c.border_weak };
            grid = grid.line([0.0, y], [VIEW, y], 1.0, rune::LineCap::Butt, ink);
        }
        // Each label is a line tall and centred on its gridline: the column starts half a line
        // above the plot, and fixed spacers put each label's top at its line's height.
        let mut axis = col().key("y-axis").h(self.height + lh).offset(0.0, -lh / 2.0).mr(6.0).items_end().a11y_hidden();
        let mut top = 0.0;
        for (i, &t) in s.ticks.iter().rev().enumerate() {
            let at = (1.0 - s.frac(t)) * self.height;
            axis = axis.child(el().h((at - top).max(0.0)));
            axis = axis
                .child(text((self.format)(t)).key(format!("y-{i}")).font(font).color(c.text_weak).single_line().h(lh));
            top = at.max(top) + lh;
        }
        let mut plot = el()
            .key(format!("{}-plot", self.key))
            .w_full()
            .h(self.height)
            .child(canvas(grid.view_box(VIEW, VIEW)).key("grid").absolute(0.0, 0.0).w_full().h_full().a11y_hidden())
            .child(canvas(marks.view_box(VIEW, VIEW)).key("marks").absolute(0.0, 0.0).w_full().h_full().a11y_hidden());
        let n = self.bands.max(1);
        let mut hits = row().key("bands").role("list").absolute(0.0, 0.0).w_full().h_full();
        for (i, item) in self.items.into_iter().enumerate() {
            let band = el()
                .key(format!("{}-band-{i}", self.key))
                .role("listitem")
                .label(item)
                .position_in_set(i + 1, self.bands)
                .w_frac(1.0 / n as f32)
                .h_full();
            hits = hits.child(match self.hover {
                Some(h) => h(band, Some(i)),
                None => band,
            });
        }
        plot = plot.child(hits);
        if let Some(h) = self.hover {
            plot = h(plot, None);
        }
        let stride = label_stride(self.bands, 12);
        let mut xs = row().key("x-axis").w_full().pt(4.0).a11y_hidden();
        for i in 0..self.bands {
            let words = if i % stride == 0 { self.labels.get(i).cloned().unwrap_or_default() } else { String::new() };
            xs = xs.child(
                row()
                    .w_frac(1.0 / n as f32)
                    .justify_center()
                    .child(text(words).font(font).color(c.text_weak).single_line()),
            );
        }
        row().key("frame").w_full().items_start().child(axis).child(col().grow(1.0).min_w(0.0).child(plot).child(xs))
    }
}

/// The x of band `i`'s centre in [`VIEW`] units.
pub(crate) fn band_x(i: usize, n: usize) -> f32 {
    (i as f32 + 0.5) / n.max(1) as f32 * VIEW
}

#[cfg(test)]
mod tests;
