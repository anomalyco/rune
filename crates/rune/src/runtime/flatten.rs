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
        vis.hover_within = above
            .and_then(|a| nodes.iter().position(|n| n.key.as_ref() == Some(&a.key)))
            .and_then(|j| nodes[j].vis.hover_within.clone())
            .or_else(|| parent.and_then(|p| nodes[p].vis.hover_within.clone()));
    }
    if vis.hover_motion.is_none() {
        vis.hover_motion = vis.hover_within.clone();
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
pub(super) fn flatten(
    el: El,
    parent: Option<usize>,
    parent_path: &str,
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
    let mut path_buf = String::with_capacity(parent_path.len() + 128);
    path_buf.push_str(parent_path);
    flatten_owned(el, parent, &mut path_buf, index, motion, in_row, parent_row, None, taffy, nodes, entries, scale, memo_ctx)
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
        on_paste,
        on_window_focus,
        focus,
        modal,
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
        on_press,
        on_double_press,
        on_secondary_press,
        on_hover,
        on_hover_change,
        hover_delay,
        on_pointer_move,
        gestures,
        above,
        on_drag,
        drag_out,
        drag_threshold,
        on_drag_start,
        on_drag_end,
        drop,
        opacity_motion,
        lead,
        width_leads,
        transition,
        matched,
        scroll,
        scroll_axes,
        on_at_bottom,
        on_far_from_bottom,
        on_visible_rows,
        on_layout,
        on_screen_layout,
        on_drawn_layout,
        on_scroll_offset,
        sticky,
        scroll_edges,
        growth,
        streaming,
        whole,
        glide,
        input,
        cursor,
        window_drag,
        ime_cursor,
        press_scale,
        scale: static_scale,
        time_scale,
        help,
        help_cut,
        material,
        aspect_fit,
        selection_group,
        selection_copy,
        on_selection_change,
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
    nodes.push(Node {
        on_paste,
        on_window_focus,
        focus,
        modal,
        looping,
        id: NodeId(0),
        path,
        key,
        trace_name,
        trace_namespace,
        role,
        label,
        vis,
        live,
        text,
        text_layout: None,
        elided: None,
        run_layout: None,
        live_text_seen: None,
        on_press,
        on_double_press,
        on_secondary_press,
        on_hover,
        on_hover_change,
        hover_delay,
        on_pointer_move,
        gestures,
        above,
        on_drag,
        drag_out,
        drag_threshold,
        on_drag_start,
        on_drag_end,
        drop,
        motion,
        opacity_motion,
        lead,
        width_leads,
        instant_changes,
        transition,
        matched: matched.map(|id| (id, own_motion)),
        scroll,
        scroll_axes,
        on_at_bottom,
        on_far_from_bottom,
        on_visible_rows,
        on_layout,
        on_screen_layout,
        on_drawn_layout,
        on_scroll_offset,
        sticky,
        scroll_edges,
        growth,
        streaming,
        whole,
        glide,
        input,
        cursor,
        window_drag,
        ime_cursor,
        selection_group,
        selection_copy,
        on_selection_change,
        a11y,
        inert,
        press_scale,
        static_scale,
        scale_track: false,
        group_fade: false,
        time_scale,
        pixel_aspect,
        aspect_fit,
        material,
        in_row,
        virtual_mount,
        parent,
        taffy: taffy::NodeId::from(0u64),
        target: [0.0; 4],
        local: MRect::ZERO,
        inset: [0.0; 4],
        help,
        help_cut,
        moved_stub: false,
        #[cfg(debug_assertions)]
        scroll_source,
    });
    let child_in_row = match in_row {
        RowLayout::Free if parent_is_stack => if animate_layout { RowLayout::Horizontal } else { RowLayout::Snaps },
        r => r,
    };
    let row = matches!(style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
    let mut kids = Vec::with_capacity(total_children);
    let shared_len = memo_children.as_ref().map_or(0, |m| m.children.len());
    if let Some(shared) = &memo_children {
        for (i, c) in shared.children.iter().enumerate() {
            kids.push(flatten_ref(c, Some(me), path_buf, i, motion, child_in_row, row, child_sx, taffy, nodes, entries, scale, memo_ctx));
        }
    }
    for (j, c) in children.into_iter().enumerate() {
        kids.push(flatten_owned(c, Some(me), path_buf, shared_len + j, motion, child_in_row, row, child_sx, taffy, nodes, entries, scale, memo_ctx));
    }
    path_buf.truncate(prev_len);
    let id = sync_taffy_node(me, Cow::Owned(style), &kids, text_meta, taffy, nodes, entries);
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
    nodes.push(Node {
        on_paste: el.on_paste.clone(),
        on_window_focus: el.on_window_focus.clone(),
        focus: el.focus.clone(),
        modal: el.modal.clone(),
        looping: el.looping.clone(),
        id: NodeId(0),
        path,
        key: el.key.clone(),
        trace_name: el.trace_name,
        trace_namespace,
        role: el.role,
        label: el.label.clone(),
        vis,
        live: el.live.clone(),
        text,
        text_layout: None,
        elided: None,
        run_layout: None,
        live_text_seen: None,
        on_press: el.on_press.clone(),
        on_double_press: el.on_double_press.clone(),
        on_secondary_press: el.on_secondary_press.clone(),
        on_hover: el.on_hover.clone(),
        on_hover_change: el.on_hover_change.clone(),
        hover_delay: el.hover_delay,
        on_pointer_move: el.on_pointer_move.clone(),
        gestures: el.gestures.clone(),
        above: el.above.clone(),
        on_drag: el.on_drag.clone(),
        drag_out: el.drag_out.clone(),
        drag_threshold: el.drag_threshold,
        on_drag_start: el.on_drag_start.clone(),
        on_drag_end: el.on_drag_end.clone(),
        drop: el.drop.clone(),
        motion,
        opacity_motion: el.opacity_motion.clone(),
        lead: el.lead,
        width_leads: el.width_leads,
        instant_changes,
        transition: el.transition.clone(),
        matched: el.matched.as_ref().map(|id| (id.clone(), own_motion)),
        scroll: el.scroll,
        scroll_axes: el.scroll_axes,
        on_at_bottom: el.on_at_bottom.clone(),
        on_far_from_bottom: el.on_far_from_bottom.clone(),
        on_visible_rows: el.on_visible_rows.clone(),
        on_layout: el.on_layout.clone(),
        on_screen_layout: el.on_screen_layout.clone(),
        on_drawn_layout: el.on_drawn_layout.clone(),
        on_scroll_offset: el.on_scroll_offset.clone(),
        sticky: el.sticky.clone(),
        scroll_edges: el.scroll_edges,
        growth: el.growth.clone(),
        streaming: el.streaming,
        whole: el.whole,
        glide: el.glide,
        input,
        cursor: el.cursor.clone(),
        window_drag: el.window_drag,
        ime_cursor: el.ime_cursor,
        selection_group: el.selection_group,
        selection_copy: el.selection_copy.clone(),
        on_selection_change: el.on_selection_change.clone(),
        a11y: el.a11y,
        inert,
        press_scale: el.press_scale,
        static_scale: el.scale,
        scale_track: false,
        group_fade: false,
        time_scale: el.time_scale,
        pixel_aspect,
        aspect_fit,
        material: el.material,
        in_row,
        virtual_mount: el.virtual_mount,
        parent,
        taffy: taffy::NodeId::from(0u64),
        target: [0.0; 4],
        local: MRect::ZERO,
        inset: [0.0; 4],
        help: el.help.clone(),
        help_cut: el.help_cut,
        moved_stub: false,
        #[cfg(debug_assertions)]
        scroll_source: el.scroll_source,
    });
    let child_in_row = match in_row {
        RowLayout::Free if parent_is_stack => if animate_layout { RowLayout::Horizontal } else { RowLayout::Snaps },
        r => r,
    };
    let row = matches!(style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
    let mut kids = Vec::with_capacity(total_children);
    for (i, c) in el.all_children().enumerate() {
        kids.push(flatten_ref(c, Some(me), path_buf, i, motion, child_in_row, row, child_sx, taffy, nodes, entries, scale, memo_ctx));
    }
    path_buf.truncate(prev_len);
    let id = sync_taffy_node(me, style, &kids, text_meta, taffy, nodes, entries);
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
