use std::rc::Rc;

use rune::{El, LineCap, Shape, col, row, text};

use super::{Format, Frame, Hover, Scale, VIEW, band_x, hover, key_item, label_font, plain, series_color, show};
use crate::Kit;

struct Series {
    name: String,
    values: Vec<f32>,
    color: Option<rune::Color>,
}

/// One or more series of values over shared x labels (days, runs, versions), each drawn as a
/// line through the centres of equal x bands, with value ticks on the left and the labels under
/// their bands. A non-finite value (`f32::NAN`) leaves a gap in its line.
///
/// ```
/// # use rune_kit::{Kit, LineChart};
/// # struct S { hover: Option<usize> }
/// # let k = Kit::neutral(true);
/// # let s = S { hover: None };
/// # let _ =
/// LineChart::new("cpu")
///     .label("CPU and memory")
///     .labels(["Mon", "Tue", "Wed", "Thu", "Fri"])
///     .series("CPU", [12.0, 30.0, 22.0, 41.0, 35.0])
///     .series("Memory", [40.0, 42.0, 45.0, 44.0, 50.0])
///     .format(|v| format!("{v:.0}%"))
///     .hovered(s.hover)
///     .on_hover(|s: &mut S, i| s.hover = i)
///     .build(&k)
/// # ;
/// ```
pub struct LineChart {
    key: String,
    label: Option<String>,
    labels: Vec<String>,
    series: Vec<Series>,
    range: Option<[f32; 2]>,
    height: f32,
    width: f32,
    format: Format,
    hovered: Option<usize>,
    hover: Option<Hover>,
}

impl LineChart {
    /// `key` names the chart's elements; give each chart on screen its own.
    pub fn new(key: impl Into<String>) -> LineChart {
        LineChart {
            key: key.into(),
            label: None,
            labels: Vec::new(),
            series: Vec::new(),
            range: None,
            height: 160.0,
            width: 2.0,
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
    /// The x labels, one per value; the series' longest sets how many bands there are.
    pub fn labels<I: IntoIterator<Item = T>, T: Into<String>>(mut self, labels: I) -> Self {
        self.labels = labels.into_iter().map(Into::into).collect();
        self
    }
    /// Adds a series in the next theme colour.
    pub fn series(mut self, name: impl Into<String>, values: impl Into<Vec<f32>>) -> Self {
        self.series.push(Series { name: name.into(), values: values.into(), color: None });
        self
    }
    /// Adds a series in its own colour.
    pub fn series_colored(mut self, name: impl Into<String>, values: impl Into<Vec<f32>>, color: rune::Color) -> Self {
        self.series.push(Series { name: name.into(), values: values.into(), color: Some(color) });
        self
    }
    /// The value axis's ends; by default the data's, taking in zero and rounded out.
    pub fn range(mut self, lo: f32, hi: f32) -> Self {
        self.range = Some([lo, hi]);
        self
    }
    /// The plot's height in points (160 by default); the chart fills its width.
    pub fn height(mut self, h: f32) -> Self {
        self.height = h.max(1.0);
        self
    }
    /// The lines' width in points (2 by default).
    pub fn line_width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
    /// How ticks and the readout write values (two decimals at most by default).
    pub fn format(mut self, f: impl Fn(f32) -> String + 'static) -> Self {
        self.format = Rc::new(f);
        self
    }
    /// The x index under the pointer, from [`Self::on_hover`]: draws a guide, marks each
    /// series' point and reads their values out above the plot.
    pub fn hovered(mut self, i: Option<usize>) -> Self {
        self.hovered = i;
        self
    }
    /// Called with the x index the pointer enters, and `None` when it leaves the plot.
    pub fn on_hover<S: 'static>(mut self, f: impl Fn(&mut S, Option<usize>) + 'static) -> Self {
        self.hover = Some(hover(f));
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let n = self.series.iter().map(|s| s.values.len()).max().unwrap_or(0).max(self.labels.len());
        let scale = Scale::new(self.series.iter().flat_map(|s| s.values.iter().copied()), self.range);
        let color = |i: usize, s: &Series| s.color.unwrap_or_else(|| series_color(k, i));
        let hovered = self.hovered.filter(|&i| i < n);
        let y = |v: f32| (1.0 - scale.frac(v)) * VIEW;
        let mut marks = Shape::new();
        if let Some(i) = hovered {
            let x = band_x(i, n);
            marks = marks.line([x, 0.0], [x, VIEW], 1.0, LineCap::Butt, k.c.border_base);
        }
        for (si, s) in self.series.iter().enumerate() {
            let ink = color(si, s);
            for run in runs(&s.values) {
                let pts: Vec<[f32; 2]> = run.map(|(i, v)| [band_x(i, n), y(v)]).collect();
                marks = if pts.len() == 1 {
                    dot(marks, pts[0], self.width * 2.0, ink)
                } else {
                    marks.stroke(pts, self.width, ink)
                };
            }
            if let Some(v) = hovered.and_then(|i| s.values.get(i)).filter(|v| v.is_finite()) {
                marks = dot(marks, [band_x(hovered.unwrap_or(0), n), y(*v)], self.width * 3.5, ink);
            }
        }

        let mut root = col().key(self.key.clone()).role("group").w_full().gap(6.0);
        if let Some(l) = &self.label {
            root = root.label(l.clone());
        }
        // The readout replaces the legend while hovered, so the plot doesn't move.
        let header = self.series.len() > 1 || self.hover.is_some();
        if header {
            let mut head = row().key("readout").h(16.0).gap(12.0).center_y().min_w(0.0);
            if let Some(l) = hovered.and_then(|i| self.labels.get(i)) {
                head = head.child(text(l.clone()).font(label_font(k)).color(k.c.text_weak).single_line());
            }
            for (si, s) in self.series.iter().enumerate() {
                let words = match hovered {
                    Some(i) => {
                        format!("{} {}", s.name, self.value(s, i))
                    }
                    None if self.series.len() > 1 => s.name.clone(),
                    None => continue,
                };
                head = head.child(key_item(k, color(si, s), words));
            }
            root = root.child(head);
        }
        let items = (0..n).map(|i| self.item(i)).collect();
        let frame = Frame {
            key: &self.key,
            scale: &scale,
            labels: &self.labels,
            items,
            bands: n,
            height: self.height,
            format: &self.format,
            hover: self.hover.as_ref(),
        };
        root.child(frame.build(k, marks))
    }
}

impl LineChart {
    /// Series `s`'s value at `i`, a dash where it has none.
    fn value(&self, s: &Series, i: usize) -> String {
        show(&self.format, s.values.get(i).copied().unwrap_or(f32::NAN))
    }

    /// What assistive technology reads for band `i`: `Tue: CPU 30%, Memory 42%`.
    fn item(&self, i: usize) -> String {
        let values: Vec<String> = self.series.iter().map(|s| format!("{} {}", s.name, self.value(s, i))).collect();
        let label = self.labels.get(i).cloned().unwrap_or_else(|| (i + 1).to_string());
        format!("{label}: {}", values.join(", "))
    }
}

/// A round dot `d` points across at `at` (a zero-length round-capped stroke).
fn dot(s: Shape, at: [f32; 2], d: f32, ink: rune::Color) -> Shape {
    s.line(at, [at[0] + 1e-3, at[1]], d, LineCap::Round, ink)
}

/// The runs of consecutive finite values, with their indices.
pub(super) fn runs(values: &[f32]) -> impl Iterator<Item = impl Iterator<Item = (usize, f32)> + '_> + '_ {
    values
        .split(|v| !v.is_finite())
        .scan(0usize, |start, run| {
            let at = *start;
            *start += run.len() + 1;
            Some((at, run))
        })
        .filter(|(_, run)| !run.is_empty())
        .map(|(at, run)| run.iter().enumerate().map(move |(j, &v)| (at + j, v)))
}
