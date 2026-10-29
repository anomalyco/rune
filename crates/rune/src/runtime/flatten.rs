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
) -> taffy::NodeId {
    let mut el = el;
    el.expand_memo();
    let path = node_path(parent_path, el.key.as_deref(), index);
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
        style,
        vis,
        live,
        text,
        children,
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
    let mut text = text;
    if let Some(t) = &mut text && t.redacted {
        // Rich links carry URLs and can expose them through hover, copy and tree children.
        use zeroize::Zeroize;
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
    if input.as_ref().is_some_and(|i| i.edit.is_hidden_multiline()) && let Some(t) = &mut text {
        // The fixed mask stays one line even in a field narrower than twelve bullets.
        t.wrap = false;
    } else if input.is_none()
        && let Some(t) = &mut text
        && t.redacted
    {
        t.case = None;
        if !t.secure_revealed {
            t.conceal();
        }
    }
    let inert = inert || parent.is_some_and(|p| nodes[p].inert);
    let mut vis = vis;
    if vis.hover_within.is_none() {
        vis.hover_within = above.as_ref()
            .and_then(|a| nodes.iter().position(|n| n.key.as_ref() == Some(&a.key)))
            .and_then(|j| nodes[j].vis.hover_within.clone())
            .or_else(|| parent.and_then(|p| nodes[p].vis.hover_within.clone()));
    }
    if vis.hover_motion.is_none() {
        vis.hover_motion = vis.hover_within.clone();
    }
    if let Some(t) = &text && t.live.is_some() {
        assert!(input.is_none() && !t.redacted && !t.wrap && t.spans.is_empty() && t.reveal < 0.0 && !t.truncate,
            "live_text requires plain single-line text; use truncate_fade for overflow");
    }
    let mut style = style;
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
    let mut text = text;
    if input.is_none() && let Some(t) = &mut text { t.apply_case(); }
    if let Some(t) = &mut text { t.key_shape(scale); }
    let measured = text.is_some();
    let measure = text.as_ref().map(|t| t.shape.map_or(0, |s| s.1) ^ u64::from(t.wrap) ^ (u64::from(t.truncate || t.truncate_fade) << 1)
        ^ ((t.max_lines as u64) << 2)
        ^ (u64::from(input.as_ref().is_some_and(|i| i.edit.is_revealed()) || t.secure_revealed) << 3)
        ^ (u64::from(t.wrap_anywhere) << 4)
        ^ t.advance_in.as_ref().map_or(0, |(run, at)| run.hash.rotate_left(3) ^ (*at as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)));
    // Secure leaves remeasure without retaining a content-derived dictionary oracle.
    let secure_measure = text.as_ref().is_some_and(|t| t.redacted);
    let measure = if secure_measure { Some(0) } else { measure };
    nodes.push(Node {
        on_paste,
        on_window_focus,
        focus,
        modal,
        looping,
        id: NodeId(0),
        path: path.clone(),
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
        #[cfg(debug_assertions)]
        scroll_source,
    });
    let child_in_row = match in_row {
        RowLayout::Free if parent_is_stack => if animate_layout { RowLayout::Horizontal } else { RowLayout::Snaps },
        r => r,
    };
    let mut children = children;
    if scroll.is_some() && scroll_axes.x() {
        if let [content] = children.as_mut_slice() {
            style.flex_direction = FlexDirection::Row;
            unwrapped_width(&mut content.style, scroll_axes.y());
        } else {
            if !scroll_axes.y() {
                style.flex_direction = FlexDirection::Row;
            }
            for child in &mut children {
                let w_full = child.style.size.width == Dimension::percent(1.0);
                if child.style.size.width.is_auto() || w_full {
                    child.style.size.width = Dimension::max_content();
                    if w_full && child.style.min_size.width.is_auto() {
                        child.style.min_size.width = percent(1.0);
                    }
                }
                child.style.flex_shrink = 0.0;
                if scroll_axes.y() && child.style.align_self.is_none() {
                    child.style.align_self = Some(AlignSelf::FLEX_START);
                }
            }
        }
    }
    let row = matches!(style.flex_direction, FlexDirection::Row | FlexDirection::RowReverse);
    let kids: Vec<taffy::NodeId> = children
        .into_iter()
        .enumerate()
        .map(|(i, c)| flatten(c, Some(me), &path, i, motion, child_in_row, row, taffy, nodes, entries, scale))
        .collect();
    let id = if let Some(entry) = entries.get_mut(&path) {
        let id = entry.id;
        if taffy.style(id).unwrap() != &style {
            taffy.set_style(id, style).unwrap();
        }
        if !taffy.child_ids(id).eq(kids.iter().copied()) {
            taffy.set_children(id, &kids).unwrap();
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
    } else if !kids.is_empty() {
        taffy.new_with_children(style, &kids).unwrap()
    } else if measured {
        taffy.new_leaf_with_context(style, me).unwrap()
    } else {
        taffy.new_leaf(style).unwrap()
    };
    if !entries.contains_key(&path) {
        if measured && taffy.get_node_context(id).is_none() {
            taffy.set_node_context(id, Some(me)).unwrap();
        }
        entries.insert(path, LayoutEntry { id, measure, used: true });
    }
    nodes[me].taffy = id;
    id
}
