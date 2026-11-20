//! SMIL SVG animation support for icons (`<animate>`, `<animateTransform>`, `<set>`).
//!
//! Static icons pay one cached pointer check (`is_animated`) and stay on the monochrome glyph
//! atlas or static colour-icon cache. Animated icons parse their tracks once, sample them at the
//! runtime's clock `t`, and update one streaming [`Pixels`] handle in place (reusing its image
//! atlas slot). Under reduced motion, Rune draws the static `t = 0` frame through the ordinary
//! atlas path and schedules zero animation frames.

use std::collections::HashMap;
#[cfg(feature = "svg")]
use std::fmt::Write as _;

use rune_core::color::{Color, srgb_encode};
use rune_image::pixels::Pixels;
use swash::zeno::Transform;

use super::{Facing, Icon, attr, numbers, then, transform};

/// CPU bytes of animated icon frames kept before frames unused last frame are dropped.
const BUDGET: usize = 4 * 1024 * 1024;
/// Hard cap on SMIL tracks parsed from one icon (guards against pathological inputs).
const MAX_TRACKS: usize = 32;
/// Hard cap on keyframes per SMIL track.
const MAX_KEYFRAMES: usize = 64;
/// Minimum allowed track duration in seconds.
const MIN_DUR: f32 = 0.016;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttrKind {
    R,
    Cx,
    Cy,
    Rx,
    Ry,
    X,
    Y,
    Width,
    Height,
    X1,
    Y1,
    X2,
    Y2,
    Opacity,
    FillOpacity,
    StrokeOpacity,
    StrokeWidth,
    Transform(TransformKind),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TransformKind {
    Translate,
    Scale,
    Rotate,
    SkewX,
    SkewY,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CalcMode {
    Linear,
    Discrete,
    Spline,
    Paced,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Repeat {
    Indefinite,
    Count(f32),
}

#[derive(Clone, Copy, Debug)]
struct KeyVal {
    v: [f32; 3],
    len: u8,
}

#[derive(Clone, Debug)]
struct Track {
    target: usize,
    attr: AttrKind,
    additive: bool,
    begin: f32,
    dur: f32,
    repeat: Repeat,
    freeze: bool,
    calc_mode: CalcMode,
    values: Vec<KeyVal>,
    key_times: Vec<f32>,
    key_splines: Vec<[f32; 4]>,
}

/// Parsed SMIL tracks for one animated [`Icon`], sorted by target element index.
#[derive(Clone, Debug)]
pub(super) struct Plan {
    tracks: Vec<Track>,
}

/// Evaluated numeric and transform overrides for a single element at time `t`.
#[derive(Default)]
pub(super) struct Overrides {
    attrs: Vec<(AttrKind, f32)>,
    transforms: Vec<(Transform, bool)>,
}

impl Overrides {
    pub(super) fn get(&self, kind: AttrKind) -> Option<f32> {
        self.attrs.iter().rev().find(|(k, _)| *k == kind).map(|(_, v)| *v)
    }

    pub(super) fn apply_transform(&self, base: Option<&str>) -> Option<Transform> {
        if self.transforms.is_empty() {
            return base.map(transform);
        }
        let mut cur = if self.transforms[0].1 { base.map(transform).unwrap_or(Transform::IDENTITY) } else { Transform::IDENTITY };
        for &(tr, additive) in &self.transforms {
            cur = if additive { then(tr, cur) } else { tr };
        }
        Some(cur)
    }
}

impl Plan {
    /// Whether any track is still advancing at time `t` seconds.
    pub(super) fn active_at(&self, t: f64) -> bool {
        self.tracks.iter().any(|tr| match tr.repeat {
            Repeat::Indefinite => true,
            Repeat::Count(n) => t < f64::from(tr.begin) + f64::from(tr.dur) * f64::from(n),
        })
    }

    /// Evaluates all tracks targeting element `target` at time `t` seconds.
    pub(super) fn sample_element(&self, target: usize, t: f64) -> Option<Overrides> {
        let mut out: Option<Overrides> = None;
        for tr in self.tracks.iter().filter(|tr| tr.target == target) {
            let Some(val) = sample_track(tr, t) else { continue };
            let ov = out.get_or_insert_with(Overrides::default);
            match tr.attr {
                AttrKind::Transform(kind) => ov.transforms.push((build_transform(kind, val), tr.additive)),
                other => ov.attrs.push((other, val.v[0])),
            }
        }
        out
    }
}

/// Parses SMIL animation elements in `body`. Returns `None` when `body` has no valid tracks.
pub(super) fn parse(body: &str) -> Option<Plan> {
    if !body.contains("<animate") && !body.contains("<set") {
        return None;
    }
    let mut stack: Vec<usize> = Vec::new();
    let mut tracks: Vec<Track> = Vec::new();
    for (idx, raw) in body.split('>').enumerate() {
        let el = raw.trim();
        if el.is_empty() || el.starts_with("<!") || el.starts_with("<?") {
            continue;
        }
        if el.starts_with("</") {
            stack.pop();
            continue;
        }
        let tag = el.trim_start_matches('<').split_whitespace().next().unwrap_or("").trim_end_matches('/');
        let open = !el.ends_with('/');
        if matches!(tag, "animate" | "animateTransform" | "set") {
            if tracks.len() < MAX_TRACKS
                && let Some(&target) = stack.last()
                && let Some(track) = parse_track(tag, el, target)
            {
                tracks.push(track);
            }
            if open {
                stack.push(idx);
            }
            continue;
        }
        if open {
            stack.push(idx);
        }
    }
    (!tracks.is_empty()).then_some(Plan { tracks })
}

fn parse_track(tag: &str, el: &str, target: usize) -> Option<Track> {
    let attr_kind = match tag {
        "animateTransform" => {
            if attr(el, "attributeName") != Some("transform") {
                return None;
            }
            let kind = match attr(el, "type").unwrap_or("translate") {
                "translate" => TransformKind::Translate,
                "scale" => TransformKind::Scale,
                "rotate" => TransformKind::Rotate,
                "skewX" => TransformKind::SkewX,
                "skewY" => TransformKind::SkewY,
                _ => return None,
            };
            AttrKind::Transform(kind)
        }
        _ => match attr(el, "attributeName")? {
            "r" => AttrKind::R,
            "cx" => AttrKind::Cx,
            "cy" => AttrKind::Cy,
            "rx" => AttrKind::Rx,
            "ry" => AttrKind::Ry,
            "x" => AttrKind::X,
            "y" => AttrKind::Y,
            "width" => AttrKind::Width,
            "height" => AttrKind::Height,
            "x1" => AttrKind::X1,
            "y1" => AttrKind::Y1,
            "x2" => AttrKind::X2,
            "y2" => AttrKind::Y2,
            "opacity" => AttrKind::Opacity,
            "fill-opacity" => AttrKind::FillOpacity,
            "stroke-opacity" => AttrKind::StrokeOpacity,
            "stroke-width" => AttrKind::StrokeWidth,
            _ => return None,
        },
    };
    let begin = attr(el, "begin").and_then(parse_clock).unwrap_or(0.0).max(0.0);
    let dur = if tag == "set" {
        attr(el, "dur").and_then(parse_clock).unwrap_or(f32::INFINITY)
    } else {
        let d = attr(el, "dur").and_then(parse_clock)?;
        if !d.is_finite() || d <= 0.0 {
            return None;
        }
        d.max(MIN_DUR)
    };
    let repeat = match attr(el, "repeatCount").map(str::trim) {
        Some("indefinite") => Repeat::Indefinite,
        Some(s) => {
            let n = s.parse::<f32>().ok()?;
            if !n.is_finite() || n <= 0.0 {
                return None;
            }
            Repeat::Count(n)
        }
        None => Repeat::Count(1.0),
    };
    let freeze = attr(el, "fill") == Some("freeze");
    let additive = attr(el, "additive") == Some("sum");
    let values = if tag == "set" {
        let to = parse_key_val(attr(el, "to")?)?;
        vec![to]
    } else if let Some(list) = attr(el, "values") {
        let mut out = Vec::new();
        for part in list.split(';') {
            let p = part.trim();
            if p.is_empty() {
                continue;
            }
            if out.len() >= MAX_KEYFRAMES {
                break;
            }
            out.push(parse_key_val(p)?);
        }
        out
    } else {
        let from = parse_key_val(attr(el, "from")?)?;
        let to = parse_key_val(attr(el, "to")?)?;
        vec![from, to]
    };
    if values.is_empty() {
        return None;
    }
    let calc_mode = if tag == "set" {
        CalcMode::Discrete
    } else {
        match attr(el, "calcMode") {
            Some("discrete") => CalcMode::Discrete,
            Some("spline") => CalcMode::Spline,
            Some("paced") => CalcMode::Paced,
            _ => CalcMode::Linear,
        }
    };
    let key_times = parse_key_times(attr(el, "keyTimes"), values.len(), calc_mode);
    let key_splines = if calc_mode == CalcMode::Spline {
        parse_key_splines(attr(el, "keySplines"), values.len().saturating_sub(1))
    } else {
        Vec::new()
    };
    Some(Track { target, attr: attr_kind, additive, begin, dur, repeat, freeze, calc_mode, values, key_times, key_splines })
}

fn parse_clock(raw: &str) -> Option<f32> {
    let s = raw.trim();
    let v = if let Some(ms) = s.strip_suffix("ms") {
        ms.trim().parse::<f32>().ok()? / 1000.0
    } else if let Some(min) = s.strip_suffix("min") {
        min.trim().parse::<f32>().ok()? * 60.0
    } else if let Some(h) = s.strip_suffix('h') {
        h.trim().parse::<f32>().ok()? * 3600.0
    } else if let Some(sec) = s.strip_suffix('s') {
        sec.trim().parse::<f32>().ok()?
    } else {
        s.parse::<f32>().ok()?
    };
    v.is_finite().then_some(v)
}

fn parse_key_val(raw: &str) -> Option<KeyVal> {
    let nums = numbers(raw);
    if nums.is_empty() || nums.iter().any(|n| !n.is_finite()) {
        return None;
    }
    let mut v = [0.0; 3];
    let len = nums.len().min(3);
    v[..len].copy_from_slice(&nums[..len]);
    Some(KeyVal { v, len: len as u8 })
}

fn parse_key_times(raw: Option<&str>, count: usize, mode: CalcMode) -> Vec<f32> {
    if count <= 1 {
        return vec![0.0];
    }
    if let Some(raw) = raw {
        let parsed: Vec<f32> = raw
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<f32>().ok().filter(|v| v.is_finite()))
            .collect();
        if parsed.len() == count && parsed.windows(2).all(|w| w[0] <= w[1]) {
            return parsed;
        }
    }
    let denom = if mode == CalcMode::Discrete { count } else { count - 1 } as f32;
    (0..count).map(|i| i as f32 / denom).collect()
}

fn parse_key_splines(raw: Option<&str>, segments: usize) -> Vec<[f32; 4]> {
    let Some(raw) = raw else { return vec![[0.0, 0.0, 1.0, 1.0]; segments] };
    let mut out = Vec::with_capacity(segments);
    for seg in raw.split(';') {
        let s = seg.trim();
        if s.is_empty() {
            continue;
        }
        let n = numbers(s);
        if n.len() >= 4 && n[..4].iter().all(|v| v.is_finite()) {
            out.push([n[0].clamp(0.0, 1.0), n[1], n[2].clamp(0.0, 1.0), n[3]]);
        } else {
            out.push([0.0, 0.0, 1.0, 1.0]);
        }
    }
    out.resize(segments, [0.0, 0.0, 1.0, 1.0]);
    out
}

fn sample_track(tr: &Track, t: f64) -> Option<KeyVal> {
    let elapsed = t - f64::from(tr.begin);
    if elapsed < 0.0 {
        return None;
    }
    if !tr.dur.is_finite() {
        return tr.values.first().copied();
    }
    let dur = f64::from(tr.dur);
    let u = match tr.repeat {
        Repeat::Indefinite => (elapsed % dur) / dur,
        Repeat::Count(count) => {
            let total = dur * f64::from(count);
            if elapsed >= total {
                return tr.freeze.then(|| tr.values.last().copied()).flatten();
            }
            (elapsed % dur) / dur
        }
    } as f32;
    if tr.values.len() == 1 {
        return Some(tr.values[0]);
    }
    if tr.calc_mode == CalcMode::Discrete {
        let mut idx = 0;
        for (i, &kt) in tr.key_times.iter().enumerate() {
            if u >= kt {
                idx = i;
            }
        }
        return Some(tr.values[idx.min(tr.values.len() - 1)]);
    }
    let seg_count = tr.values.len() - 1;
    let mut seg = seg_count - 1;
    for i in 0..seg_count {
        if u < tr.key_times[i + 1] {
            seg = i;
            break;
        }
    }
    let t0 = tr.key_times[seg];
    let t1 = tr.key_times[seg + 1];
    let span = t1 - t0;
    let raw = if span <= 1e-6 { 1.0 } else { ((u - t0) / span).clamp(0.0, 1.0) };
    let frac = match tr.calc_mode {
        CalcMode::Spline => {
            let [x1, y1, x2, y2] = tr.key_splines.get(seg).copied().unwrap_or([0.0, 0.0, 1.0, 1.0]);
            cubic_bezier(x1, y1, x2, y2, raw)
        }
        _ => raw,
    };
    let a = tr.values[seg];
    let b = tr.values[seg + 1];
    let len = a.len.max(b.len);
    let mut v = [0.0; 3];
    for i in 0..len as usize {
        v[i] = a.v[i] + (b.v[i] - a.v[i]) * frac;
    }
    Some(KeyVal { v, len })
}

fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, x: f32) -> f32 {
    if (x1 - y1).abs() < 1e-6 && (x2 - y2).abs() < 1e-6 {
        return x;
    }
    let x = x.clamp(0.0, 1.0);
    let sample = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let deriv = |a: f32, b: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = x;
    for _ in 0..8 {
        let err = sample(x1, x2, s) - x;
        if err.abs() < 1e-5 {
            return sample(y1, y2, s);
        }
        let d = deriv(x1, x2, s);
        if d.abs() < 1e-6 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    s = x;
    for _ in 0..20 {
        let mid = 0.5 * (lo + hi);
        if sample(x1, x2, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
        s = mid;
    }
    sample(y1, y2, s)
}

fn build_transform(kind: TransformKind, val: KeyVal) -> Transform {
    match kind {
        TransformKind::Translate => Transform::translation(val.v[0], if val.len >= 2 { val.v[1] } else { 0.0 }),
        TransformKind::Scale => Transform::scale(val.v[0], if val.len >= 2 { val.v[1] } else { val.v[0] }),
        TransformKind::Rotate => {
            let (sin, cos) = f64::from(val.v[0]).to_radians().sin_cos();
            let (cx, cy) = (f64::from(val.v[1]), f64::from(val.v[2]));
            let (tx, ty) = (cx - cos * cx + sin * cy, cy - sin * cx - cos * cy);
            Transform::new(cos as f32, sin as f32, -sin as f32, cos as f32, tx as f32, ty as f32)
        }
        TransformKind::SkewX => Transform::new(1.0, 0.0, val.v[0].to_radians().tan(), 1.0, 0.0, 0.0),
        TransformKind::SkewY => Transform::new(1.0, val.v[0].to_radians().tan(), 0.0, 1.0, 0.0, 0.0),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FrameKey {
    icon: usize,
    px: u32,
    size_bits: u32,
    facing: u8,
    rgb: [u8; 3],
    one: bool,
}

struct FrameEntry {
    pixels: Pixels,
    last_t: f64,
    used: bool,
}

/// Per-`TextSystem` cache of parsed SMIL plans and streaming RGBA frames for animated icons.
#[derive(Default)]
pub(crate) struct AnimatedIcons {
    plans: HashMap<usize, Option<Plan>>,
    frames: HashMap<FrameKey, FrameEntry>,
    bytes: usize,
}

impl AnimatedIcons {
    pub(crate) fn begin_frame(&mut self) {
        if self.bytes > BUDGET {
            self.frames.retain(|_, e| e.used);
            self.bytes = self.frames.values().map(|e| bytes_of(&e.pixels)).sum();
        }
        self.frames.values_mut().for_each(|e| e.used = false);
    }

    fn plan(&mut self, icon: &'static Icon) -> Option<&Plan> {
        let icon = super::swapped(icon);
        let at = icon as *const Icon as usize;
        self.plans.entry(at).or_insert_with(|| parse(icon.body)).as_ref()
    }

    pub(crate) fn is_animated(&mut self, icon: &'static Icon) -> bool {
        self.plan(icon).is_some()
    }

    /// Rasterizes animated `icon` at time `t` seconds into its reused [`Pixels`] handle.
    /// Returns `(&Pixels, still_animating)`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn frame(
        &mut self,
        icon: &'static Icon,
        (px, size): (u32, f32),
        color: Color,
        one: bool,
        is_color: bool,
        facing: Facing,
        t: f64,
    ) -> Option<(&Pixels, bool)> {
        if px == 0 || px > super::super::ATLAS_SIZE {
            return None;
        }
        let icon = super::swapped(icon);
        let at = icon as *const Icon as usize;
        let plan = self.plans.entry(at).or_insert_with(|| parse(icon.body)).clone()?;
        let active = plan.active_at(t);
        let rgb = [color.r, color.g, color.b].map(|v| (srgb_encode(v) * 255.0).round() as u8);
        let key = FrameKey {
            icon: at,
            px,
            size_bits: size.to_bits(),
            facing: u8::from(facing == Facing::Mirrored),
            rgb,
            one,
        };
        let entry = self.frames.entry(key).or_insert_with(|| {
            let pixels = Pixels::new();
            self.bytes += (px as usize) * (px as usize) * 4;
            FrameEntry { pixels, last_t: f64::NAN, used: false }
        });
        if entry.last_t.is_nan() || (t - entry.last_t).abs() > 1e-6 {
            let rgba = if is_color {
                rasterize_color_frame(icon, &plan, px, color, one, facing, t)?
            } else {
                rasterize_mono_frame(icon, &plan, (px, size), rgb, facing, t)
            };
            entry.pixels.set(px, px, rgba);
            entry.last_t = t;
        }
        entry.used = true;
        Some((&entry.pixels, active))
    }
}

fn rasterize_mono_frame(icon: &Icon, plan: &Plan, (px, size): (u32, f32), rgb: [u8; 3], facing: Facing, t: f64) -> Vec<u8> {
    let mut cov = super::icon_coverage_with_plan(icon, px, size, swash::zeno::Point::new(0.0, 0.0), Some((plan, t)));
    if facing == Facing::Mirrored {
        cov.chunks_exact_mut(px as usize).for_each(<[u8]>::reverse);
    }
    let mut rgba = Vec::with_capacity(cov.len() * 4);
    for a in cov {
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
    }
    rgba
}

#[cfg(feature = "svg")]
fn rasterize_color_frame(icon: &Icon, plan: &Plan, px: u32, color: Color, one: bool, facing: Facing, t: f64) -> Option<Vec<u8>> {
    let hex = &color.hex()[..7];
    let body = rewrite_svg_at(icon.body, plan, t);
    let body = if body.contains("currentColor") { body.replace("currentColor", hex) } else { body };
    let v = icon.view;
    let doc = format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {v} {v}">{body}</svg>"#);
    let (w, h, mut rgba) = rune_image::svg_rgba(doc.as_bytes(), px).ok()?;
    if w != px || h != px {
        return None;
    }
    if facing == Facing::Mirrored {
        for row in rgba.chunks_exact_mut(px as usize * 4) {
            let mut l = 0;
            let mut r = px as usize - 1;
            while l < r {
                for c in 0..4 {
                    row.swap(l * 4 + c, r * 4 + c);
                }
                l += 1;
                r -= 1;
            }
        }
    }
    if one {
        let rgb = [color.r, color.g, color.b].map(|v| (srgb_encode(v) * 255.0).round() as u8);
        rgba.chunks_exact_mut(4).for_each(|p| p[..3].copy_from_slice(&rgb));
    }
    Some(rgba)
}

#[cfg(not(feature = "svg"))]
fn rasterize_color_frame(_: &Icon, _: &Plan, _: u32, _: Color, _: bool, _: Facing, _: f64) -> Option<Vec<u8>> {
    None
}

#[cfg(feature = "svg")]
fn strip_attr(tag: &str, name: &str) -> String {
    let mut out = String::with_capacity(tag.len());
    let mut rest = tag;
    while let Some(i) = rest.find(name) {
        let before = rest[..i].chars().last();
        let after = &rest[i + name.len()..];
        if before.is_some_and(|c| c.is_whitespace())
            && after.starts_with("=\"")
            && let Some(end) = after[2..].find('"')
        {
            out.push_str(rest[..i].trim_end_matches(|c: char| c.is_whitespace()));
            rest = &after[2 + end + 1..];
            continue;
        }
        out.push_str(&rest[..i + name.len()]);
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(feature = "svg")]
fn rewrite_svg_at(body: &str, plan: &Plan, t: f64) -> String {
    let mut out = String::with_capacity(body.len());
    for (idx, raw) in body.split('>').enumerate() {
        let el = raw.trim();
        if el.is_empty() {
            continue;
        }
        let tag = el.trim_start_matches('<').split_whitespace().next().unwrap_or("").trim_end_matches('/');
        if matches!(tag, "animate" | "animateTransform" | "set") || el.starts_with("</animate") || el.starts_with("</set") {
            continue;
        }
        if let Some(ov) = plan.sample_element(idx, t) {
            let self_close = el.ends_with('/');
            let mut head = el.trim_end_matches('/').to_owned();
            for &(kind, _) in &ov.attrs {
                let name = match kind {
                    AttrKind::R => "r",
                    AttrKind::Cx => "cx",
                    AttrKind::Cy => "cy",
                    AttrKind::Rx => "rx",
                    AttrKind::Ry => "ry",
                    AttrKind::X => "x",
                    AttrKind::Y => "y",
                    AttrKind::Width => "width",
                    AttrKind::Height => "height",
                    AttrKind::X1 => "x1",
                    AttrKind::Y1 => "y1",
                    AttrKind::X2 => "x2",
                    AttrKind::Y2 => "y2",
                    AttrKind::Opacity => "opacity",
                    AttrKind::FillOpacity => "fill-opacity",
                    AttrKind::StrokeOpacity => "stroke-opacity",
                    AttrKind::StrokeWidth => "stroke-width",
                    AttrKind::Transform(_) => continue,
                };
                head = strip_attr(&head, name);
            }
            let tr = ov.apply_transform(attr(el, "transform"));
            if tr.is_some() {
                head = strip_attr(&head, "transform");
            }
            out.push_str(&head);
            for &(kind, val) in &ov.attrs {
                let name = match kind {
                    AttrKind::R => "r",
                    AttrKind::Cx => "cx",
                    AttrKind::Cy => "cy",
                    AttrKind::Rx => "rx",
                    AttrKind::Ry => "ry",
                    AttrKind::X => "x",
                    AttrKind::Y => "y",
                    AttrKind::Width => "width",
                    AttrKind::Height => "height",
                    AttrKind::X1 => "x1",
                    AttrKind::Y1 => "y1",
                    AttrKind::X2 => "x2",
                    AttrKind::Y2 => "y2",
                    AttrKind::Opacity => "opacity",
                    AttrKind::FillOpacity => "fill-opacity",
                    AttrKind::StrokeOpacity => "stroke-opacity",
                    AttrKind::StrokeWidth => "stroke-width",
                    AttrKind::Transform(_) => continue,
                };
                let _ = write!(out, r#" {name}="{val}""#);
            }
            if let Some(tr) = tr {
                let _ = write!(out, r#" transform="matrix({} {} {} {} {} {})""#, tr.xx, tr.yx, tr.xy, tr.yy, tr.x, tr.y);
            }
            if self_close {
                out.push('/');
            }
            out.push('>');
        } else {
            out.push_str(el);
            out.push('>');
        }
    }
    out
}

fn bytes_of(p: &Pixels) -> usize {
    let (w, h) = p.size();
    w as usize * h as usize * 4
}
