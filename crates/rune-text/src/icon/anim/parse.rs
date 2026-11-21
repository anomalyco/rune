//! Bounded, pre-parsed monochrome SMIL SVG icon compiler.
//!
//! Enforces hard caps BEFORE allocating on every untrusted input dimension (body length, nesting
//! depth, element count, shape count, opacity-layer count, animation track count, path bytes &
//! commands, polyline points, keyframes, transform ops, clock ranges, and coordinate bounds).
//! Shared cap numbers match OpenCode Desktop's SVG sanitizer (`opencode-plugin-runtime::svg`).

use swash::zeno::{Cap, Fill, Join, Transform};

use super::super::convex;

/// Shared defence-in-depth caps (matching `opencode-plugin-runtime::svg`).
pub const MAX_SVG_BYTES: usize = 32 * 1024;
pub const MAX_DEPTH: usize = 16;
pub const MAX_ELEMENTS: usize = 128;
pub const MAX_SHAPES: usize = 64;
pub const MAX_LAYERS: usize = 8;
pub const MAX_ANIMATIONS: usize = 16;
pub const MAX_PATH_BYTES: usize = 16 * 1024;
pub const MAX_PATH_COMMANDS: usize = 1024;
pub const MAX_POINTS_NUMBERS: usize = 1024;
pub const MAX_KEYFRAMES: usize = 64;
pub const MAX_TRANSFORM_OPS: usize = 8;
pub const MIN_VIEW_DIM: f32 = 1.0;
pub const MAX_VIEW_DIM: f32 = 4096.0;
pub const MIN_DUR_SECS: f32 = 0.016;
pub const MAX_CLOCK_SECS: f32 = 3600.0;
pub const MAX_REPEAT_COUNT: f32 = 10_000.0;
pub const MAX_COORD: f32 = 4096.0;
pub const MAX_STROKE_WIDTH: f32 = 256.0;
pub const MAX_SCALE: f32 = 64.0;
pub const MAX_SKEW_DEG: f32 = 85.0;
pub const MIN_SPLINE_Y: f32 = -4.0;
pub const MAX_SPLINE_Y: f32 = 5.0;
const MAX_ATTRS_PER_TAG: usize = 32;

const NON_RENDERING: [&str; 15] = [
    "defs", "clipPath", "mask", "symbol", "linearGradient", "radialGradient", "pattern", "marker",
    "filter", "title", "desc", "metadata", "style", "script", "foreignObject",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TransformKind {
    Translate,
    Scale,
    Rotate,
    SkewX,
    SkewY,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum CalcMode {
    Linear,
    Discrete,
    Spline,
    Paced,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct KeyVal {
    pub(super) v: [f32; 3],
    pub(super) len: u8,
}

impl KeyVal {
    pub(super) fn scalar(x: f32) -> Self {
        Self { v: [x, 0.0, 0.0], len: 1 }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Track {
    pub(super) target: u16,
    pub(super) attr: AttrKind,
    pub(super) begin: f32,
    /// `None` only for `<set>` with omitted or `"indefinite"` `dur` (one-shot switch at `begin`).
    pub(super) dur: Option<f32>,
    /// `None` means `repeatCount="indefinite"` (only on `<animate>`/`<animateTransform>`).
    pub(super) repeat: Option<f32>,
    pub(super) freeze: bool,
    pub(super) additive_sum: bool,
    pub(super) is_set: bool,
    pub(super) calc_mode: CalcMode,
    pub(super) values: Vec<KeyVal>,
    /// Normalized `[0, 1]` keyTimes, always `values.len()` entries when `values.len() >= 2`.
    pub(super) key_times: Vec<f32>,
    /// Spline control points `(x1, y1, x2, y2)`, always `values.len() - 1` entries when `Spline`.
    pub(super) key_splines: Vec<[f32; 4]>,
}

/// Inherited paint state carried on `<g>` scopes (and resolved onto shapes).
#[derive(Clone, Copy, Debug)]
pub(super) struct PaintProps {
    pub(super) fill: Option<bool>,
    pub(super) stroke: Option<bool>,
    pub(super) stroke_width: Option<f32>,
    pub(super) fill_rule: Option<Fill>,
    pub(super) cap: Option<Cap>,
    pub(super) join: Option<Join>,
    pub(super) miter_limit: Option<f32>,
    pub(super) fill_opacity: Option<f32>,
    pub(super) stroke_opacity: Option<f32>,
}

impl PaintProps {
    fn from_attrs(attrs: &[(&str, &str)]) -> Self {
        let get = |k: &str| find_attr(attrs, k);
        Self {
            fill: get("fill").map(|v| v.trim() != "none"),
            stroke: get("stroke").map(|v| v.trim() != "none"),
            stroke_width: get("stroke-width").and_then(parse_dim).map(|v| v.clamp(0.0, MAX_STROKE_WIDTH)),
            fill_rule: get("fill-rule").map(|v| if v.trim() == "evenodd" { Fill::EvenOdd } else { Fill::NonZero }),
            cap: get("stroke-linecap").map(|v| match v.trim() {
                "round" => Cap::Round,
                "square" => Cap::Square,
                _ => Cap::Butt,
            }),
            join: get("stroke-linejoin").map(|v| match v.trim() {
                "round" => Join::Round,
                "bevel" => Join::Bevel,
                _ => Join::Miter,
            }),
            miter_limit: get("stroke-miterlimit").and_then(parse_finite).map(|v| v.clamp(1.0, 100.0)),
            fill_opacity: get("fill-opacity").and_then(parse_opacity_opt),
            stroke_opacity: get("stroke-opacity").and_then(parse_opacity_opt),
        }
    }
}

#[derive(Clone)]
pub(super) enum BaseGeom {
    Circle { cx: f32, cy: f32, r: f32 },
    Ellipse { cx: f32, cy: f32, rx: f32, ry: f32 },
    Rect { x: f32, y: f32, w: f32, h: f32, rx: Option<f32>, ry: Option<f32> },
    Line { x1: f32, y1: f32, x2: f32, y2: f32 },
    Path(Vec<convex::El>),
}

#[derive(Clone)]
pub(super) enum Op {
    PushGroup {
        node_id: u16,
        transform: Transform,
        opacity: f32,
        display_none: bool,
        paint: PaintProps,
        has_opacity_anim: bool,
    },
    PopGroup,
    DrawShape {
        node_id: u16,
        geom: BaseGeom,
        transform: Transform,
        opacity: f32,
        display_none: bool,
        paint: PaintProps,
    },
}

/// A compiled monochrome animated icon: flat op stream + validated SMIL tracks.
#[derive(Clone)]
pub(crate) struct CompiledAnim {
    pub(super) view: f32,
    pub(super) ops: Vec<Op>,
    pub(super) tracks: Vec<Track>,
    pub(super) node_count: usize,
    /// True when at least one track repeats indefinitely (`repeatCount="indefinite"`).
    pub(super) has_indefinite: bool,
}

/// Fast byte-level pre-check before touching the XML compiler.
#[inline]
pub(crate) fn may_be_animated(body: &str) -> bool {
    body.len() <= MAX_SVG_BYTES && (body.contains("<animate") || body.contains("<set"))
}

/// Result of compiling an SVG icon body against the monochrome SMIL subset and hostile-input caps.
pub(crate) enum CompileOutcome {
    /// Valid monochrome animated SVG with at least one supported SMIL track.
    Animated(CompiledAnim),
    /// Well-formed SVG within all caps, but with no supported SMIL tracks (safe to draw static).
    StaticOnly,
    /// Exceeded a hostile-input cap or contained invalid XML/geometry/transforms (draw blank).
    Rejected,
}

/// Compiles `body` (for an icon with square `view`) into a pre-parsed monochrome animation AST.
pub(crate) fn compile(body: &str, view: f32) -> CompileOutcome {
    compile_inner(body, view).unwrap_or(CompileOutcome::Rejected)
}

fn compile_inner(body: &str, view: f32) -> Option<CompileOutcome> {
    if !may_be_animated(body) || !view.is_finite() || !(MIN_VIEW_DIM..=MAX_VIEW_DIM).contains(&view) {
        return None;
    }
    if body.as_bytes().contains(&0) {
        return None;
    }

    let mut scanner = XmlScanner::new(body);
    let mut ops: Vec<Op> = Vec::new();
    let mut tracks: Vec<Track> = Vec::new();
    let mut stack: Vec<StackEntry> = Vec::new();
    let mut elements = 0usize;
    let mut shapes = 0usize;
    let mut opacity_layers = 0usize;
    let mut next_node_id = 0u16;

    while let Some(tok) = scanner.next_token()? {
        match tok {
            Tok::Close(name) => {
                let top = stack.pop()?;
                if top.tag != name {
                    return None;
                }
                if top.pushed_group {
                    ops.push(Op::PopGroup);
                }
            }
            Tok::Open { tag, attrs, self_closing } => {
                elements += 1;
                if elements > MAX_ELEMENTS {
                    return None;
                }
                if stack.len() + usize::from(!self_closing) > MAX_DEPTH {
                    return None;
                }
                let in_hidden = stack.last().is_some_and(|e| e.skip_subtree);
                if in_hidden || NON_RENDERING.contains(&tag) {
                        if !self_closing {
                            stack.push(StackEntry { tag, node_id: None, pushed_group: false, skip_subtree: true, base_geom: None, base_paint: None, base_opacity: 1.0, has_transform: false });
                        }
                        continue;
                    }
                    match tag {
                        "g" | "a" | "svg" => {
                            let node_id = next_node_id;
                            next_node_id = next_node_id.checked_add(1)?;
                            let opacity = find_attr(&attrs, "opacity").and_then(parse_opacity_opt).unwrap_or(1.0);
                            let display_none = find_attr(&attrs, "display").is_some_and(|v| v.trim() == "none");
                            if opacity < 1.0 {
                                opacity_layers += 1;
                                if opacity_layers > MAX_LAYERS {
                                    return None;
                                }
                            }
                            let tr_attr = find_attr(&attrs, "transform");
                            let tr = match tr_attr {
                                Some(raw) => parse_transform_bounded(raw)?,
                                None => Transform::IDENTITY,
                            };
                            let paint = PaintProps::from_attrs(&attrs);
                            if !self_closing {
                                ops.push(Op::PushGroup { node_id, transform: tr, opacity, display_none, paint, has_opacity_anim: false });
                                stack.push(StackEntry {
                                    tag,
                                    node_id: Some(node_id),
                                    pushed_group: true,
                                    skip_subtree: false,
                                    base_geom: None,
                                    base_paint: Some(paint),
                                    base_opacity: opacity,
                                    has_transform: tr_attr.is_some(),
                                });
                            }
                        }
                        "circle" | "ellipse" | "rect" | "line" | "path" | "polyline" | "polygon" => {
                            shapes += 1;
                            if shapes > MAX_SHAPES {
                                return None;
                            }
                            let geom = parse_geom(tag, &attrs)?;
                            let node_id = next_node_id;
                            next_node_id = next_node_id.checked_add(1)?;
                            let opacity = find_attr(&attrs, "opacity").and_then(parse_opacity_opt).unwrap_or(1.0);
                            let display_none = find_attr(&attrs, "display").is_some_and(|v| v.trim() == "none");
                            let tr_attr = find_attr(&attrs, "transform");
                            let tr = match tr_attr {
                                Some(raw) => parse_transform_bounded(raw)?,
                                None => Transform::IDENTITY,
                            };
                            let paint = PaintProps::from_attrs(&attrs);
                            ops.push(Op::DrawShape { node_id, geom: geom.clone(), transform: tr, opacity, display_none, paint });
                            if !self_closing {
                                stack.push(StackEntry {
                                    tag,
                                    node_id: Some(node_id),
                                    pushed_group: false,
                                    skip_subtree: false,
                                    base_geom: Some(geom),
                                    base_paint: Some(paint),
                                    base_opacity: opacity,
                                    has_transform: tr_attr.is_some(),
                                });
                            }
                        }
                        "animate" | "animateTransform" | "set" => {
                            if let Some(parent) = stack.last()
                                && let Some(target) = parent.node_id
                                && tracks.len() < MAX_ANIMATIONS
                                && let Some(track) = parse_track(tag, &attrs, target, &stack)
                            {
                                if track.attr == AttrKind::Opacity && parent.pushed_group {
                                    // A group whose opacity is animated may drop below 1.0 at runtime:
                                    // count it against MAX_LAYERS now and mark the PushGroup op.
                                    if parent.base_opacity >= 1.0 {
                                        opacity_layers += 1;
                                        if opacity_layers > MAX_LAYERS {
                                            return None;
                                        }
                                    }
                                    for op in ops.iter_mut().rev() {
                                        if let Op::PushGroup { node_id, has_opacity_anim, .. } = op
                                            && *node_id == target
                                        {
                                            *has_opacity_anim = true;
                                            break;
                                        }
                                    }
                                }
                                tracks.push(track);
                            }
                            if !self_closing {
                                stack.push(StackEntry { tag, node_id: None, pushed_group: false, skip_subtree: true, base_geom: None, base_paint: None, base_opacity: 1.0, has_transform: false });
                            }
                        }
                        _ => {
                            if !self_closing {
                                stack.push(StackEntry { tag, node_id: None, pushed_group: false, skip_subtree: true, base_geom: None, base_paint: None, base_opacity: 1.0, has_transform: false });
                            }
                        }
                    }
            }
        }
    }

    if !stack.is_empty() {
        return None;
    }
    if tracks.is_empty() || shapes == 0 {
        return Some(CompileOutcome::StaticOnly);
    }
    let has_indefinite = tracks.iter().any(|t| !t.is_set && t.repeat.is_none());
    Some(CompileOutcome::Animated(CompiledAnim { view, ops, tracks, node_count: next_node_id as usize, has_indefinite }))
}

struct StackEntry<'a> {
    tag: &'a str,
    node_id: Option<u16>,
    pushed_group: bool,
    skip_subtree: bool,
    base_geom: Option<BaseGeom>,
    base_paint: Option<PaintProps>,
    base_opacity: f32,
    has_transform: bool,
}

fn parse_geom(tag: &str, attrs: &[(&str, &str)]) -> Option<BaseGeom> {
    let coord = |k: &str| find_attr(attrs, k).and_then(parse_dim).unwrap_or(0.0).clamp(-MAX_COORD, MAX_COORD);
    let nonneg = |k: &str| find_attr(attrs, k).and_then(parse_dim).unwrap_or(0.0).clamp(0.0, MAX_COORD);
    match tag {
        "circle" => Some(BaseGeom::Circle { cx: coord("cx"), cy: coord("cy"), r: nonneg("r") }),
        "ellipse" => Some(BaseGeom::Ellipse { cx: coord("cx"), cy: coord("cy"), rx: nonneg("rx"), ry: nonneg("ry") }),
        "rect" => Some(BaseGeom::Rect {
            x: coord("x"),
            y: coord("y"),
            w: nonneg("width"),
            h: nonneg("height"),
            rx: find_attr(attrs, "rx").and_then(parse_dim).map(|v| v.clamp(0.0, MAX_COORD)),
            ry: find_attr(attrs, "ry").and_then(parse_dim).map(|v| v.clamp(0.0, MAX_COORD)),
        }),
        "line" => Some(BaseGeom::Line { x1: coord("x1"), y1: coord("y1"), x2: coord("x2"), y2: coord("y2") }),
        "path" => {
            let d = find_attr(attrs, "d")?;
            if d.len() > MAX_PATH_BYTES {
                return None;
            }
            let mut cmds = 0usize;
            for b in d.bytes() {
                if matches!(b, b'M' | b'm' | b'L' | b'l' | b'H' | b'h' | b'V' | b'v' | b'C' | b'c' | b'S' | b's' | b'Q' | b'q' | b'T' | b't' | b'A' | b'a' | b'Z' | b'z') {
                    cmds += 1;
                    if cmds > MAX_PATH_COMMANDS {
                        return None;
                    }
                }
            }
            let parsed = convex::parse(d);
            if parsed.len() > MAX_PATH_COMMANDS * 4 {
                return None;
            }
            let pt_ok = |v: convex::V| v.x.is_finite() && v.y.is_finite() && v.x.abs() <= MAX_COORD && v.y.abs() <= MAX_COORD;
            for el in &parsed {
                let ok = match *el {
                    convex::El::Move(a) | convex::El::Line(a) => pt_ok(a),
                    convex::El::Quad(a, b) => pt_ok(a) && pt_ok(b),
                    convex::El::Cubic(a, b, c) => pt_ok(a) && pt_ok(b) && pt_ok(c),
                    convex::El::Conic(a, b, w) => pt_ok(a) && pt_ok(b) && w.is_finite() && w > 0.0 && w <= 1024.0,
                    convex::El::Close => true,
                };
                if !ok {
                    return None;
                }
            }
            Some(BaseGeom::Path(parsed))
        }
        "polyline" | "polygon" => {
            let raw = find_attr(attrs, "points")?;
            if raw.len() > MAX_PATH_BYTES {
                return None;
            }
            let nums = parse_bounded_numbers(raw, MAX_POINTS_NUMBERS)?;
            if nums.len() < 4 {
                return None;
            }
            let mut els = Vec::with_capacity(nums.len() / 2 + 1);
            for (i, &[x, y]) in nums.as_chunks::<2>().0.iter().enumerate() {
                let v = convex::V::new(x.clamp(-MAX_COORD, MAX_COORD), y.clamp(-MAX_COORD, MAX_COORD));
                els.push(if i == 0 { convex::El::Move(v) } else { convex::El::Line(v) });
            }
            if tag == "polygon" {
                els.push(convex::El::Close);
            }
            Some(BaseGeom::Path(els))
        }
        _ => None,
    }
}

fn base_attr_value(stack: &[StackEntry<'_>], attr: AttrKind) -> Option<f32> {
    let parent = stack.last()?;
    let inherited = |get: fn(PaintProps) -> Option<f32>, default: f32| {
        stack.iter().rev().filter_map(|e| e.base_paint).find_map(get).unwrap_or(default)
    };
    match attr {
        AttrKind::Opacity => Some(parent.base_opacity),
        AttrKind::FillOpacity => Some(inherited(|p| p.fill_opacity, 1.0)),
        AttrKind::StrokeOpacity => Some(inherited(|p| p.stroke_opacity, 1.0)),
        AttrKind::StrokeWidth => Some(inherited(|p| p.stroke_width, 1.0)),
        AttrKind::R => match parent.base_geom {
            Some(BaseGeom::Circle { r, .. }) => Some(r),
            _ => None,
        },
        AttrKind::Cx => match parent.base_geom {
            Some(BaseGeom::Circle { cx, .. }) | Some(BaseGeom::Ellipse { cx, .. }) => Some(cx),
            _ => None,
        },
        AttrKind::Cy => match parent.base_geom {
            Some(BaseGeom::Circle { cy, .. }) | Some(BaseGeom::Ellipse { cy, .. }) => Some(cy),
            _ => None,
        },
        AttrKind::Rx => match parent.base_geom {
            Some(BaseGeom::Ellipse { rx, .. }) => Some(rx),
            Some(BaseGeom::Rect { rx, .. }) => Some(rx.unwrap_or(0.0)),
            _ => None,
        },
        AttrKind::Ry => match parent.base_geom {
            Some(BaseGeom::Ellipse { ry, .. }) => Some(ry),
            Some(BaseGeom::Rect { rx, ry, .. }) => Some(ry.or(rx).unwrap_or(0.0)),
            _ => None,
        },
        AttrKind::X => match parent.base_geom {
            Some(BaseGeom::Rect { x, .. }) => Some(x),
            _ => None,
        },
        AttrKind::Y => match parent.base_geom {
            Some(BaseGeom::Rect { y, .. }) => Some(y),
            _ => None,
        },
        AttrKind::Width => match parent.base_geom {
            Some(BaseGeom::Rect { w, .. }) => Some(w),
            _ => None,
        },
        AttrKind::Height => match parent.base_geom {
            Some(BaseGeom::Rect { h, .. }) => Some(h),
            _ => None,
        },
        AttrKind::X1 => match parent.base_geom {
            Some(BaseGeom::Line { x1, .. }) => Some(x1),
            _ => None,
        },
        AttrKind::Y1 => match parent.base_geom {
            Some(BaseGeom::Line { y1, .. }) => Some(y1),
            _ => None,
        },
        AttrKind::X2 => match parent.base_geom {
            Some(BaseGeom::Line { x2, .. }) => Some(x2),
            _ => None,
        },
        AttrKind::Y2 => match parent.base_geom {
            Some(BaseGeom::Line { y2, .. }) => Some(y2),
            _ => None,
        },
        AttrKind::Transform(_) => None,
    }
}

fn parse_track(tag: &str, attrs: &[(&str, &str)], target: u16, stack: &[StackEntry<'_>]) -> Option<Track> {
    let parent = stack.last()?;
    let attr_name = find_attr(attrs, "attributeName")?.trim();
    let is_set = tag == "set";
    let attr = if tag == "animateTransform" {
        if attr_name != "transform" {
            return None;
        }
        let kind = match find_attr(attrs, "type").map(str::trim).unwrap_or("translate") {
            "translate" => TransformKind::Translate,
            "scale" => TransformKind::Scale,
            "rotate" => TransformKind::Rotate,
            "skewX" => TransformKind::SkewX,
            "skewY" => TransformKind::SkewY,
            _ => return None,
        };
        AttrKind::Transform(kind)
    } else {
        match attr_name {
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
        }
    };

    // `begin`: absent defaults to 0.0; if present, MUST parse as a finite clock value (rejects
    // event-bases like "click" or "indefinite" rather than silently auto-playing at 0).
    let begin = match find_attr(attrs, "begin") {
        None => 0.0,
        Some(raw) => {
            let b = parse_clock(raw)?;
            if !(-MAX_CLOCK_SECS..=MAX_CLOCK_SECS).contains(&b) {
                return None;
            }
            b
        }
    };

    // `dur`: required finite `[MIN_DUR_SECS, MAX_CLOCK_SECS]` for `<animate>`/`<animateTransform>`.
    // For `<set>`, omitted or `"indefinite"` means a single step at `begin` (`dur = None`, never
    // spinning); a specified finite `dur` must also lie in `[MIN_DUR_SECS, MAX_CLOCK_SECS]`.
    let dur = if is_set {
        match find_attr(attrs, "dur").map(str::trim) {
            None | Some("indefinite") => None,
            Some(raw) => {
                let d = parse_clock(raw)?;
                if !(MIN_DUR_SECS..=MAX_CLOCK_SECS).contains(&d) {
                    return None;
                }
                Some(d)
            }
        }
    } else {
        let d = parse_clock(find_attr(attrs, "dur")?)?;
        if !(MIN_DUR_SECS..=MAX_CLOCK_SECS).contains(&d) {
            return None;
        }
        Some(d)
    };

    let repeat = if is_set {
        Some(1.0)
    } else {
        match find_attr(attrs, "repeatCount").map(str::trim) {
            None => Some(1.0),
            Some("indefinite") => None,
            Some(raw) => {
                let n = parse_finite(raw)?;
                if !(0.0..=MAX_REPEAT_COUNT).contains(&n) || n == 0.0 {
                    return None;
                }
                Some(n)
            }
        }
    };

    let freeze = find_attr(attrs, "fill").map(str::trim) == Some("freeze");
    let additive_sum = !is_set && find_attr(attrs, "additive").map(str::trim) == Some("sum");

    let mut values = Vec::new();
    let mut extra_additive = false;
    if is_set {
        let v = parse_key_val(find_attr(attrs, "to")?, attr, false)?;
        values.push(v);
    } else if let Some(raw_vals) = find_attr(attrs, "values") {
        let mut raw_parts = 0usize;
        for part in raw_vals.split(';') {
            raw_parts += 1;
            if raw_parts > MAX_KEYFRAMES + 1 {
                return None;
            }
            let t = part.trim();
            if t.is_empty() {
                continue;
            }
            if values.len() >= MAX_KEYFRAMES {
                return None;
            }
            values.push(parse_key_val(t, attr, additive_sum)?);
        }
    } else {
        let from_raw = find_attr(attrs, "from");
        let to_raw = find_attr(attrs, "to");
        let by_raw = find_attr(attrs, "by");
        match (from_raw, to_raw, by_raw) {
            (Some(f), Some(t), _) => {
                values.push(parse_key_val(f, attr, additive_sum)?);
                values.push(parse_key_val(t, attr, additive_sum)?);
            }
            (Some(f), None, Some(b)) => {
                let fv = parse_key_val(f, attr, additive_sum)?;
                let bv = parse_key_val(b, attr, true)?;
                let len = fv.len.max(bv.len);
                let tv = KeyVal {
                    v: [fv.v[0] + bv.v[0], fv.v[1] + bv.v[1], fv.v[2] + bv.v[2]],
                    len,
                };
                values.push(fv);
                values.push(clamp_key_val(tv, attr, additive_sum));
            }
            (None, Some(t), _) => {
                let base = default_base_key_val(stack, attr)?;
                values.push(base);
                values.push(parse_key_val(t, attr, false)?);
                // On a transform with an existing static base transform, `to`-only interpolates
                // relative to the underlying transform rather than discarding it.
                if matches!(attr, AttrKind::Transform(_)) && parent.has_transform {
                    extra_additive = true;
                }
            }
            (None, None, Some(b)) => {
                // SMIL `by`-only animation is implicitly additive (`additive="sum"`):
                // for `scale`, identity is `(1, 1)` so `by` interpolates `(1, 1) -> (1 + bx, 1 + by)`;
                // for all other attributes, identity is `0` so `by` interpolates `0 -> by` (signed!).
                let bv = parse_key_val(b, attr, true)?;
                if matches!(attr, AttrKind::Transform(TransformKind::Scale)) {
                    values.push(KeyVal { v: [1.0, 1.0, 0.0], len: 2 });
                    values.push(clamp_key_val(KeyVal { v: [1.0 + bv.v[0], 1.0 + bv.v[1], 0.0], len: 2 }, attr, false));
                } else {
                    values.push(KeyVal { v: [0.0; 3], len: bv.len });
                    values.push(bv);
                }
                extra_additive = true;
            }
            _ => return None,
        }
    }

    if values.is_empty() || (!is_set && values.len() < 2) {
        return None;
    }

    let calc_mode = if is_set {
        CalcMode::Discrete
    } else {
        match find_attr(attrs, "calcMode").map(str::trim) {
            None | Some("linear") => CalcMode::Linear,
            Some("discrete") => CalcMode::Discrete,
            Some("spline") => CalcMode::Spline,
            Some("paced") => CalcMode::Paced,
            _ => return None,
        }
    };

    let (key_times, key_splines) = if is_set || values.len() < 2 {
        (Vec::new(), Vec::new())
    } else if calc_mode == CalcMode::Paced {
        // SMIL 3.0: `calcMode="paced"` ignores `keyTimes`/`keySplines` and paces at constant
        // velocity by cumulative Euclidean distance between keyframes.
        (paced_key_times(&values), Vec::new())
    } else {
        let kt = match find_attr(attrs, "keyTimes") {
            Some(raw) => parse_key_times(raw, values.len(), calc_mode)?,
            None => uniform_key_times(values.len(), calc_mode),
        };
        let ks = if calc_mode == CalcMode::Spline {
            match find_attr(attrs, "keySplines") {
                Some(raw) => parse_key_splines(raw, values.len() - 1)?,
                None => vec![[0.0, 0.0, 1.0, 1.0]; values.len() - 1],
            }
        } else {
            Vec::new()
        };
        (kt, ks)
    };

    Some(Track {
        target,
        attr,
        begin,
        dur,
        repeat,
        freeze,
        additive_sum: additive_sum || extra_additive,
        is_set,
        calc_mode,
        values,
        key_times,
        key_splines,
    })
}

fn default_base_key_val(stack: &[StackEntry<'_>], attr: AttrKind) -> Option<KeyVal> {
    match attr {
        AttrKind::Transform(TransformKind::Scale) => Some(KeyVal { v: [1.0, 1.0, 0.0], len: 2 }),
        AttrKind::Transform(TransformKind::Translate) => Some(KeyVal { v: [0.0, 0.0, 0.0], len: 2 }),
        AttrKind::Transform(TransformKind::Rotate) => Some(KeyVal { v: [0.0, 0.0, 0.0], len: 3 }),
        AttrKind::Transform(TransformKind::SkewX | TransformKind::SkewY) => Some(KeyVal::scalar(0.0)),
        _ => Some(KeyVal::scalar(base_attr_value(stack, attr)?)),
    }
}

fn uniform_key_times(count: usize, mode: CalcMode) -> Vec<f32> {
    if mode == CalcMode::Discrete {
        (0..count).map(|i| i as f32 / count as f32).collect()
    } else {
        let denom = (count - 1).max(1) as f32;
        (0..count).map(|i| i as f32 / denom).collect()
    }
}

fn paced_key_times(values: &[KeyVal]) -> Vec<f32> {
    let mut dists = Vec::with_capacity(values.len());
    dists.push(0.0f32);
    let mut total = 0.0f32;
    for pair in values.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let len = a.len.max(b.len) as usize;
        let sq: f32 = (0..len).map(|i| { let d = b.v[i] - a.v[i]; d * d }).sum();
        total += sq.sqrt();
        dists.push(total);
    }
    if total <= 1e-6 || !total.is_finite() {
        return uniform_key_times(values.len(), CalcMode::Linear);
    }
    dists.iter().map(|&d| (d / total).clamp(0.0, 1.0)).collect()
}

fn parse_key_val(raw: &str, attr: AttrKind, is_delta: bool) -> Option<KeyVal> {
    let nums = parse_small_numbers::<3>(raw)?;
    if nums.is_empty() {
        return None;
    }
    let mut v = [0.0f32; 3];
    for (i, &n) in nums.iter().enumerate() {
        v[i] = n;
    }
    let mut len = nums.len() as u8;
    // Normalize 1-arg `scale(s)` to `scale(s, s)` at compile time so mixed 1-arg / 2-arg scale
    // animations interpolate `sy` smoothly instead of interpolating from 0.
    if matches!(attr, AttrKind::Transform(TransformKind::Scale)) && len == 1 {
        v[1] = v[0];
        len = 2;
    }
    Some(clamp_key_val(KeyVal { v, len }, attr, is_delta))
}

fn clamp_key_val(mut kv: KeyVal, attr: AttrKind, is_delta: bool) -> KeyVal {
    match attr {
        AttrKind::Opacity | AttrKind::FillOpacity | AttrKind::StrokeOpacity => {
            kv.v[0] = if is_delta { kv.v[0].clamp(-1.0, 1.0) } else { kv.v[0].clamp(0.0, 1.0) };
        }
        AttrKind::StrokeWidth => {
            kv.v[0] = if is_delta { kv.v[0].clamp(-MAX_STROKE_WIDTH, MAX_STROKE_WIDTH) } else { kv.v[0].clamp(0.0, MAX_STROKE_WIDTH) };
        }
        AttrKind::R | AttrKind::Rx | AttrKind::Ry | AttrKind::Width | AttrKind::Height => {
            kv.v[0] = if is_delta { kv.v[0].clamp(-MAX_COORD, MAX_COORD) } else { kv.v[0].clamp(0.0, MAX_COORD) };
        }
        AttrKind::Cx | AttrKind::Cy | AttrKind::X | AttrKind::Y | AttrKind::X1 | AttrKind::Y1 | AttrKind::X2 | AttrKind::Y2 => {
            kv.v[0] = kv.v[0].clamp(-MAX_COORD, MAX_COORD);
        }
        AttrKind::Transform(TransformKind::Scale) => {
            kv.v[0] = kv.v[0].clamp(-MAX_SCALE, MAX_SCALE);
            kv.v[1] = kv.v[1].clamp(-MAX_SCALE, MAX_SCALE);
        }
        AttrKind::Transform(TransformKind::Translate) => {
            kv.v[0] = kv.v[0].clamp(-MAX_COORD, MAX_COORD);
            kv.v[1] = kv.v[1].clamp(-MAX_COORD, MAX_COORD);
        }
        AttrKind::Transform(TransformKind::Rotate) => {
            kv.v[0] = kv.v[0].clamp(-36_000.0, 36_000.0);
            kv.v[1] = kv.v[1].clamp(-MAX_COORD, MAX_COORD);
            kv.v[2] = kv.v[2].clamp(-MAX_COORD, MAX_COORD);
        }
        AttrKind::Transform(TransformKind::SkewX | TransformKind::SkewY) => {
            kv.v[0] = kv.v[0].clamp(-MAX_SKEW_DEG, MAX_SKEW_DEG);
        }
    }
    kv
}

fn parse_key_times(raw: &str, count: usize, mode: CalcMode) -> Option<Vec<f32>> {
    let mut parsed = Vec::with_capacity(count);
    let mut raw_parts = 0usize;
    for part in raw.split(';') {
        raw_parts += 1;
        if raw_parts > count + 1 {
            return None;
        }
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        if parsed.len() >= count {
            return None;
        }
        let v = parse_finite(t)?;
        if !(0.0..=1.0).contains(&v) {
            return None;
        }
        if let Some(&prev) = parsed.last()
            && v < prev
        {
            return None;
        }
        parsed.push(v);
    }
    if parsed.len() != count || parsed[0] != 0.0 {
        return None;
    }
    if mode != CalcMode::Discrete && (parsed[count - 1] - 1.0).abs() > 1e-4 {
        return None;
    }
    Some(parsed)
}

fn parse_key_splines(raw: &str, segments: usize) -> Option<Vec<[f32; 4]>> {
    let mut out = Vec::with_capacity(segments);
    let mut raw_parts = 0usize;
    for part in raw.split(';') {
        raw_parts += 1;
        if raw_parts > segments + 1 {
            return None;
        }
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        if out.len() >= segments {
            return None;
        }
        let nums = parse_small_numbers::<4>(t)?;
        if nums.len() != 4 || !(0.0..=1.0).contains(&nums[0]) || !(0.0..=1.0).contains(&nums[2]) {
            return None;
        }
        out.push([
            nums[0],
            nums[1].clamp(MIN_SPLINE_Y, MAX_SPLINE_Y),
            nums[2],
            nums[3].clamp(MIN_SPLINE_Y, MAX_SPLINE_Y),
        ]);
    }
    if out.len() != segments {
        return None;
    }
    Some(out)
}

/// Parses SMIL clock values: `hh:mm:ss(.fff)`, `mm:ss(.fff)`, `h`, `min`, `s`, `ms`, or bare
/// seconds, with optional leading `+` or `-`.
fn parse_clock(raw: &str) -> Option<f32> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let (sign, body) = if let Some(rest) = t.strip_prefix('-') {
        (-1.0f32, rest.trim())
    } else if let Some(rest) = t.strip_prefix('+') {
        (1.0f32, rest.trim())
    } else {
        (1.0f32, t)
    };
    if body.starts_with(['+', '-']) {
        return None;
    }
    let mag = if body.contains(':') {
        let mut parts = body.split(':');
        let a = parts.next()?;
        let b = parts.next()?;
        let c = parts.next();
        if parts.next().is_some() {
            return None;
        }
        match c {
            Some(sec) => {
                let h = parse_finite(a)?;
                let m = parse_finite(b)?;
                let s = parse_finite(sec)?;
                if h < 0.0 || !(0.0..60.0).contains(&m) || !(0.0..60.0).contains(&s) {
                    return None;
                }
                h * 3600.0 + m * 60.0 + s
            }
            None => {
                let m = parse_finite(a)?;
                let s = parse_finite(b)?;
                if m < 0.0 || !(0.0..60.0).contains(&s) {
                    return None;
                }
                m * 60.0 + s
            }
        }
    } else if let Some(v) = body.strip_suffix("ms") {
        parse_finite(v)? / 1000.0
    } else if let Some(v) = body.strip_suffix("min") {
        parse_finite(v)? * 60.0
    } else if let Some(v) = body.strip_suffix('h') {
        parse_finite(v)? * 3600.0
    } else if let Some(v) = body.strip_suffix('s') {
        parse_finite(v)?
    } else {
        parse_finite(body)?
    };
    let val = sign * mag;
    val.is_finite().then_some(val)
}

fn parse_transform_bounded(raw: &str) -> Option<Transform> {
    let mut m = Transform::IDENTITY;
    let mut ops = 0usize;
    let mut rest = raw.trim();
    while !rest.is_empty() {
        ops += 1;
        if ops > MAX_TRANSFORM_OPS {
            return None;
        }
        let (name, after) = rest.split_once('(')?;
        let (args, tail) = after.split_once(')')?;
        let op = name.trim().trim_start_matches(',').trim();
        let nums = parse_small_numbers::<6>(args)?;
        let t = match op {
            "matrix" if nums.len() == 6 => Transform::new(
                nums[0].clamp(-MAX_SCALE, MAX_SCALE),
                nums[1].clamp(-MAX_SCALE, MAX_SCALE),
                nums[2].clamp(-MAX_SCALE, MAX_SCALE),
                nums[3].clamp(-MAX_SCALE, MAX_SCALE),
                nums[4].clamp(-MAX_COORD, MAX_COORD),
                nums[5].clamp(-MAX_COORD, MAX_COORD),
            ),
            "translate" if matches!(nums.len(), 1 | 2) => Transform::translation(
                nums[0].clamp(-MAX_COORD, MAX_COORD),
                nums.get(1).copied().unwrap_or(0.0).clamp(-MAX_COORD, MAX_COORD),
            ),
            "scale" if matches!(nums.len(), 1 | 2) => {
                let sx = nums[0].clamp(-MAX_SCALE, MAX_SCALE);
                let sy = nums.get(1).copied().unwrap_or(sx).clamp(-MAX_SCALE, MAX_SCALE);
                Transform::scale(sx, sy)
            }
            "rotate" if matches!(nums.len(), 1 | 3) => {
                let deg = nums[0].clamp(-36_000.0, 36_000.0);
                let cx = nums.get(1).copied().unwrap_or(0.0).clamp(-MAX_COORD, MAX_COORD);
                let cy = nums.get(2).copied().unwrap_or(0.0).clamp(-MAX_COORD, MAX_COORD);
                rotate_transform(deg, cx, cy)
            }
            "skewX" if nums.len() == 1 => {
                Transform::new(1.0, 0.0, nums[0].clamp(-MAX_SKEW_DEG, MAX_SKEW_DEG).to_radians().tan(), 1.0, 0.0, 0.0)
            }
            "skewY" if nums.len() == 1 => {
                Transform::new(1.0, nums[0].clamp(-MAX_SKEW_DEG, MAX_SKEW_DEG).to_radians().tan(), 0.0, 1.0, 0.0, 0.0)
            }
            _ => return None,
        };
        m = super::super::then(t, m);
        if !super::local_ctm_is_safe(m) {
            return None;
        }
        rest = tail.trim().trim_start_matches(',').trim();
    }
    Some(m)
}

pub(super) fn rotate_transform(deg: f32, cx: f32, cy: f32) -> Transform {
    let (sin, cos) = (deg as f64).to_radians().sin_cos();
    let (cx, cy) = (cx as f64, cy as f64);
    let (tx, ty) = (cx - cos * cx + sin * cy, cy - sin * cx - cos * cy);
    Transform::new(cos as f32, sin as f32, -sin as f32, cos as f32, tx as f32, ty as f32)
}

/// Parses up to `N` finite floats directly onto the stack — zero heap allocation before caps.
fn parse_small_numbers<const N: usize>(s: &str) -> Option<SmallVec<N>> {
    let mut out = SmallVec::<N>::new();
    let mut start: Option<usize> = None;
    let bytes = s.as_bytes();
    let mut prev = b' ';
    for (i, &b) in bytes.iter().enumerate() {
        let sign_split = (b == b'-' || b == b'+') && !matches!(prev, b'e' | b'E');
        let sep = b == b',' || b.is_ascii_whitespace();
        if (sep || (sign_split && start.is_some())) && let Some(st) = start.take() {
            if out.len == N {
                return None;
            }
            out.push(parse_finite(&s[st..i])?);
        }
        if !sep && start.is_none() {
            start = Some(i);
        }
        prev = b;
    }
    if let Some(st) = start {
        if out.len == N {
            return None;
        }
        out.push(parse_finite(&s[st..])?);
    }
    Some(out)
}

struct SmallVec<const N: usize> {
    data: [f32; N],
    len: usize,
}

impl<const N: usize> SmallVec<N> {
    fn new() -> Self {
        Self { data: [0.0; N], len: 0 }
    }
    fn push(&mut self, v: f32) {
        self.data[self.len] = v;
        self.len += 1;
    }
    fn as_slice(&self) -> &[f32] {
        &self.data[..self.len]
    }
}

impl<const N: usize> std::ops::Deref for SmallVec<N> {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        self.as_slice()
    }
}

fn parse_bounded_numbers(s: &str, max: usize) -> Option<Vec<f32>> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let bytes = s.as_bytes();
    let mut prev = b' ';
    for (i, &b) in bytes.iter().enumerate() {
        let sign_split = (b == b'-' || b == b'+') && !matches!(prev, b'e' | b'E');
        let sep = b == b',' || b.is_ascii_whitespace();
        if (sep || (sign_split && start.is_some())) && let Some(st) = start.take() {
            if out.len() >= max {
                return None;
            }
            out.push(parse_finite(&s[st..i])?);
        }
        if !sep && start.is_none() {
            start = Some(i);
        }
        prev = b;
    }
    if let Some(st) = start {
        if out.len() >= max {
            return None;
        }
        out.push(parse_finite(&s[st..])?);
    }
    Some(out)
}

fn parse_finite(s: &str) -> Option<f32> {
    let v = s.trim().parse::<f32>().ok()?;
    v.is_finite().then_some(v)
}

fn parse_dim(s: &str) -> Option<f32> {
    parse_finite(s.trim().trim_end_matches("px"))
}

fn parse_opacity_opt(s: &str) -> Option<f32> {
    let t = s.trim();
    let v = match t.strip_suffix('%') {
        Some(p) => parse_finite(p)? / 100.0,
        None => parse_finite(t)?,
    };
    Some(v.clamp(0.0, 1.0))
}

fn find_attr<'a>(attrs: &[(&'a str, &'a str)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|&&(k, _)| k == name).map(|&(_, v)| v)
}

enum Tok<'a> {
    Open { tag: &'a str, attrs: Vec<(&'a str, &'a str)>, self_closing: bool },
    Close(&'a str),
}

/// Iterative XML tag scanner supporting XML comments (including `>` inside comments), both single
/// and double quoted attribute values (including `>` inside attributes), and rejecting DTD/CDATA.
struct XmlScanner<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> XmlScanner<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn next_token(&mut self) -> Option<Option<Tok<'a>>> {
        let bytes = self.src.as_bytes();
        loop {
            let Some(rel) = self.src[self.pos..].find('<') else {
                return Some(None);
            };
            self.pos += rel;
            let rem = &self.src[self.pos..];
            if let Some(after) = rem.strip_prefix("<!--") {
                let end = after.find("-->")?;
                self.pos += 4 + end + 3;
                continue;
            }
            if rem.starts_with("<!") {
                // Reject DTD / CDATA / entity declarations.
                return None;
            }
            if let Some(after) = rem.strip_prefix("<?") {
                let end = after.find("?>")?;
                self.pos += 2 + end + 2;
                continue;
            }
            if rem.starts_with("</") {
                let close = rem.find('>')?;
                let tag = rem[2..close].trim();
                if tag.is_empty() {
                    return None;
                }
                self.pos += close + 1;
                return Some(Some(Tok::Close(tag)));
            }
            let mut i = self.pos + 1;
            let tag_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
                i += 1;
            }
            if i == tag_start {
                return None;
            }
            let tag = &self.src[tag_start..i];
            let mut attrs = Vec::new();
            loop {
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i >= bytes.len() {
                    return None;
                }
                if bytes[i] == b'>' {
                    self.pos = i + 1;
                    return Some(Some(Tok::Open { tag, attrs, self_closing: false }));
                }
                if bytes[i] == b'/' {
                    if bytes.get(i + 1) != Some(&b'>') {
                        return None;
                    }
                    self.pos = i + 2;
                    return Some(Some(Tok::Open { tag, attrs, self_closing: true }));
                }
                let attr_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' && bytes[i] != b'>' && bytes[i] != b'/' {
                    i += 1;
                }
                let attr_name = &self.src[attr_start..i];
                if attr_name.is_empty() {
                    return None;
                }
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if bytes.get(i) != Some(&b'=') {
                    return None;
                }
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                let &quote = bytes.get(i).filter(|&&b| b == b'"' || b == b'\'')?;
                i += 1;
                let val_start = i;
                while i < bytes.len() && bytes[i] != quote {
                    i += 1;
                }
                if i >= bytes.len() {
                    return None;
                }
                let attr_val = &self.src[val_start..i];
                i += 1;
                if attrs.len() >= MAX_ATTRS_PER_TAG {
                    return None;
                }
                attrs.push((attr_name, attr_val));
            }
        }
    }
}
