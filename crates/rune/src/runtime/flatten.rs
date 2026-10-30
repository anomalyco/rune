use std::borrow::Cow;

use super::*;

/// An `aspect_fit` element's first layout: it takes the free space along its parent's main axis
/// and the whole cross side, within its max sizes, as the room its fitted box must fit. Its
/// content never sizes the room: with no max along the main axis it starts from 0 and grows.
pub(super) fn fit_room(own: &Style, parent_row: bool) -> Style {
    let zero = |d: LengthPercentageAuto| if d.is_auto() { length(0.0) } else { d };
    let main_max = if parent_row { own.max_size.width } else { own.max_size.height };
    Style {
        size: own.max_size.map(Into::into),
        min_size: Size { width: zero(own.min_size.width), height: zero(own.min_size.height) },
        flex_grow: 1.0,
        flex_shrink: 1.0,
        flex_basis: if main_max.is_auto() { length(0.0) } else { auto() },
        align_self: Some(AlignSelf::STRETCH),
        aspect_ratio: None,
        ..own.clone()
    }
}

/// The largest box at `ratio` inside `room`, as the fixed size an `aspect_fit` element lays
/// out at once its room is known.
pub(super) fn fitted(ratio: f32, own: &Style, room: Size<f32>) -> Style {
    let w = room.width.min(room.height * ratio).max(0.0);
    Style {
        size: Size { width: length(w), height: length(w / ratio) },
        flex_grow: 0.0,
        flex_shrink: 0.0,
        flex_basis: auto(),
        aspect_ratio: None,
        ..own.clone()
    }
}

/// Width over height of the image in `pixels`, or 0 while it's empty.
pub(super) fn pixel_ratio(pixels: &crate::Pixels) -> f32 {
    let (w, h) = pixels.size();
    if w > 0 && h > 0 { w as f32 / h as f32 } else { 0.0 }
}

// Build the owned path directly, without cloning a key or formatting an index into a
// temporary segment first. Measurement and flattening must use the same identity rule.
/// Whether `path` lies inside an element keyed `scope` (always, without one).
pub(crate) fn in_scope(path: &str, scope: Option<&str>) -> bool {
    scope.is_none_or(|s| path.split('/').any(|seg| seg == s))
}

pub(super) fn node_path(parent: &str, key: Option<&str>, index: usize) -> String {
    match (parent.is_empty(), key) {
        (true, Some(key)) => key.to_owned(),
        (true, None) => format!("#{index}"),
        // Sized up front: `format!` guesses short and reallocates every deep path, twice a
        // commit per element.
        (false, Some(key)) => {
            let mut path = String::with_capacity(parent.len() + 1 + key.len());
            path.push_str(parent);
            path.push('/');
            path.push_str(key);
            path
        }
        (false, None) => {
            use std::fmt::Write;
            let mut path = String::with_capacity(parent.len() + 22);
            path.push_str(parent);
            let _ = write!(path, "/#{index}");
            path
        }
    }
}

fn push_segment(path: &mut String, key: Option<&str>, index: usize) {
    if !path.is_empty() {
        path.push('/');
    }
    match key {
        Some(k) => path.push_str(k),
        None => {
            use std::fmt::Write;
            let _ = write!(path, "#{index}");
        }
    }
}

#[derive(Clone, Copy)]
enum ScrollXChild {
    Single { vertical: bool },
    Many { vertical: bool },
}

fn apply_scroll_x_child(style: &mut Style, sx: ScrollXChild) {
    match sx {
        ScrollXChild::Single { vertical } => unwrapped_width(style, vertical),
        ScrollXChild::Many { vertical } => {
            let w_full = style.size.width == Dimension::percent(1.0);
            if style.size.width.is_auto() || w_full {
                style.size.width = Dimension::max_content();
                if w_full && style.min_size.width.is_auto() {
                    style.min_size.width = percent(1.0);
                }
            }
            style.flex_shrink = 0.0;
            if vertical && style.align_self.is_none() {
                style.align_self = Some(AlignSelf::FLEX_START);
            }
        }
    }
}

fn prepare_text(
    text: &mut Option<crate::cold::SharedBox<TextSpec>>,
    input: Option<&InputSpec>,
    scale: f32,
) -> (bool, Option<u64>, bool) {
    if let Some(t) = text && t.redacted {
        // Rich links carry URLs and can expose them through hover, copy and tree children.
        use zeroize::Zeroize;
        let t: &mut TextSpec = t;
        for link in &mut t.links {
            link.id.zeroize();
            if let Some(label) = &mut link.label { label.zeroize(); }
        }
        t.links.clear();
        t.rings.clear();
        t.selectable = false;
        t.advance_in = None;
        t.live = None;
    }
    if input.is_some_and(|i| i.edit.is_hidden_multiline()) && let Some(t) = text {
        // The fixed mask stays one line even in a field narrower than twelve bullets.
        if t.wrap { t.wrap = false; }
    } else if input.is_none()
        && let Some(t) = text
        && t.redacted
    {
        t.case = None;
        if !t.secure_revealed {
            t.conceal();
        }
    }
    if let Some(t) = text && t.live.is_some() {
        assert!(input.is_none() && !t.redacted && !t.wrap && t.spans.is_empty() && t.reveal < 0.0 && !t.truncate,
            "live_text requires plain single-line text; use truncate_fade for overflow");
    }
    if input.is_none()
        && text.as_ref().is_some_and(|t| t.redacted || t.case.is_some() || !t.rings.is_empty())
        && let Some(t) = text
    {
        t.apply_case();
    }
    if text.as_ref().is_some_and(|t| t.redacted || t.shape.is_none_or(|s| s.0 != scale.to_bits()))
        && let Some(t) = text
    {
        t.key_shape(scale);
    }
    let measured = text.is_some();
    let measure = text.as_ref().map(|t| t.shape.map_or(0, |s| s.1) ^ u64::from(t.wrap) ^ (u64::from(t.truncate || t.truncate_fade) << 1)
        ^ ((t.max_lines as u64) << 2)
        ^ (u64::from(input.is_some_and(|i| i.edit.is_revealed()) || t.secure_revealed) << 3)
        ^ (u64::from(t.wrap_anywhere) << 4)
        ^ t.advance_in.as_ref().map_or(0, |(run, at)| run.hash.rotate_left(3) ^ (*at as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)));
    // Secure leaves remeasure without retaining a content-derived dictionary oracle.
    let secure_measure = text.as_ref().is_some_and(|t| t.redacted);
    let measure = if secure_measure { Some(0) } else { measure };
    (measured, measure, secure_measure)
}

fn inherit_hover_within(
    vis: &mut Visual,
    above: Option<&crate::el::Anchor>,
    parent: Option<usize>,
    nodes: &[Node],
) {
    if vis.hover_within.is_none() {
        let inherited = above
            .and_then(|a| nodes.iter().position(|n| n.key.as_ref() == Some(&a.key)))
            .and_then(|j| nodes[j].vis.hover_within.clone())
            .or_else(|| parent.and_then(|p| nodes[p].vis.hover_within.clone()));
        if inherited.is_some() {
            vis.cold_mut().hover_within = inherited;
        }
    }
    if vis.hover_motion.is_none()
        && let Some(hw) = vis.hover_within.clone()
    {
        vis.cold_mut().hover_motion = Some(hw);
    }
}

#[inline]
fn node_cold_needed(c: &NodeCold) -> bool {
    let base = c.trace_name.is_some()
        || c.trace_namespace.is_some()
        || c.label.is_some()
        || !c.live.is_empty()
        || c.looping.is_some()
        || c.opacity_motion.is_some()
        || c.lead.is_some()
        || c.transition.is_some()
        || c.matched.is_some()
        || c.above.is_some()
        || c.scroll.is_some()
        || c.scroll_axes != crate::el::ScrollAxes::Y
        || c.scroll_edges != crate::el::ScrollEdges::default()
        || c.sticky.is_some()
        || c.growth.is_some()
        || c.glide.is_some()
        || c.a11y != crate::el::A11y::default()
        || c.pixel_aspect.is_some()
        || c.aspect_fit.is_some()
        || c.material.is_some()
        || c.help.is_some()
        || c.help_cut;
    #[cfg(debug_assertions)]
    let base = base || c.scroll_source.is_some();
    base
}

#[inline]
fn pack_node_cold(c: NodeCold) -> crate::cold::Cold<NodeCold> {
    if node_cold_needed(&c) {
        crate::cold::Cold::from_owned(c)
    } else {
        crate::cold::Cold::default()
    }
}

fn sync_taffy_node(
    me: usize,
    style: Cow<'_, Style>,
    kids: &[taffy::NodeId],
    (measured, measure, secure_measure): (bool, Option<u64>, bool),
    taffy: &mut TaffyTree<usize>,
    nodes: &mut [Node],
    entries: &mut HashMap<String, LayoutEntry>,
) -> taffy::NodeId {
    let path = nodes[me].path.as_str();
    let id = if let Some(entry) = entries.get_mut(path) {
        let id = entry.id;
        if taffy.style(id).unwrap() != style.as_ref() {
            taffy.set_style(id, style.into_owned()).unwrap();
        }
        if !taffy.child_ids(id).eq(kids.iter().copied()) {
            taffy.set_children(id, kids).unwrap();
        }
        // Context indices can move after insertion/reorder without invalidating measurements.
        // Only text/style/scale changes invalidate the cached leaf result.
        if entry.measure.is_some() != measured {
            taffy.set_node_context(id, measured.then_some(me)).unwrap();
        } else if measured {
            *taffy.get_node_context_mut(id).unwrap() = me;
        }
        if secure_measure || entry.measure != measure {
            taffy.mark_dirty(id).unwrap();
        }
        entry.measure = measure;
        entry.used = true;
        id
    } else {
        let style = style.into_owned();
        let id = if !kids.is_empty() {
            taffy.new_with_children(style, kids).unwrap()
        } else if measured {
            taffy.new_leaf_with_context(style, me).unwrap()
        } else {
            taffy.new_leaf(style).unwrap()
        };
        if measured && taffy.get_node_context(id).is_none() {
            taffy.set_node_context(id, Some(me)).unwrap();
        }
        entries.insert(nodes[me].path.clone(), LayoutEntry { id, measure, used: true });
        id
    };
    nodes[me].taffy = id;
    id
}

#[allow(clippy::too_many_arguments)]
pub(super) fn flatten_with_buf(
    el: El,
    parent: Option<usize>,
    path_buf: &mut String,
    index: usize,
    motion: Spring,
    in_row: RowLayout,
    // The parent lays its children out in a row (its main axis is horizontal).
    parent_row: bool,
    taffy: &mut TaffyTree<usize>,
    nodes: &mut Vec<Node>,
    entries: &mut HashMap<String, LayoutEntry>,
    scale: f32,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> taffy::NodeId {
    if path_buf.capacity() < path_buf.len() + 128 {
        path_buf.reserve(128);
    }
    flatten_owned(el, parent, path_buf, index, motion, in_row, parent_row, None, taffy, nodes, entries, scale, memo_ctx)
}

#[allow(clippy::too_many_arguments)]
fn flatten_owned(
    el: El,
    parent: Option<usize>,
    path_buf: &mut String,
    index: usize,
    motion: Spring,
    in_row: RowLayout,
    parent_row: bool,
    parent_sx: Option<ScrollXChild>,
    taffy: &mut TaffyTree<usize>,
    nodes: &mut Vec<Node>,
    entries: &mut HashMap<String, LayoutEntry>,
    scale: f32,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> taffy::NodeId {
    if parent_sx.is_none()
        && let Some(id) = memo_reuse::try_reuse_memo(&el, parent, path_buf, index, motion, in_row, scale, taffy, nodes, entries, memo_ctx)
    {
        return id;
    }
    let record_built = parent_sx.is_none().then(|| {
        let built = el.memo_built.as_ref().filter(|b| b.reusable && memo_reuse::same_root_el(&el, b))?;
        let (inherited, hover_within) = memo_reuse::inherited_for(parent, nodes, motion, in_row, scale);
        Some((built.id, inherited, hover_within))
    }).flatten();
    let prev_len = path_buf.len();
    push_segment(path_buf, el.key.as_deref(), index);
    let path = path_buf.clone();
    let own_motion = el.motion;
    let motion = own_motion.unwrap_or(motion);
    let me = nodes.len();
    let parent_is_stack = parent.is_some_and(|p| nodes[p].growth.is_some());
    let animate_layout = el.animate_layout;
    let in_row = if el.above.is_some() && (animate_layout || in_row == RowLayout::Horizontal) {
        RowLayout::Free
    } else if animate_layout && in_row == RowLayout::Snaps {
        RowLayout::Horizontal
    } else {
        in_row
    };
    let virtual_mount = el.virtual_mount;
    let instant_changes = el.instant_changes || parent.is_some_and(|p| nodes[p].instant_changes);
    #[cfg(debug_assertions)]
    let scroll_source = el.scroll_source;
    let trace_namespace = el.trace_namespace.or_else(|| parent.and_then(|p| nodes[p].trace_namespace));
    let El {
        focus,
        handlers,
        looping,
        key,
        trace_name,
        role,
        label,
        mut style,
        mut vis,
        live,
        mut text,
        children,
        memo_children,
        above,
        opacity_motion,
        lead,
        width_leads,
        transition,
        matched,
        scroll,
        scroll_axes,
        sticky,
        scroll_edges,
        growth,
        streaming,
        whole,
        glide,
        input,
        window_drag,
        ime_cursor,
        scale: static_scale,
        time_scale,
        help,
        help_cut,
        material,
        aspect_fit,
        selection_group,
        a11y,
        inert,
        ..
    } = el;
    if let Some(sx) = parent_sx {
        apply_scroll_x_child(&mut style, sx);
    }
    let text_meta = prepare_text(&mut text, input.as_deref(), scale);
    let inert = inert || parent.is_some_and(|p| nodes[p].inert);
    inherit_hover_within(&mut vis, above.as_ref(), parent, nodes);
    let pixel_aspect = vis.pixels.as_ref()
        .filter(|_| style.aspect_ratio.is_none() && style.size.width.is_auto() != style.size.height.is_auto())
        .map(pixel_ratio);
    if let Some(ratio) = pixel_aspect.filter(|&r| r > 0.0) {
        style.aspect_ratio = Some(ratio);
    }
    let aspect_fit = (aspect_fit > 0.0).then(|| {
        let room = fit_room(&style, parent_row);
        Box::new((aspect_fit, std::mem::replace(&mut style, room)))
    });
    let total_children = memo_children.as_ref().map_or(0, |m| m.children.len()) + children.len();
    let child_sx = if scroll.is_some() && scroll_axes.x() {
        if total_children == 1 {
            style.flex_direction = FlexDirection::Row;
            Some(ScrollXChild::Single { vertical: scroll_axes.y() })
        } else {
            if !scroll_axes.y() {
                style.flex_direction = FlexDirection::Row;
            }
            Some(ScrollXChild::Many { vertical: scroll_axes.y() })
        }
    } else {
        None
    };
    let cold = pack_node_cold(NodeCold {
        trace_name,
        trace_namespace,
        label,
        live,
        run_layout: None,
        live_text_seen: None,
        looping,
        opacity_motion,
        lead,
        transition,
        matched: matched.map(|id| (id, own_motion)),
        above,
        scroll,
        scroll_axes,
        scroll_edges,
        sticky,
        growth,
        glide,
        a11y,
        pixel_aspect,
        aspect_fit,
        material,
        help,
        help_cut,
        #[cfg(debug_assertions)]
        scroll_source,
    });
    nodes.push(Node {
        id: NodeId(0),
        path,
        key,
        role,
        parent,
        taffy: taffy::NodeId::from(0u64),
        text,
        text_layout: None,
        elided: None,
        input,
        focus,
        handlers,
        cold,
        vis,
        target: [0.0; 4],
        local: MRect::ZERO,
        inset: [0.0; 4],
        motion,
        static_scale,
        time_scale,
        in_row,
        width_leads,
        instant_changes,
        streaming,
        whole,
        inert,
        scale_track: false,
        group_fade: false,
        virtual_mount,
        selection_group,
        window_drag,
        ime_cursor,
        moved_stub: false,
    });
    let child_in_row = match in_row {
        RowLayout::Free if parent_is_stack => if animate_layout { RowLayout::Horizontal } else { RowLayout::Snaps },
        r => r,
    };
    let row = matches!(style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
    let kids_start = memo_ctx.kids.len();
    memo_ctx.kids.reserve(total_children);
    let shared_len = memo_children.as_ref().map_or(0, |m| m.children.len());
    if let Some(shared) = &memo_children {
        for (i, c) in shared.children.iter().enumerate() {
            let kid = flatten_ref(c, Some(me), path_buf, i, motion, child_in_row, row, child_sx, taffy, nodes, entries, scale, memo_ctx);
            memo_ctx.kids.push(kid);
        }
    }
    for (j, c) in children.into_iter().enumerate() {
        let kid = flatten_owned(c, Some(me), path_buf, shared_len + j, motion, child_in_row, row, child_sx, taffy, nodes, entries, scale, memo_ctx);
        memo_ctx.kids.push(kid);
    }
    path_buf.truncate(prev_len);
    let id = sync_taffy_node(me, Cow::Owned(style), &memo_ctx.kids[kids_start..], text_meta, taffy, nodes, entries);
    memo_ctx.kids.truncate(kids_start);
    if let Some((built_id, inherited, hover_within)) = record_built {
        memo_ctx.pending.push(memo_reuse::PendingFlat {
            built_id,
            start: me,
            len: nodes.len() - me,
            inherited,
            hover_within,
        });
    }
    id
}

#[allow(clippy::too_many_arguments)]
fn flatten_ref(
    el: &El,
    parent: Option<usize>,
    path_buf: &mut String,
    index: usize,
    motion: Spring,
    in_row: RowLayout,
    parent_row: bool,
    parent_sx: Option<ScrollXChild>,
    taffy: &mut TaffyTree<usize>,
    nodes: &mut Vec<Node>,
    entries: &mut HashMap<String, LayoutEntry>,
    scale: f32,
    memo_ctx: &mut memo_reuse::MemoFlattenCtx<'_>,
) -> taffy::NodeId {
    if parent_sx.is_none()
        && let Some(id) = memo_reuse::try_reuse_memo(el, parent, path_buf, index, motion, in_row, scale, taffy, nodes, entries, memo_ctx)
    {
        return id;
    }
    let record_built = parent_sx.is_none().then(|| {
        let built = el.memo_built.as_ref().filter(|b| b.reusable && memo_reuse::same_root_el(el, b))?;
        let (inherited, hover_within) = memo_reuse::inherited_for(parent, nodes, motion, in_row, scale);
        Some((built.id, inherited, hover_within))
    }).flatten();
    let prev_len = path_buf.len();
    push_segment(path_buf, el.key.as_deref(), index);
    let path = path_buf.clone();
    let own_motion = el.motion;
    let motion = own_motion.unwrap_or(motion);
    let me = nodes.len();
    let parent_is_stack = parent.is_some_and(|p| nodes[p].growth.is_some());
    let animate_layout = el.animate_layout;
    let in_row = if el.above.is_some() && (animate_layout || in_row == RowLayout::Horizontal) {
        RowLayout::Free
    } else if animate_layout && in_row == RowLayout::Snaps {
        RowLayout::Horizontal
    } else {
        in_row
    };
    let instant_changes = el.instant_changes || parent.is_some_and(|p| nodes[p].instant_changes);
    let trace_namespace = el.trace_namespace.or_else(|| parent.and_then(|p| nodes[p].trace_namespace));
    let mut style: Cow<'_, Style> = Cow::Borrowed(&el.style);
    if let Some(sx) = parent_sx {
        apply_scroll_x_child(style.to_mut(), sx);
    }
    let mut text = el.text.clone();
    let input = el.input.clone();
    let text_meta = prepare_text(&mut text, input.as_deref(), scale);
    let inert = el.inert || parent.is_some_and(|p| nodes[p].inert);
    let mut vis = el.vis.clone();
    inherit_hover_within(&mut vis, el.above.as_ref(), parent, nodes);
    let pixel_aspect = vis.pixels.as_ref()
        .filter(|_| style.aspect_ratio.is_none() && style.size.width.is_auto() != style.size.height.is_auto())
        .map(pixel_ratio);
    if let Some(ratio) = pixel_aspect.filter(|&r| r > 0.0) {
        style.to_mut().aspect_ratio = Some(ratio);
    }
    let aspect_fit = if el.aspect_fit > 0.0 {
        let room = fit_room(&style, parent_row);
        let orig = std::mem::replace(&mut style, Cow::Owned(room)).into_owned();
        Some(Box::new((el.aspect_fit, orig)))
    } else {
        None
    };
    let total_children = el.memo_children.as_ref().map_or(0, |m| m.children.len()) + el.children.len();
    let child_sx = if el.scroll.is_some() && el.scroll_axes.x() {
        if total_children == 1 {
            style.to_mut().flex_direction = FlexDirection::Row;
            Some(ScrollXChild::Single { vertical: el.scroll_axes.y() })
        } else {
            if !el.scroll_axes.y() {
                style.to_mut().flex_direction = FlexDirection::Row;
            }
            Some(ScrollXChild::Many { vertical: el.scroll_axes.y() })
        }
    } else {
        None
    };
    let cold_any = el.trace_name.is_some()
        || trace_namespace.is_some()
        || el.label.is_some()
        || !el.live.is_empty()
        || el.looping.is_some()
        || el.opacity_motion.is_some()
        || el.lead.is_some()
        || el.transition.is_some()
        || el.matched.is_some()
        || el.above.is_some()
        || el.scroll.is_some()
        || el.scroll_axes != crate::el::ScrollAxes::Y
        || el.scroll_edges != crate::el::ScrollEdges::default()
        || el.sticky.is_some()
        || el.growth.is_some()
        || el.glide.is_some()
        || el.a11y != crate::el::A11y::default()
        || pixel_aspect.is_some()
        || aspect_fit.is_some()
        || el.material.is_some()
        || el.help.is_some()
        || el.help_cut;
    #[cfg(debug_assertions)]
    let cold_any = cold_any || el.scroll_source.is_some();
    let cold = if cold_any {
        crate::cold::Cold::from_owned(NodeCold {
            trace_name: el.trace_name,
            trace_namespace,
            label: el.label.clone(),
            live: el.live.clone(),
            run_layout: None,
            live_text_seen: None,
            looping: el.looping.clone(),
            opacity_motion: el.opacity_motion.clone(),
            lead: el.lead,
            transition: el.transition.clone(),
            matched: el.matched.as_ref().map(|id| (id.clone(), own_motion)),
            above: el.above.clone(),
            scroll: el.scroll,
            scroll_axes: el.scroll_axes,
            scroll_edges: el.scroll_edges,
            sticky: el.sticky.clone(),
            growth: el.growth.clone(),
            glide: el.glide,
            a11y: el.a11y,
            pixel_aspect,
            aspect_fit,
            material: el.material,
            help: el.help.clone(),
            help_cut: el.help_cut,
            #[cfg(debug_assertions)]
            scroll_source: el.scroll_source,
        })
    } else {
        crate::cold::Cold::default()
    };
    nodes.push(Node {
        id: NodeId(0),
        path,
        key: el.key.clone(),
        role: el.role,
        parent,
        taffy: taffy::NodeId::from(0u64),
        text,
        text_layout: None,
        elided: None,
        input,
        focus: el.focus.clone(),
        handlers: el.handlers.clone(),
        cold,
        vis,
        target: [0.0; 4],
        local: MRect::ZERO,
        inset: [0.0; 4],
        motion,
        static_scale: el.scale,
        time_scale: el.time_scale,
        in_row,
        width_leads: el.width_leads,
        instant_changes,
        streaming: el.streaming,
        whole: el.whole,
        inert,
        scale_track: false,
        group_fade: false,
        virtual_mount: el.virtual_mount,
        selection_group: el.selection_group,
        window_drag: el.window_drag,
        ime_cursor: el.ime_cursor,
        moved_stub: false,
    });
    let child_in_row = match in_row {
        RowLayout::Free if parent_is_stack => if animate_layout { RowLayout::Horizontal } else { RowLayout::Snaps },
        r => r,
    };
    let row = matches!(style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
    let kids_start = memo_ctx.kids.len();
    memo_ctx.kids.reserve(total_children);
    for (i, c) in el.all_children().enumerate() {
        let kid = flatten_ref(c, Some(me), path_buf, i, motion, child_in_row, row, child_sx, taffy, nodes, entries, scale, memo_ctx);
        memo_ctx.kids.push(kid);
    }
    path_buf.truncate(prev_len);
    let id = sync_taffy_node(me, style, &memo_ctx.kids[kids_start..], text_meta, taffy, nodes, entries);
    memo_ctx.kids.truncate(kids_start);
    if let Some((built_id, inherited, hover_within)) = record_built {
        memo_ctx.pending.push(memo_reuse::PendingFlat {
            built_id,
            start: me,
            len: nodes.len() - me,
            inherited,
            hover_within,
        });
    }
    id
}
