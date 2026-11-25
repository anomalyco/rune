use std::borrow::Cow;
use crate::el::ElFlags;

use super::*;

/// An `aspect_fit` element's first layout: it takes the free space along its parent's main axis
/// and the whole cross side, within its max sizes, as the room its fitted box must fit. Its
/// content never sizes the room: with no max along the main axis it starts from 0 and grows.
pub(in crate::runtime) fn fit_room(own: &CompactStyle, parent_row: bool) -> CompactStyle {
    let raw = own.to_style();
    let zero = |d: LengthPercentageAuto| if d.is_auto() { length(0.0) } else { d };
    let main_max = if parent_row { raw.max_size.width } else { raw.max_size.height };
    let size = if parent_row {
        Size { width: main_max.into(), height: auto() }
    } else {
        Size { width: auto(), height: main_max.into() }
    };
    CompactStyle::from_style(Style {
        size,
        min_size: Size { width: zero(raw.min_size.width), height: zero(raw.min_size.height) },
        flex_grow: 1.0,
        flex_shrink: 1.0,
        flex_basis: if main_max.is_auto() { length(0.0) } else { auto() },
        align_self: Some(AlignSelf::STRETCH),
        aspect_ratio: None,
        ..raw
    })
}

/// The largest box at `ratio` inside `room`, as the fixed size an `aspect_fit` element lays
/// out at once its room is known.
pub(in crate::runtime) fn fitted(ratio: f32, own: &CompactStyle, room: Size<f32>) -> CompactStyle {
    let w = room.width.min(room.height * ratio).max(0.0);
    CompactStyle::from_style(Style {
        size: Size { width: length(w), height: length(w / ratio) },
        flex_grow: 0.0,
        flex_shrink: 0.0,
        flex_basis: auto(),
        aspect_ratio: None,
        ..own.to_style()
    })
}

/// Width over height of the image in `pixels`, or 0 while it's empty.
pub(in crate::runtime) fn pixel_ratio(pixels: &crate::Pixels) -> f32 {
    let (w, h) = pixels.size();
    if w > 0 && h > 0 { w as f32 / h as f32 } else { 0.0 }
}

/// How a picture that takes one side from its image's ratio gets the other. Taffy carries an
/// aspect ratio over from the preferred size before min and max clamp it (`w_full().max_w(176.0)`
/// at 4.4:1 in a 696-point column came out 176 wide yet reserved 158 tall), so a side in points
/// is clamped here first. A percentage under a min or max can't be: that picture measures
/// instead (`Picture::Measured`), its other side from the side it gets, as a browser sizes an
/// image.
fn picture_side(style: &mut CompactStyle) -> Picture {
    use taffy::util::MaybeResolve;
    let size = style.size();
    let tall = size.height.is_auto();
    let (min, max) = (style.min_size(), style.max_size());
    let (set, min, max) = if tall { (size.width, min.width, max.width) } else { (size.height, min.height, max.height) };
    if min.is_auto() && max.is_auto() { return Picture::Styled; }
    let points = |l: LengthPercentageAuto| l.maybe_resolve(None, |_, _| 0.0);
    let limits_in_points = (min.is_auto() || points(min).is_some()) && (max.is_auto() || points(max).is_some());
    let Some(v) = set.maybe_resolve(None, |_, _| 0.0).filter(|_| limits_in_points) else { return Picture::Measured };
    let v = points(max).map_or(v, |m| v.min(m));
    let v = points(min).map_or(v, |m| v.max(m));
    let side = taffy::Dimension::length(v);
    style.set_size(if tall { Size { width: side, height: size.height } } else { Size { width: size.width, height: side } });
    Picture::Styled
}

/// How a picture's ratio reaches layout.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Picture {
    /// As the style's aspect ratio.
    Styled,
    /// As its leaf's measure (`text_layout::compute_layout`), keyed by the ratio.
    Measured,
}

/// A measured picture's measure key: its ratio, so a new one measures again.
fn picture_key(ratio: f32) -> u64 { 0x7069_6300_0000_0000 | u64::from(ratio.to_bits()) }

pub(crate) use super::path::in_scope;
pub(in crate::runtime) use super::path::node_path;
use super::path::push_segment;
#[cfg(test)]
use super::path::push_usize;

#[inline]
fn can_take_old_path(old: &Node, path_buf: &str) -> bool {
    !path_buf.is_empty()
        && !old.is_moved_stub()
        && old.path == path_buf
        && old.input.is_none()
        && old.focus.get().is_none_or(|f| f.return_to.is_none())
        && old.handlers.get().is_none_or(|h| h.on_focus_change.is_none() && h.on_focus_within_change.is_none())
}

#[inline]
fn resolve_path_and_id(
    path_buf: &str,
    new_idx: usize,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> (String, NodeId) {
    let old_limit = memo_ctx.old_present_len.min(memo_ctx.old_nodes.len());
    if memo_ctx.committing && memo_ctx.old_cursor < old_limit {
        let oi = memo_ctx.old_cursor;
        if can_take_old_path(&memo_ctx.old_nodes[oi], path_buf) {
            let old = &mut memo_ctx.old_nodes[oi];
            let id = old.id;
            let path = std::mem::take(&mut old.path);
            memo_ctx.taken_paths.push((oi as u32, new_idx as u32));
            memo_ctx.old_cursor = oi + 1;
            return (path, id);
        }
    }
    if let Some(&id) = memo_ctx.path_ids.get(path_buf) {
        if memo_ctx.committing
            && let Some(&oi) = memo_ctx.old_node_index.get(&id)
            && oi < old_limit
            && memo_ctx.old_nodes[oi].id == id
            && can_take_old_path(&memo_ctx.old_nodes[oi], path_buf)
        {
            let old = &mut memo_ctx.old_nodes[oi];
            let path = std::mem::take(&mut old.path);
            memo_ctx.taken_paths.push((oi as u32, new_idx as u32));
            memo_ctx.old_cursor = oi + 1;
            return (path, id);
        }
        return (path_buf.to_owned(), id);
    }
    let path = path_buf.to_owned();
    let id = NodeId(*memo_ctx.next_id);
    *memo_ctx.next_id += 1;
    memo_ctx.path_ids.insert(path.clone(), id);
    (path, id)
}

#[derive(Clone, Copy)]
enum ScrollXChild {
    Single { vertical: bool },
    Many { vertical: bool },
}

fn apply_scroll_x_child(style: &mut CompactStyle, sx: ScrollXChild) {
    match sx {
        ScrollXChild::Single { vertical } => unwrapped_width(style, vertical),
        ScrollXChild::Many { vertical } => {
            let w = style.size().width;
            let w_full = w == Dimension::percent(1.0);
            if w.is_auto() || w_full {
                style.set_width(Dimension::max_content());
                if w_full && style.min_size().width.is_auto() {
                    style.set_min_width(percent(1.0));
                }
            }
            style.set_flex_shrink(0.0);
            if vertical && style.align_self().is_none() {
                style.set_align_self(Some(AlignSelf::FLEX_START));
            }
        }
    }
}

fn prepare_text(
    text: &mut Option<crate::cold::SharedBox<TextSpec>>,
    input: Option<&InputSpec>,
    scale: f32,
    flow: Option<crate::el::Flow>,
) -> (Option<u64>, bool) {
    if let Some(t) = text
        && let dir = crate::el::step_text_dir(t.dir, crate::el::TextDirEvent::Flatten(flow))
        // A field Ctrl+Shift set reads that way, every line of it.
        && let dir = input.and_then(|i| i.edit.forced_dir()).map_or(dir, crate::el::TextDir::Fixed)
        && t.dir != dir
    {
        t.dir = dir;
        t.shape = None;
    }
    if let Some(t) = text && t.redacted {
        // Rich links carry URLs and can expose them through hover, copy and tree children.
        let t: &mut TextSpec = t;
        t.selectable = false;
        if let Some(cold) = t.cold.get_mut_if_some() {
            crate::el::wipe_links(&mut cold.links);
            cold.rings.clear();
            cold.overlines.clear();
            cold.span_boxes.clear();
            cold.inline_boxes.clear();
            cold.inline_anchors.clear();
            cold.line_fills.clear();
            cold.text_indent = 0.0;
            cold.advance_in = None;
            cold.live = None;
        }
    }
    if input.is_some_and(|i| i.edit.is_hidden_multiline()) && let Some(t) = text {
        // The fixed mask stays one line even in a field narrower than twelve bullets.
        if t.wrap { t.wrap = false; }
    } else if input.is_none()
        && let Some(t) = text
        && t.redacted
    {
        t.case = None;
        if !t.cold.secure_revealed {
            t.conceal();
        }
    }
    if let Some(t) = text && t.cold.live.is_some() {
        let incompatible = input.is_some()
            || t.redacted
            || !t.spans.is_empty()
            || t.reveal >= 0.0
            || t.cold.advance_in.is_some()
            || !t.cold.links.is_empty()
            || !t.cold.pills.is_empty()
            || !t.cold.isolates.is_empty()
            || !t.cold.rings.is_empty()
            || !t.cold.strikes.is_empty()
            || !t.cold.overlines.is_empty()
            || !t.cold.span_boxes.is_empty()
            || !t.cold.highlights.is_empty()
            || !t.cold.line_fills.is_empty()
            || !t.cold.blocks.is_empty()
            || !t.cold.soft_breaks.is_empty()
            || !t.cold.uncopied.is_empty();
        if incompatible {
            t.cold.live = None;
        }
    }
    if let Some(pt) = text.as_deref().and_then(TextSpec::positioned) {
        let secure_measure = text.as_ref().is_some_and(|t| t.redacted);
        let vb = pt.view_box_size().map_or(0u64, |[w, h]| {
            u64::from(w.to_bits()).wrapping_mul(0x517c_c1b7_2722_0a95) ^ u64::from(h.to_bits()).rotate_left(17)
        });
        let key = pt.id().wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ vb ^ 0x706f_7369_7469_6f6e;
        return (Some(if secure_measure { 0 } else { key }), secure_measure);
    }
    if input.is_none()
        && text.as_ref().is_some_and(|t| !t.cold.isolates.is_empty() || !t.cold.pills.is_empty())
        && let Some(t) = text
    {
        t.apply_isolates();
    }
    if input.is_none()
        && text.as_ref().is_some_and(|t| t.redacted || t.case.is_some() || !t.cold.rings.is_empty())
        && let Some(t) = text
    {
        t.apply_case();
    }
    if text.as_ref().is_some_and(|t| t.redacted || t.shape.is_none_or(|s| s.0 != scale.to_bits()))
        && let Some(t) = text
    {
        t.key_shape(scale);
    }
    let measure = text.as_ref().map(|t| t.shape.map_or(0, |s| s.1) ^ u64::from(t.wrap) ^ (u64::from(t.truncate || t.truncate_fade) << 1)
        ^ ((t.max_lines as u64) << 2)
        ^ (u64::from(input.is_some_and(|i| i.edit.is_revealed()) || t.cold.secure_revealed) << 3)
        ^ (u64::from(t.wrap_anywhere) << 4) ^ (u64::from(t.greedy()) << 5) ^ (u64::from(t.by_line) << 6)
        ^ ((t.elide as u64) << 7)
        ^ if t.cold.text_indent == 0.0 { 0 } else { u64::from(t.cold.text_indent.to_bits()).wrapping_mul(0x517c_c1b7_2722_0a95) }
        ^ t.cold.advance_in.as_ref().map_or(0, |(run, at)| run.hash.rotate_left(3) ^ (*at as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)));
    // Secure leaves remeasure without retaining a content-derived dictionary oracle.
    let secure_measure = text.as_ref().is_some_and(|t| t.redacted);
    let measure = if secure_measure { Some(0) } else { measure };
    (measure, secure_measure)
}

/// Text, an icon, and a text field's placeholder, caret and selection that weren't given a
/// colour take the window's foreground ([`Cx::foreground`]), as CSS text takes `color`.
/// A text field with its own [`El::color`] already derives its uninked defaults from that colour.
fn take_foreground(
    text: &mut Option<crate::cold::SharedBox<TextSpec>>,
    input: Option<&mut InputSpec>,
    vis: &mut Visual,
    foreground: Option<Color>,
) {
    let Some(fg) = foreground else { return };
    let text_inked = text.as_ref().is_some_and(|t| t.inked);
    if let Some(t) = text && !t.inked && t.color != fg {
        t.color = fg;
    }
    if let Some(i) = input && !text_inked {
        if !i.placeholder_inked {
            i.placeholder_color = fg.alpha(0.4);
        }
        if !i.caret_inked {
            i.caret = fg;
        }
        if !i.selection_inked {
            i.selection = fg.alpha(0.2);
        }
    }
    if !vis.icon_inked && vis.icon.is_some_and(|(_, c)| c != fg) && let Some((_, c)) = &mut vis.cold_mut().icon {
        *c = fg;
    }
}

/// The `El::hover_motion_within` pair a node goes by: its own, else its anchor's, else its
/// parent's.
#[inline(always)]
fn inherit_hover_within(
    vis: &Visual,
    above: Option<&crate::el::Anchor>,
    parent: Option<usize>,
    nodes: &[Node],
) -> Option<Rc<(Anim, Anim)>> {
    if vis.hover_within.is_none() && above.is_none() && parent.is_none_or(|p| nodes[p].hover_within.is_none()) {
        return None;
    }
    inherit_hover_within_slow(vis, above, parent, nodes)
}

#[inline(never)]
fn inherit_hover_within_slow(
    vis: &Visual,
    above: Option<&crate::el::Anchor>,
    parent: Option<usize>,
    nodes: &[Node],
) -> Option<Rc<(Anim, Anim)>> {
    vis.hover_within.clone().or_else(|| {
        above
            .and_then(|a| nodes.iter().position(|n| n.key.as_ref() == Some(&a.key)))
            .and_then(|j| nodes[j].hover_within.clone())
            .or_else(|| parent.and_then(|p| nodes[p].hover_within.clone()))
    })
}

#[inline(always)]
fn needs_node_cold(c: &crate::el::ElCold, meta: &ColdMeta) -> bool {
    let base = meta.trace_namespace.is_some() || meta.pixel_aspect.is_some() || meta.aspect_fit.is_some()
        || c.label.is_some() || c.help.is_some() || c.transition.is_some()
        || !c.live.is_empty() || c.scroll.is_some() || c.matched.is_some()
        || c.wheel_scrolls.is_some()
        || c.above.is_some() || c.above_boundary.is_some() || c.growth.is_some() || c.sticky.is_some()
        || c.trace_name.is_some() || c.looping.is_some() || c.loop_times.is_some() || c.frame_motion.is_some() || c.opacity_motion.is_some()
        || c.transition_motion.is_some() || c.enter_in_instant || c.matched_motion.is_some()
        || c.lead.is_some() || c.glide.is_some() || c.material.is_some()
        || c.help_cut || c.help_side.is_some() || c.help_open || !c.data.is_empty()
        || c.scroll_axes != crate::el::ScrollAxes::Y
        || c.scroll_edges != crate::el::ScrollEdges::default()
        || c.a11y != crate::el::A11y::default();
    #[cfg(debug_assertions)]
    let base = base || c.scroll_source.is_some();
    base
}

#[inline(always)]
fn node_cold_meta_only(meta: ColdMeta) -> NodeCold {
    NodeCold {
        trace_name: None, trace_namespace: meta.trace_namespace, label: None, live: Vec::new(),
        run_layout: None, tabular_layout: None, live_text_seen: None, looping: None, loop_times: None, frame_motion: None, opacity_motion: None, lead: None,
        transition: None, transition_motion: None, enter_in_instant: false, matched: None, matched_motion: None, above: None, above_boundary: None, resolved_anchor: None, scroll: None, scroll_axes: crate::el::ScrollAxes::Y,
        scroll_edges: crate::el::ScrollEdges::default(), wheel_scrolls: None, sticky: None, growth: None, glide: None,
        a11y: crate::el::A11y::EMPTY, pixel_aspect: meta.pixel_aspect, aspect_fit: meta.aspect_fit,
        material: None, help: None, help_cut: false, help_side: None, help_open: false, data: Vec::new(),
        #[cfg(debug_assertions)]
        scroll_source: None,
    }
}

#[cold]
#[inline(never)]
fn take_cold_input(cold: &mut crate::cold::Cold<crate::el::ElCold>) -> Option<Box<InputSpec>> {
    cold.get_mut_if_some().and_then(|c| c.input.take())
}

#[cold]
#[inline(never)]
fn merge_path_handlers(
    dst: &mut crate::cold::Cold<crate::el::Handlers>,
    patch: &mut crate::el::PathBind,
) {
    if let Some(next) = patch.on_pointer_move.take() {
        let h: &mut crate::el::Handlers = &mut *dst;
        h.on_pointer_move = Some(match h.on_pointer_move.take() {
            Some(prev) => Rc::new(move |st, p, tx| {
                prev(st, p, tx);
                let prev_live = std::mem::take(&mut tx.live_only);
                next(p, tx);
                tx.live_only &= prev_live;
            }),
            None => Rc::new(move |_, p, tx| next(p, tx)),
        });
        #[cfg(debug_assertions)]
        if h.sites.pointer_move.is_none() {
            h.sites.pointer_move = patch.pointer_move_site;
        }
    }
    if let Some(next) = patch.on_hover_change.take() {
        let h: &mut crate::el::Handlers = &mut *dst;
        h.on_hover_change = Some(match h.on_hover_change.take() {
            Some(prev) => Rc::new(move |st, inside, tx| {
                prev(st, inside, tx);
                let prev_live = std::mem::take(&mut tx.live_only);
                next(inside, tx);
                tx.live_only &= prev_live;
            }),
            None => Rc::new(move |_, inside, tx| next(inside, tx)),
        });
        #[cfg(debug_assertions)]
        if h.sites.hover_change.is_none() {
            h.sites.hover_change = patch.hover_change_site;
        }
    }
    if let Some(next) = patch.on_scroll_offset.take() {
        let h: &mut crate::el::Handlers = &mut *dst;
        h.on_scroll_offset = Some(match h.on_scroll_offset.take() {
            Some(prev) => Rc::new(move |at| {
                prev(at);
                next(at);
            }),
            None => next,
        });
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::El {}
    impl Sealed for &super::El {}
}

trait StyleBuf {
    fn as_ref(&self) -> &CompactStyle;
    fn into_owned(self) -> CompactStyle;
}

impl StyleBuf for CompactStyle {
    #[inline(always)] fn as_ref(&self) -> &CompactStyle { self }
    #[inline(always)] fn into_owned(self) -> CompactStyle { self }
}

impl StyleBuf for Cow<'_, CompactStyle> {
    #[inline(always)] fn as_ref(&self) -> &CompactStyle { self }
    #[inline(always)] fn into_owned(self) -> CompactStyle {
        match self { Cow::Borrowed(s) => s.clone(), Cow::Owned(s) => s }
    }
}

#[derive(Clone, Copy)]
struct ChildEnv { me: usize, motion: Spring, in_row: RowLayout, row: bool, sx: Option<ScrollXChild>, attach_taffy: bool }

struct RefScratch<'a> {
    style: Cow<'a, CompactStyle>,
    vis: Visual,
    text: Option<crate::cold::SharedBox<TextSpec>>,
}

struct ColdMeta {
    trace_namespace: Option<(&'static str, &'static str)>,
    mirror: bool,
    pixel_aspect: Option<f32>,
    aspect_fit: Option<Box<(f32, CompactStyle)>>,
}

macro_rules! take_field {
    (move, $e:expr) => { $e };
    (clone, $e:expr) => { $e.clone() };
}

macro_rules! node_cold_from {
    ($mode:ident, $c:ident, $m:ident) => {{
        let mut above = take_field!($mode, $c.above);
        if $m.mirror && let Some(a) = &mut above { crate::el::mirror_anchor(a); }
        let matched = take_field!($mode, $c.matched).map(|id| (id, $c.motion));
        let help_side = if $m.mirror { $c.help_side.map(crate::el::mirror_side) } else { $c.help_side };
        NodeCold {
            trace_name: $c.trace_name, trace_namespace: $m.trace_namespace,
            label: take_field!($mode, $c.label), live: take_field!($mode, $c.live),
            run_layout: None, tabular_layout: None, live_text_seen: None,
            looping: take_field!($mode, $c.looping), loop_times: $c.loop_times, frame_motion: take_field!($mode, $c.frame_motion),
            opacity_motion: take_field!($mode, $c.opacity_motion), lead: $c.lead,
            transition: take_field!($mode, $c.transition), transition_motion: take_field!($mode, $c.transition_motion),
            enter_in_instant: $c.enter_in_instant,
            matched, matched_motion: take_field!($mode, $c.matched_motion), above, above_boundary: take_field!($mode, $c.above_boundary), resolved_anchor: None,
            scroll: $c.scroll, scroll_axes: $c.scroll_axes, scroll_edges: $c.scroll_edges,
            wheel_scrolls: take_field!($mode, $c.wheel_scrolls),
            sticky: take_field!($mode, $c.sticky), growth: take_field!($mode, $c.growth),
            glide: $c.glide, a11y: take_field!($mode, $c.a11y),
            pixel_aspect: $m.pixel_aspect, aspect_fit: $m.aspect_fit, material: $c.material,
            help: take_field!($mode, $c.help), help_cut: $c.help_cut, help_side, help_open: $c.help_open,
            data: take_field!($mode, $c.data),
            #[cfg(debug_assertions)]
            scroll_source: $c.scroll_source,
        }
    }};
}

#[inline(never)]
fn pack_cold_owned(cold: crate::cold::Cold<crate::el::ElCold>, meta: ColdMeta) -> crate::cold::Cold<NodeCold> {
    let Some(c_ref) = cold.get() else { return crate::cold::Cold::from_owned(node_cold_meta_only(meta)); };
    if !needs_node_cold(c_ref, &meta) { return crate::cold::Cold::default(); }
    let c = cold.into_owned();
    crate::cold::Cold::from_owned(node_cold_from!(move, c, meta))
}

#[inline(never)]
fn pack_cold_ref(c: Option<&crate::el::ElCold>, meta: ColdMeta) -> crate::cold::Cold<NodeCold> {
    let Some(c) = c else { return crate::cold::Cold::from_owned(node_cold_meta_only(meta)); };
    if !needs_node_cold(c, &meta) { return crate::cold::Cold::default(); }
    crate::cold::Cold::from_owned(node_cold_from!(clone, c, meta))
}

type FinishedWork<S, C, K> = (
    Option<String>,
    crate::cold::Cold<crate::el::Handlers>,
    Option<crate::cold::SharedBox<TextSpec>>,
    Visual,
    C,
    S,
    K,
);

trait FlattenEl: sealed::Sealed {
    type Scratch;
    type Style: StyleBuf;
    type Cold;
    type Children;

    fn as_el(&self) -> &El;
    fn new_scratch(&mut self) -> (Self::Scratch, Option<Box<InputSpec>>);
    fn style<'a>(&'a self, s: &'a Self::Scratch) -> &'a CompactStyle;
    fn style_mut<'a>(&'a mut self, s: &'a mut Self::Scratch) -> &'a mut CompactStyle;
    fn replace_style(&mut self, s: &mut Self::Scratch, style: CompactStyle) -> CompactStyle;
    fn vis<'a>(&'a self, s: &'a Self::Scratch) -> &'a Visual;
    fn vis_mut<'a>(&'a mut self, s: &'a mut Self::Scratch) -> &'a mut Visual;
    fn text_mut<'a>(&'a mut self, s: &'a mut Self::Scratch) -> &'a mut Option<crate::cold::SharedBox<TextSpec>>;
    fn text_and_vis_mut<'a>(&'a mut self, s: &'a mut Self::Scratch) -> (&'a mut Option<crate::cold::SharedBox<TextSpec>>, &'a mut Visual);
    fn style_mut_and_vis<'a>(&'a mut self, s: &'a mut Self::Scratch) -> (&'a mut CompactStyle, &'a Visual);
    fn finish(self, s: Self::Scratch) -> FinishedWork<Self::Style, Self::Cold, Self::Children>;
    fn cold_is_empty(cold: &Self::Cold) -> bool;
    fn pack_cold(cold: Self::Cold, meta: ColdMeta) -> crate::cold::Cold<NodeCold>;
    #[allow(clippy::too_many_arguments)]
    fn flatten_children(
        children: Self::Children, env: ChildEnv, path_buf: &mut String,
        taffy: &mut LayoutEngine, nodes: &mut Vec<Node>, scale: f32,
        memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
    ) -> bool;
}

impl FlattenEl for El {
    type Scratch = ();
    type Style = CompactStyle;
    type Cold = crate::cold::Cold<crate::el::ElCold>;
    type Children = (Option<Rc<crate::memo::Built>>, Vec<El>);

    #[inline(always)] fn as_el(&self) -> &El { self }
    #[inline(always)] fn new_scratch(&mut self) -> ((), Option<Box<InputSpec>>) {
        // Only a text field's element has to own its cold fields here; a memo placeholder shares them.
        let input = if self.cold.input.is_some() { take_cold_input(&mut self.cold) } else { None };
        ((), input)
    }
    #[inline(always)] fn style<'a>(&'a self, _: &'a ()) -> &'a CompactStyle { &self.style }
    #[inline(always)] fn style_mut<'a>(&'a mut self, _: &'a mut ()) -> &'a mut CompactStyle { &mut self.style }
    #[inline(always)] fn replace_style(&mut self, _: &mut (), style: CompactStyle) -> CompactStyle { std::mem::replace(&mut self.style, style) }
    #[inline(always)] fn vis<'a>(&'a self, _: &'a ()) -> &'a Visual { &self.vis }
    #[inline(always)] fn vis_mut<'a>(&'a mut self, _: &'a mut ()) -> &'a mut Visual { &mut self.vis }
    #[inline(always)] fn text_mut<'a>(&'a mut self, _: &'a mut ()) -> &'a mut Option<crate::cold::SharedBox<TextSpec>> { &mut self.text }
    #[inline(always)] fn text_and_vis_mut<'a>(&'a mut self, _: &'a mut ()) -> (&'a mut Option<crate::cold::SharedBox<TextSpec>>, &'a mut Visual) { (&mut self.text, &mut self.vis) }
    #[inline(always)] fn style_mut_and_vis<'a>(&'a mut self, _: &'a mut ()) -> (&'a mut CompactStyle, &'a Visual) { (&mut self.style, &self.vis) }
    #[inline(always)] fn finish(self, _: ()) -> FinishedWork<Self::Style, Self::Cold, Self::Children> {
        let El { key, handlers, text, vis, cold, style, memo, children, .. } = self;
        (key, handlers, text, vis, cold, style, (memo, children))
    }
    #[inline(always)] fn cold_is_empty(cold: &Self::Cold) -> bool { cold.is_empty() }
    #[inline(always)] fn pack_cold(cold: Self::Cold, meta: ColdMeta) -> crate::cold::Cold<NodeCold> { pack_cold_owned(cold, meta) }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn flatten_children(
        (memo, children): Self::Children, env: ChildEnv, path_buf: &mut String,
        taffy: &mut LayoutEngine, nodes: &mut Vec<Node>, scale: f32,
        memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
    ) -> bool {
        let mut children_dirty = false;
        let shared_len = memo.as_ref().map_or(0, |b| b.children.children.len());
        if let Some(built) = &memo {
            for (i, c) in built.children.children.iter().enumerate() {
                let (kid, kid_dirty) = flatten_ref(c, Some(env.me), path_buf, i, env.motion, env.in_row, env.row, env.sx, taffy, nodes, scale, memo_ctx);
                if env.attach_taffy { taffy.push_child(kid); }
                children_dirty |= kid_dirty;
            }
        }
        for (j, c) in children.into_iter().enumerate() {
            let (kid, kid_dirty) = flatten_owned(c, Some(env.me), path_buf, shared_len + j, env.motion, env.in_row, env.row, env.sx, taffy, nodes, scale, memo_ctx);
            if env.attach_taffy { taffy.push_child(kid); }
            children_dirty |= kid_dirty;
        }
        children_dirty
    }
}

impl<'e> FlattenEl for &'e El {
    type Scratch = RefScratch<'e>;
    type Style = Cow<'e, CompactStyle>;
    type Cold = Option<&'e crate::el::ElCold>;
    type Children = &'e El;

    #[inline(always)] fn as_el(&self) -> &El { self }
    #[inline(always)] fn new_scratch(&mut self) -> (RefScratch<'e>, Option<Box<InputSpec>>) {
        let input = self.cold.input.clone();
        (RefScratch { style: Cow::Borrowed(&self.style), vis: self.vis.clone(), text: self.text.clone() }, input)
    }
    #[inline(always)] fn style<'a>(&'a self, s: &'a RefScratch<'e>) -> &'a CompactStyle { &s.style }
    #[inline(always)] fn style_mut<'a>(&'a mut self, s: &'a mut RefScratch<'e>) -> &'a mut CompactStyle { s.style.to_mut() }
    #[inline(always)] fn replace_style(&mut self, s: &mut RefScratch<'e>, style: CompactStyle) -> CompactStyle {
        std::mem::replace(&mut s.style, Cow::Owned(style)).into_owned()
    }
    #[inline(always)] fn vis<'a>(&'a self, s: &'a RefScratch<'e>) -> &'a Visual { &s.vis }
    #[inline(always)] fn vis_mut<'a>(&'a mut self, s: &'a mut RefScratch<'e>) -> &'a mut Visual { &mut s.vis }
    #[inline(always)] fn text_mut<'a>(&'a mut self, s: &'a mut RefScratch<'e>) -> &'a mut Option<crate::cold::SharedBox<TextSpec>> { &mut s.text }
    #[inline(always)] fn text_and_vis_mut<'a>(&'a mut self, s: &'a mut RefScratch<'e>) -> (&'a mut Option<crate::cold::SharedBox<TextSpec>>, &'a mut Visual) { (&mut s.text, &mut s.vis) }
    #[inline(always)] fn style_mut_and_vis<'a>(&'a mut self, s: &'a mut RefScratch<'e>) -> (&'a mut CompactStyle, &'a Visual) { (s.style.to_mut(), &s.vis) }
    #[inline(always)] fn finish(self, s: RefScratch<'e>) -> FinishedWork<Self::Style, Self::Cold, Self::Children> {
        (self.key.clone(), self.handlers.clone(), s.text, s.vis, self.cold.get(), s.style, self)
    }
    #[inline(always)] fn cold_is_empty(cold: &Self::Cold) -> bool { cold.is_none() }
    #[inline(always)] fn pack_cold(cold: Self::Cold, meta: ColdMeta) -> crate::cold::Cold<NodeCold> { pack_cold_ref(cold, meta) }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn flatten_children(
        el: Self::Children, env: ChildEnv, path_buf: &mut String,
        taffy: &mut LayoutEngine, nodes: &mut Vec<Node>, scale: f32,
        memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
    ) -> bool {
        let mut children_dirty = false;
        for (i, c) in el.all_children().enumerate() {
            let (kid, kid_dirty) = flatten_ref(c, Some(env.me), path_buf, i, env.motion, env.in_row, env.row, env.sx, taffy, nodes, scale, memo_ctx);
            if env.attach_taffy { taffy.push_child(kid); }
            children_dirty |= kid_dirty;
        }
        children_dirty
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::runtime) fn flatten_with_buf(
    el: El, parent: Option<usize>, path_buf: &mut String, index: usize, motion: Spring,
    in_row: RowLayout,
    // The parent lays its children out in a row (its main axis is horizontal).
    parent_row: bool,
    taffy: &mut LayoutEngine, nodes: &mut Vec<Node>, scale: f32,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> (SlotId, bool) {
    if path_buf.capacity() < path_buf.len() + 128 { path_buf.reserve(128); }
    flatten_owned(el, parent, path_buf, index, motion, in_row, parent_row, None, taffy, nodes, scale, memo_ctx)
}

// Keep both monomorphizations in this module's codegen unit alongside its private helpers
// (`push_segment`, `prepare_text`, `take_foreground`, `inherit_hover_within`, `pack_cold_*`)
// so LLVM retains intra-CGU attributes and layout under `codegen-units = 16, lto = false`.
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn flatten_owned(
    el: El, parent: Option<usize>, path_buf: &mut String, index: usize, motion: Spring,
    in_row: RowLayout, parent_row: bool, parent_sx: Option<ScrollXChild>,
    taffy: &mut LayoutEngine, nodes: &mut Vec<Node>, scale: f32,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> (SlotId, bool) {
    rune_taffy::maybe_grow(|| {
        flatten_node(el, parent, path_buf, index, motion, in_row, parent_row, parent_sx, taffy, nodes, scale, memo_ctx)
    })
}

#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn flatten_ref(
    el: &El, parent: Option<usize>, path_buf: &mut String, index: usize, motion: Spring,
    in_row: RowLayout, parent_row: bool, parent_sx: Option<ScrollXChild>,
    taffy: &mut LayoutEngine, nodes: &mut Vec<Node>, scale: f32,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> (SlotId, bool) {
    rune_taffy::maybe_grow(|| {
        flatten_node(el, parent, path_buf, index, motion, in_row, parent_row, parent_sx, taffy, nodes, scale, memo_ctx)
    })
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn flatten_node<E: FlattenEl>(
    mut el: E, parent: Option<usize>, path_buf: &mut String, index: usize, motion: Spring,
    in_row: RowLayout, parent_row: bool, parent_sx: Option<ScrollXChild>,
    taffy: &mut LayoutEngine, nodes: &mut Vec<Node>, scale: f32,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> (SlotId, bool) {
    let el_ref = el.as_el();
    if parent_sx.is_none()
        && let Some(slot) = memo_reuse::try_reuse_memo(el_ref, parent, path_buf, index, motion, in_row, scale, taffy, nodes, memo_ctx)
    {
        return (slot, false);
    }
    if el_ref.flags.has(ElFlags::HAS_VIRTUAL) && el_ref.memo.is_some() {
        memo_ctx.missed_virtual_memo = true;
    }
    let record_built = parent_sx.is_none().then(|| {
        let built = el_ref.memo_built().filter(|b| b.reusable && memo_reuse::same_root_el(el_ref, b, false))
            .or_else(|| el_ref.cold.expanded_memo.as_ref().filter(|b| b.reusable && memo_reuse::same_root_el(el_ref, b, true)))?;
        let (inherited, hover_within) = memo_reuse::inherited_for(parent, nodes, motion, in_row, scale, memo_ctx.foreground);
        Some((built.id, inherited, hover_within))
    }).flatten();
    let prev_len = path_buf.len();
    push_segment(path_buf, el_ref.key.as_deref(), index);
    let me = nodes.len();
    let (path, id) = resolve_path_and_id(path_buf, me, memo_ctx);
    let motion = el_ref.cold.motion.unwrap_or(motion);
    let parent_is_stack = parent.is_some_and(|p| nodes[p].growth.is_some());
    let flags = el_ref.flags;
    let animate_layout = flags.has(ElFlags::ANIMATE_LAYOUT);
    let in_row = if el_ref.cold.above.is_some() && (animate_layout || in_row == RowLayout::Horizontal) {
        RowLayout::Free
    } else if animate_layout && in_row == RowLayout::Snaps {
        RowLayout::Horizontal
    } else {
        in_row
    };
    let animated_changes = el_ref.cold.animated_changes;
    let instant_changes = flags.has(ElFlags::INSTANT_CHANGES) || (parent.is_some_and(|p| nodes[p].instant_changes) && !animated_changes);
    let trace_namespace = el_ref.cold.trace_namespace.or_else(|| parent.and_then(|p| nodes[p].trace_namespace));
    let parent_flow = parent.map_or(crate::el::Flow::Ltr, |p| nodes[p].flow);
    let (flow, dir_effect) = crate::el::step_direction(parent_flow, crate::el::DirEvent::of(el_ref));
    let directed = flags.dir().is_some() || parent.is_some_and(|p| nodes[p].directed);
    let inert = flags.has(ElFlags::INERT) || parent.is_some_and(|p| nodes[p].inert);
    let selection_off = crate::el::selection_off(el_ref.cold.selection_enabled, parent.is_some_and(|p| nodes[p].selection_off));
    let mirror = dir_effect == crate::el::DirEffect::Mirror;
    let (mut s, mut input) = el.new_scratch();
    if let Some(sx) = parent_sx { apply_scroll_x_child(el.style_mut(&mut s), sx); }
    let (measure, secure_measure) = {
        let text = el.text_mut(&mut s);
        if text.is_some() { prepare_text(text, input.as_deref(), scale, directed.then_some(flow)) } else { (None, false) }
    };
    if memo_ctx.foreground.is_some() {
        let (text_m, vis_m) = el.text_and_vis_mut(&mut s);
        take_foreground(text_m, input.as_deref_mut(), vis_m, memo_ctx.foreground);
    }
    if el.vis(&s).border_layout {
        let (style, vis) = el.style_mut_and_vis(&mut s);
        device::border(style, vis, scale);
    }
    if let Some(metrics) = el.as_el().cold.list_disc { device::disc(el.style_mut(&mut s), metrics, scale); }
    if mirror { crate::el::mirror_visual(el.vis_mut(&mut s)); }
    let hover_within = inherit_hover_within(el.vis(&s), el.as_el().cold.above.as_ref(), parent, nodes);
    let pixel_aspect = el.vis(&s).pixels.as_ref()
        .filter(|_| {
            el.vis(&s).pixels_wrap.is_none() && {
                let st = el.style(&s);
                let sz = st.size();
                st.aspect_ratio().is_none() && sz.width.is_auto() != sz.height.is_auto()
            }
        })
        .map(pixel_ratio);
    let picture = pixel_aspect.filter(|&r| r > 0.0).map(|ratio| (ratio, picture_side(el.style_mut(&mut s))));
    if let Some((ratio, Picture::Styled)) = picture { el.style_mut(&mut s).set_aspect_ratio(Some(ratio)); }
    let measure = measure.or(picture.filter(|p| p.1 == Picture::Measured).map(|p| picture_key(p.0)));
    let aspect_fit = {
        let af = el.as_el().cold.aspect_fit;
        (af > 0.0).then(|| {
            let room = fit_room(el.style(&s), parent_row);
            Box::new((af, el.replace_style(&mut s, room)))
        })
    };
    let child_sx = {
        let r = el.as_el();
        let c = &*r.cold;
        if c.scroll.is_some() && c.scroll_axes.x() {
            let scroll_axes = c.scroll_axes;
            let total_children = r.memo.as_ref().map_or(0, |b| b.children.children.len()) + r.children.len();
            if total_children == 1 {
                el.style_mut(&mut s).set_flex_direction(FlexDirection::Row);
                Some(ScrollXChild::Single { vertical: scroll_axes.y() })
            } else {
                if !scroll_axes.y() { el.style_mut(&mut s).set_flex_direction(FlexDirection::Row); }
                Some(ScrollXChild::Many { vertical: scroll_axes.y() })
            }
        } else {
            None
        }
    };
    let el_r = el.as_el();
    let role = el_r.role_name();
    let pin = crate::el::Pin::of(el_r);
    let vis_flags = el_r.vis_flags;
    let static_scale = el_r.scale;
    let time_scale = el_r.cold.time_scale;
    let option_as_alt = el_r.cold.option_as_alt;
    let window_drag = el_r.window_drag_mode();
    let virtual_list = el_r.cold.virtual_estimate.is_some();
    let virtual_items = el_r.cold.virtual_items.is_some();
    let mut focus = el_r.cold.focus.clone();
    if (focus.disabled || focus.opt_out) && (focus.enabled || !focus.opt_out) {
        let f = &mut *focus;
        f.enabled = false;
        f.opt_out = true;
    }
    let on_path = el_r.cold.get().and_then(|c| c.on_path.clone());
    let child_count = if on_path.is_some() {
        el_r.memo.as_ref().map_or(0, |b| b.children.children.len()) + el_r.children.len()
    } else {
        0
    };
    let (key, mut handlers, mut text, vis, cold, style, children) = el.finish(s);
    if selection_off { crate::el::apply_selection_off(&mut text, &mut handlers); }
    let extra_children = if let Some(bind) = on_path {
        let mut patch = crate::el::PathBind::default();
        bind(&path, &mut patch);
        merge_path_handlers(&mut handlers, &mut patch);
        patch.children
    } else {
        Vec::new()
    };
    let in_flow = {
        let st = style.as_ref();
        !matches!(st.position(), Position::Absolute | Position::Fixed) && st.display() != Display::None
    };
    let cold = if E::cold_is_empty(&cold) && trace_namespace.is_none() && pixel_aspect.is_none() && aspect_fit.is_none() {
        crate::cold::Cold::default()
    } else {
        E::pack_cold(cold, ColdMeta { trace_namespace, mirror, pixel_aspect, aspect_fit })
    };
    nodes.push(Node {
        id, path, key, role, parent, taffy: SlotId(0), text, text_layout: None, elided: None,
        input, focus, handlers, cold, vis, vis_flags, hover_within,
        target: [0.0; 4], local: MRect::ZERO, inset: [0.0; 4],
        motion, static_scale, time_scale, in_row,
        width_leads: flags.has(ElFlags::WIDTH_LEADS),
        instant_changes,
        animated_changes,
        streaming: flags.has(ElFlags::STREAMING),
        whole: flags.has(ElFlags::WHOLE),
        inert, selection_off, flow, directed, pin, scale_track: false, group_fade: false,
        virtual_mount: flags.has(ElFlags::VIRTUAL_MOUNT),
        virtual_list,
        virtual_items,
        selection_group: flags.has(ElFlags::SELECTION_GROUP),
        window_drag,
        ime_cursor: flags.has(ElFlags::IME_CURSOR),
        ime_option_as_alt: option_as_alt,
        in_flow,
        collapsing: false,
        moved_stub: false,
        epoch: crate::runtime::paint_keep::next_epoch(),
    });
    if nodes[me].focus.caret_browsing
        && !nodes[me].focus.opt_out
        && !selection_off
        && nodes[me].input.is_none()
        && (nodes[me].selection_group || nodes[me].text.as_ref().is_some_and(|t| t.selectable && !t.redacted))
    {
        nodes[me].focus.enabled = true;
    }
    let child_in_row = match in_row {
        RowLayout::Free if parent_is_stack => if animate_layout { RowLayout::Horizontal } else { RowLayout::Snaps },
        r => r,
    };
    let row = matches!(style.as_ref().flex_direction(), FlexDirection::Row | FlexDirection::RowReverse);
    let attach_taffy = nodes[me].text.as_ref().is_none_or(|t| t.cold.inline_boxes.is_empty());
    let child_mark = taffy.begin_children();
    let env = ChildEnv { me, motion, in_row: child_in_row, row, sx: child_sx, attach_taffy };
    let mut children_dirty = E::flatten_children(children, env, path_buf, taffy, nodes, scale, memo_ctx);
    for (k, extra) in extra_children.into_iter().enumerate() {
        let (kid, kid_dirty) = flatten_owned(extra, Some(env.me), path_buf, child_count + k, env.motion, env.in_row, env.row, env.sx, taffy, nodes, scale, memo_ctx);
        if attach_taffy { taffy.push_child(kid); }
        children_dirty |= kid_dirty;
    }
    path_buf.truncate(prev_len);
    let (slot, dirty) = taffy.sync_node(id.0, me as u32, style.into_owned(), measure, secure_measure, child_mark, children_dirty);
    nodes[me].taffy = slot;
    if let Some((built_id, inherited, hover_within)) = record_built {
        memo_ctx.pending.push(memo_reuse::PendingFlat {
            built_id, start: me, len: nodes.len() - me, inherited, hover_within,
        });
    }
    (slot, dirty)
}

#[cfg(test)]
#[path = "flatten_tests.rs"]
mod flatten_tests;
