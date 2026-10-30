//! Reusing a [`memo`](crate::memo)'s flattened `Node` slice and Taffy `NodeId` subtree across
//! commits when neither the memo's inputs, its root modifiers nor its inherited flatten state
//! changed, rebasing node paths and ids only when its parent path moved.
use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct InheritedFlatten {
    pub(super) motion: Spring,
    pub(super) in_row: RowLayout,
    pub(super) parent_is_stack: bool,
    pub(super) instant_changes: bool,
    pub(super) inert: bool,
    pub(super) trace_namespace: Option<(&'static str, &'static str)>,
    pub(super) scale_bits: u32,
}

pub(crate) struct FlattenedMemo {
    pub(super) start: usize,
    pub(super) len: usize,
    pub(super) root_id: NodeId,
    pub(super) last_id: NodeId,
    pub(super) inherited: InheritedFlatten,
    pub(super) hover_within: Option<Rc<(Anim, Anim)>>,
}

pub(crate) struct PendingFlat {
    pub(super) built_id: u64,
    pub(super) start: usize,
    pub(super) len: usize,
    pub(super) inherited: InheritedFlatten,
    pub(super) hover_within: Option<Rc<(Anim, Anim)>>,
}

#[derive(Clone, Copy)]
pub(crate) struct ReusedSpan {
    pub(super) new_start: usize,
    pub(super) old_start: usize,
    pub(super) len: usize,
    pub(super) same_path: bool,
    pub(super) moved: bool,
}

/// A contiguous sub-range of `nodes` (`new_start .. new_start + len`, from `old_start`) whose
/// per-node `apply` diff is a guaranteed no-op.
#[derive(Clone, Copy)]
pub(crate) struct CleanApplySpan {
    pub(super) new_start: usize,
    pub(super) old_start: usize,
    pub(super) len: usize,
}

pub(super) fn is_in_clean_spans(spans: &[CleanApplySpan], i: usize) -> bool {
    let idx = spans.partition_point(|s| s.new_start <= i);
    idx > 0 && {
        let s = &spans[idx - 1];
        i < s.new_start + s.len
    }
}

impl<S: 'static> Runtime<S> {
    fn node_clean_for_apply(&self, new_n: &Node, old_n: &Node, any_animating: bool) -> bool {
        new_n.id == old_n.id
            && new_n.local == old_n.local
            && (!any_animating || !self.scene.store().is_node_animating(new_n.id))
            && self.scene.phase(new_n.id) == Some(PresencePhase::Present)
            && !old_n.group_fade
            && new_n.matched.is_none()
            && new_n.looping.is_none()
            && (self.flying.is_empty() || !self.flying.iter().any(|f| f.0 == new_n.id))
            && (self.aspect_snap.is_empty() || !self.aspect_snap.contains(&new_n.id))
            && new_n.text.as_ref().is_none_or(|t| t.reveal < 0.0)
    }

    /// Fills `out` with the reused memo slices that can skip `apply`'s per-node diff loop.
    pub(super) fn clean_apply_spans(
        &self,
        nodes: &[Node],
        reused: &[ReusedSpan],
        can_skip: bool,
        out: &mut Vec<CleanApplySpan>,
    ) {
        out.clear();
        if !can_skip || reused.is_empty() {
            return;
        }
        out.reserve(reused.len());
        let any_animating = self.scene.store().active_len() > 0;
        for span in reused {
            if !span.same_path || span.len == 0 || span.old_start + span.len > self.nodes.len() || span.new_start + span.len > nodes.len() {
                continue;
            }
            let r_new = &nodes[span.new_start];
            let r_old = &self.nodes[span.old_start];
            if !self.scene.contains(r_new.id) || self.is_exiting(r_new.id) {
                continue;
            }
            let mut scene_rect = r_new.local;
            if let Some(v) = r_new.parent.and_then(|p| self.virtuals.get(&nodes[p].path)) {
                scene_rect.y -= v.before;
            }
            let parent_top_clean = r_new.in_row != RowLayout::Snaps
                || r_new.parent.is_none_or(|p| {
                    nodes.get(p + 1).is_none_or(|first_sib| {
                        let mut sr = first_sib.local;
                        if let Some(v) = self.virtuals.get(&nodes[p].path) {
                            sr.y -= v.before;
                        }
                        self.scene.get(first_sib.id, prop::FRAME) == Some(sr)
                    })
                });
            let root_clean = self.node_clean_for_apply(r_new, r_old, any_animating)
                && parent_top_clean
                && r_new.parent.map(|p| nodes[p].id) == r_old.parent.map(|p| self.nodes[p].id)
                && !self.virtuals.contains_key(&r_new.path)
                && self.scene.get(r_new.id, prop::FRAME) == Some(scene_rect)
                && self.reorder_jump(nodes, span.new_start) == 0.0
                && !r_new.parent.and_then(|p| self.virtuals.get(&nodes[p].path)).is_some_and(crate::virtual_list::VirtualList::replaced_view);

            if !root_clean && !self.flying.is_empty() {
                continue;
            }
            let offset = if root_clean { 0 } else { 1 };
            if offset >= span.len {
                continue;
            }
            let interior_clean = (offset..span.len).all(|k| {
                self.node_clean_for_apply(&nodes[span.new_start + k], &self.nodes[span.old_start + k], any_animating)
            });
            if interior_clean {
                out.push(CleanApplySpan {
                    new_start: span.new_start + offset,
                    old_start: span.old_start + offset,
                    len: span.len - offset,
                });
            }
        }
    }
}

pub(super) struct MemoFlattenCtx<'a> {
    pub(super) old_nodes: &'a mut [Node],
    pub(super) old_present_len: usize,
    pub(super) committing: bool,
    pub(super) committed: &'a HashMap<u64, FlattenedMemo>,
    pub(super) pending: Vec<PendingFlat>,
    pub(super) reused: Vec<ReusedSpan>,
}

fn opt_rc_eq<T: ?Sized>(a: &Option<Rc<T>>, b: &Option<Rc<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

fn opt_box_eq<T: PartialEq>(a: &Option<Box<T>>, b: &Option<Box<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => **x == **y,
        _ => false,
    }
}

fn same_visual(a: &Visual, b: &Visual) -> bool {
    let Visual {
        hit,
        surface,
        fill,
        radius,
        continuous_corners,
        border_width,
        border,
        border_bottom,
        border_dash,
        border_dash_phase,
        shadows,
        opacity,
        blur,
        blur_motion,
        paint_alpha,
        blend,
        hover,
        press,
        clip,
        clip_radius,
        clip_corners,
        icon,
        gradient,
        radial,
        outline,
        shape,
        pixels,
        pixels_fit,
        mark,
        effect,
        backdrop_blur,
        hover_group,
        hover_motion,
        hover_within,
        group_opacity,
        group_color,
        icon_turn,
        cache,
        cache_key,
        offset,
        offset_motion,
        raise,
        no_pixel_snap,
        scale_motion,
        origin,
        rotate,
        rotate_motion,
        child_exit,
        ride_trailing,
    } = b;
    a.hit == *hit
        && match (&a.surface, surface) {
            (None, None) => true,
            (Some((sa, fa)), Some((sb, fb))) => sa.key() == sb.key() && fa == fb,
            _ => false,
        }
        && a.fill == *fill
        && a.radius == *radius
        && a.continuous_corners == *continuous_corners
        && a.border_width == *border_width
        && a.border == *border
        && a.border_bottom == *border_bottom
        && a.border_dash == *border_dash
        && a.border_dash_phase == *border_dash_phase
        && a.shadows == *shadows
        && a.opacity == *opacity
        && a.blur == *blur
        && opt_box_eq(&a.blur_motion, blur_motion)
        && a.paint_alpha == *paint_alpha
        && a.blend == *blend
        && a.hover == *hover
        && a.press == *press
        && a.clip == *clip
        && a.clip_radius == *clip_radius
        && a.clip_corners == *clip_corners
        && match (&a.icon, icon) {
            (None, None) => true,
            (Some((ia, ca)), Some((ib, cb))) => std::ptr::eq(*ia, *ib) && ca == cb,
            _ => false,
        }
        && opt_box_eq(&a.gradient, gradient)
        && opt_box_eq(&a.radial, radial)
        && a.outline == *outline
        && opt_rc_eq(&a.shape, shape)
        && match (&a.pixels, pixels) {
            (None, None) => true,
            (Some(pa), Some(pb)) => Arc::ptr_eq(&pa.source, &pb.source),
            _ => false,
        }
        && a.pixels_fit == *pixels_fit
        && a.mark == *mark
        && a.effect == *effect
        && a.backdrop_blur == *backdrop_blur
        && a.hover_group == *hover_group
        && opt_rc_eq(&a.hover_motion, hover_motion)
        && opt_rc_eq(&a.hover_within, hover_within)
        && a.group_opacity == *group_opacity
        && a.group_color == *group_color
        && a.icon_turn == *icon_turn
        && a.cache == *cache
        && a.cache_key == *cache_key
        && a.offset == *offset
        && opt_box_eq(&a.offset_motion, offset_motion)
        && a.raise == *raise
        && a.no_pixel_snap == *no_pixel_snap
        && opt_box_eq(&a.scale_motion, scale_motion)
        && a.origin == *origin
        && a.rotate == *rotate
        && opt_box_eq(&a.rotate_motion, rotate_motion)
        && opt_box_eq(&a.child_exit, child_exit)
        && a.ride_trailing == *ride_trailing
}

fn same_gestures(
    a: &Option<Box<crate::canvas_input::CanvasInput>>,
    b: &Option<Box<crate::canvas_input::CanvasInput>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.buttons == y.buttons
                && opt_rc_eq(&x.wheel, &y.wheel)
                && opt_rc_eq(&x.magnify, &y.magnify)
                && opt_rc_eq(&x.rotate, &y.rotate)
                && opt_rc_eq(&x.smart_magnify, &y.smart_magnify)
        }
        _ => false,
    }
}

fn same_drop(
    a: &Option<Box<crate::el::DropSpec>>,
    b: &Option<Box<crate::el::DropSpec>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            opt_rc_eq(&x.on_drop, &y.on_drop)
                && opt_rc_eq(&x.accepts, &y.accepts)
                && opt_rc_eq(&x.on_hover, &y.on_hover)
                && opt_rc_eq(&x.on_reject, &y.on_reject)
                && opt_rc_eq(&x.on_reject_hover, &y.on_reject_hover)
                && opt_rc_eq(&x.on_move, &y.on_move)
        }
        _ => false,
    }
}

/// Exhaustively checks that `el` has not been mutated by builders after `Built::placeholder`
/// returned it (apart from `key` and `virtual_mount`, which flattening handles explicitly).
pub(super) fn same_root_el(el: &El, built: &crate::memo::Built) -> bool {
    let base = &built.root;
    let El {
        on_paste,
        on_window_focus,
        focus,
        modal,
        looping,
        key: _,
        trace_name,
        trace_namespace,
        #[cfg(debug_assertions)]
        key_source: _,
        #[cfg(debug_assertions)]
        scroll_source: _,
        #[cfg(debug_assertions)]
        virtual_source: _,
        role,
        label,
        style,
        vis,
        live,
        text,
        children: _,
        memo_children: _,
        memo_built: _,
        has_virtual: _,
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
        animate_layout,
        instant_changes,
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
        virtual_estimate,
        virtual_rows,
        virtual_rows_version,
        virtual_mount: _,
        streaming,
        whole,
        glide,
        input,
        cursor,
        window_drag,
        ime_cursor,
        press_scale,
        scale,
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
    } = base;

    el.children.is_empty()
        && el.memo_children.as_ref().is_some_and(|mc| Rc::ptr_eq(mc, &built.children))
        && el.role == *role
        && el.label == *label
        && el.trace_name == *trace_name
        && el.trace_namespace == *trace_namespace
        && el.style == *style
        && same_visual(&el.vis, vis)
        && el.live.is_empty()
        && live.is_empty()
        && match (&el.text, text) {
            (None, None) => true,
            (Some(ta), Some(tb)) => crate::cold::SharedBox::ptr_eq(ta, tb),
            _ => false,
        }
        && crate::cold::Cold::ptr_eq(&el.focus, focus)
        && el.modal.is_none()
        && modal.is_none()
        && opt_box_eq(&el.looping, looping)
        && match (&el.on_paste, on_paste) {
            (None, None) => true,
            (Some(pa), Some(pb)) => pa.rich == pb.rich && Rc::ptr_eq(&pa.call, &pb.call),
            _ => false,
        }
        && opt_rc_eq(&el.on_window_focus, on_window_focus)
        && opt_rc_eq(&el.on_press, on_press)
        && opt_rc_eq(&el.on_double_press, on_double_press)
        && opt_rc_eq(&el.on_secondary_press, on_secondary_press)
        && opt_rc_eq(&el.on_hover, on_hover)
        && opt_rc_eq(&el.on_hover_change, on_hover_change)
        && el.hover_delay == *hover_delay
        && opt_rc_eq(&el.on_pointer_move, on_pointer_move)
        && same_gestures(&el.gestures, gestures)
        && el.above == *above
        && opt_rc_eq(&el.on_drag, on_drag)
        && opt_rc_eq(&el.drag_out, drag_out)
        && el.drag_threshold == *drag_threshold
        && opt_rc_eq(&el.on_drag_start, on_drag_start)
        && opt_rc_eq(&el.on_drag_end, on_drag_end)
        && same_drop(&el.drop, drop)
        && el.motion == *motion
        && opt_box_eq(&el.opacity_motion, opacity_motion)
        && el.lead == *lead
        && el.width_leads == *width_leads
        && el.animate_layout == *animate_layout
        && el.instant_changes == *instant_changes
        && opt_box_eq(&el.transition, transition)
        && el.matched == *matched
        && el.scroll == *scroll
        && el.scroll_axes == *scroll_axes
        && opt_rc_eq(&el.on_at_bottom, on_at_bottom)
        && match (&el.on_far_from_bottom, on_far_from_bottom) {
            (None, None) => true,
            (Some((da, fa)), Some((db, fb))) => da == db && Rc::ptr_eq(fa, fb),
            _ => false,
        }
        && opt_rc_eq(&el.on_visible_rows, on_visible_rows)
        && opt_rc_eq(&el.on_layout, on_layout)
        && opt_rc_eq(&el.on_screen_layout, on_screen_layout)
        && opt_rc_eq(&el.on_drawn_layout, on_drawn_layout)
        && opt_rc_eq(&el.on_scroll_offset, on_scroll_offset)
        && opt_box_eq(&el.sticky, sticky)
        && el.scroll_edges == *scroll_edges
        && opt_box_eq(&el.growth, growth)
        && el.virtual_estimate == *virtual_estimate
        && opt_rc_eq(&el.virtual_rows, virtual_rows)
        && el.virtual_rows_version == *virtual_rows_version
        && el.streaming == *streaming
        && el.whole == *whole
        && el.glide == *glide
        && el.input.is_none()
        && input.is_none()
        && el.cursor == *cursor
        && el.window_drag == *window_drag
        && el.ime_cursor == *ime_cursor
        && el.press_scale == *press_scale
        && el.scale == *scale
        && el.time_scale == *time_scale
        && el.help == *help
        && el.help_cut == *help_cut
        && el.material == *material
        && el.aspect_fit == *aspect_fit
        && el.selection_group == *selection_group
        && opt_rc_eq(&el.selection_copy, selection_copy)
        && opt_rc_eq(&el.on_selection_change, on_selection_change)
        && el.a11y == *a11y
        && el.inert == *inert
}

pub(super) fn inherited_for(
    parent: Option<usize>,
    nodes: &[Node],
    motion: Spring,
    in_row: RowLayout,
    scale: f32,
) -> (InheritedFlatten, Option<Rc<(Anim, Anim)>>) {
    let parent_is_stack = parent.is_some_and(|p| nodes[p].growth.is_some());
    let instant_changes = parent.is_some_and(|p| nodes[p].instant_changes);
    let inert = parent.is_some_and(|p| nodes[p].inert);
    let trace_namespace = parent.and_then(|p| nodes[p].trace_namespace);
    let hover_within = parent.and_then(|p| nodes[p].vis.hover_within.clone());
    (
        InheritedFlatten {
            motion,
            in_row,
            parent_is_stack,
            instant_changes,
            inert,
            trace_namespace,
            scale_bits: scale.to_bits(),
        },
        hover_within,
    )
}

fn path_matches(old_path: &str, parent_path: &str, key: Option<&str>, index: usize) -> bool {
    let tail = if parent_path.is_empty() {
        old_path
    } else {
        let Some(rest) = old_path.strip_prefix(parent_path).and_then(|r| r.strip_prefix('/')) else {
            return false;
        };
        rest
    };
    match key {
        Some(k) => tail == k,
        None => tail.strip_prefix('#').is_some_and(|n| !n.contains('/') && n.parse::<usize>().ok() == Some(index)),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn try_reuse_memo(
    el: &El,
    parent: Option<usize>,
    parent_path: &str,
    index: usize,
    motion: Spring,
    in_row: RowLayout,
    scale: f32,
    taffy: &mut TaffyTree<usize>,
    nodes: &mut Vec<Node>,
    entries: &mut HashMap<String, LayoutEntry>,
    ctx: &mut MemoFlattenCtx<'_>,
) -> Option<taffy::NodeId> {
    let built = el.memo_built.as_ref()?;
    if !built.reusable {
        return None;
    }
    let flat = ctx.committed.get(&built.id)?;
    let end = flat.start.checked_add(flat.len)?;
    if flat.len == 0 || end > ctx.old_present_len || end > ctx.old_nodes.len() {
        return None;
    }
    let old_slice = &ctx.old_nodes[flat.start..end];
    if old_slice[0].is_moved_stub() || old_slice[0].id != flat.root_id || old_slice[flat.len - 1].id != flat.last_id {
        return None;
    }
    let (inherited, hover_within) = inherited_for(parent, nodes, motion, in_row, scale);
    if flat.inherited != inherited || !opt_rc_eq(&flat.hover_within, &hover_within) {
        return None;
    }
    if el.key != old_slice[0].key || !same_root_el(el, built) {
        return None;
    }
    if !old_slice.iter().all(|old| !old.is_moved_stub() && entries.get(&old.path).is_some_and(|e| !e.used && e.id == old.taffy)) {
        return None;
    }

    let old_root_path = old_slice[0].path.as_str();
    let same_path = path_matches(old_root_path, parent_path, el.key.as_deref(), index);
    let new_start = nodes.len();
    let moved = same_path && ctx.committing;
    let root_taffy = old_slice[0].taffy;

    if moved {
        for k in 0..flat.len {
            let new_idx = new_start + k;
            let slot = &mut ctx.old_nodes[flat.start + k];
            let entry = entries.get_mut(&slot.path).unwrap();
            entry.used = true;
            if entry.measure.is_some() {
                *taffy.get_node_context_mut(slot.taffy).unwrap() = new_idx;
            }
            let stub = Node::moved_stub(slot);
            let mut n = std::mem::replace(slot, stub);
            n.text_layout = None;
            n.elided = None;
            n.run_layout = None;
            n.scale_track = false;
            n.group_fade = false;
            n.virtual_mount = if k == 0 { el.virtual_mount } else { false };
            n.parent = if k == 0 { parent } else { n.parent.map(|p| new_start + (p - flat.start)) };
            n.target = [0.0; 4];
            n.local = MRect::ZERO;
            n.inset = [0.0; 4];
            nodes.push(n);
        }
    } else if same_path {
        for (k, old) in old_slice.iter().enumerate() {
            let new_idx = new_start + k;
            let entry = entries.get_mut(&old.path).unwrap();
            entry.used = true;
            if entry.measure.is_some() {
                *taffy.get_node_context_mut(old.taffy).unwrap() = new_idx;
            }
            nodes.push(Node {
                id: old.id,
                text_layout: None,
                elided: None,
                run_layout: None,
                scale_track: false,
                group_fade: false,
                virtual_mount: if k == 0 { el.virtual_mount } else { false },
                parent: if k == 0 { parent } else { old.parent.map(|p| new_start + (p - flat.start)) },
                target: [0.0; 4],
                local: MRect::ZERO,
                inset: [0.0; 4],
                ..old.clone()
            });
        }
    } else {
        let new_root_path = node_path(parent_path, el.key.as_deref(), index);
        let mut rebased = Vec::with_capacity(flat.len);
        for (k, old) in old_slice.iter().enumerate() {
            let path = if k == 0 {
                new_root_path.clone()
            } else {
                let tail = &old.path[old_root_path.len()..];
                let mut s = String::with_capacity(new_root_path.len() + tail.len());
                s.push_str(&new_root_path);
                s.push_str(tail);
                s
            };
            if entries.get(&path).is_some_and(|e| e.used) {
                return None;
            }
            rebased.push(path);
        }
        if let Some(old_p) = taffy.parent(old_slice[0].taffy) {
            let _ = taffy.remove_child(old_p, old_slice[0].taffy);
        }
        let moved_entries: Vec<LayoutEntry> = old_slice
            .iter()
            .map(|old| entries.remove(&old.path).unwrap())
            .collect();
        for path in &rebased {
            if let Some(stale) = entries.remove(path) {
                let _ = taffy.set_children(stale.id, &[]);
                taffy.remove(stale.id).unwrap();
            }
        }
        for (k, ((old, path), mut entry)) in old_slice.iter().zip(rebased).zip(moved_entries).enumerate() {
            let new_idx = new_start + k;
            entry.used = true;
            if entry.measure.is_some() {
                *taffy.get_node_context_mut(old.taffy).unwrap() = new_idx;
            }
            entries.insert(path.clone(), entry);
            nodes.push(Node {
                id: NodeId(0),
                path,
                text_layout: None,
                elided: None,
                run_layout: None,
                scale_track: false,
                group_fade: false,
                virtual_mount: if k == 0 { el.virtual_mount } else { false },
                parent: if k == 0 { parent } else { old.parent.map(|p| new_start + (p - flat.start)) },
                target: [0.0; 4],
                local: MRect::ZERO,
                inset: [0.0; 4],
                ..old.clone()
            });
        }
    }

    ctx.pending.push(PendingFlat {
        built_id: built.id,
        start: new_start,
        len: flat.len,
        inherited,
        hover_within,
    });
    ctx.reused.push(ReusedSpan {
        new_start,
        old_start: flat.start,
        len: flat.len,
        same_path,
        moved,
    });
    Some(root_taffy)
}

/// Puts `moved` slices back into `old_nodes` with their pre-pass geometry and flags before a
/// virtual-window retry or a layout-read, focus or caret re-measure runs again.
pub(super) fn restore_moved_nodes(old_nodes: &mut [Node], nodes: &mut [Node], reused: &[ReusedSpan]) {
    for span in reused.iter().filter(|s| s.moved) {
        for k in 0..span.len {
            let stub = &old_nodes[span.old_start + k];
            let n = &mut nodes[span.new_start + k];
            n.parent = stub.parent;
            n.target = stub.target;
            n.local = stub.local;
            n.inset = stub.inset;
            n.scale_track = stub.scale_track;
            n.group_fade = stub.group_fade;
            n.text_layout = None;
            n.elided = None;
            n.run_layout = None;
            std::mem::swap(&mut old_nodes[span.old_start + k], n);
        }
    }
}

impl crate::memo::MemoCache {
    /// Updates flattened subtree spans once `apply` installs `nodes` (`0..present_len`).
    /// `None` when `relayout` reused the previous commit's node array in place.
    pub(crate) fn commit_flattened(
        &mut self,
        nodes: &[Node],
        present_len: usize,
        pending: Option<Vec<PendingFlat>>,
        mut reused: Vec<ReusedSpan>,
    ) {
        self.present_len = present_len;
        let Some(mut pending) = pending else {
            reused.clear();
            self.reused_scratch = reused;
            return;
        };
        // Nested memos inside an outer memo that was reused whole stay live at their rebased index.
        if !self.flattened.is_empty() {
            self.flattened.retain(|_, flat| {
                let Some(span) = reused.iter().find(|s| {
                    flat.start >= s.old_start && flat.start + flat.len <= s.old_start + s.len
                }) else {
                    return false;
                };
                flat.start = span.new_start + (flat.start - span.old_start);
                if !span.same_path {
                    flat.root_id = nodes[flat.start].id;
                    flat.last_id = nodes[flat.start + flat.len - 1].id;
                }
                true
            });
        }
        for p in pending.drain(..) {
            if p.len == 0 || p.start + p.len > present_len {
                continue;
            }
            self.flattened.insert(
                p.built_id,
                FlattenedMemo {
                    start: p.start,
                    len: p.len,
                    root_id: nodes[p.start].id,
                    last_id: nodes[p.start + p.len - 1].id,
                    inherited: p.inherited,
                    hover_within: p.hover_within,
                },
            );
        }
        reused.clear();
        self.pending_scratch = pending;
        self.reused_scratch = reused;
    }

    /// Rebases flattened subtree indices after `prune_exited` compacts `self.nodes`.
    pub(crate) fn on_prune(&mut self, remap: &[Option<usize>]) {
        self.present_len = remap
            .get(..self.present_len.min(remap.len()))
            .map_or(0, |s| s.iter().filter(|r| r.is_some()).count());
        self.flattened.retain(|_, flat| {
            let Some(new_start) = remap.get(flat.start).copied().flatten() else {
                return false;
            };
            let Some(new_last) = remap.get(flat.start + flat.len - 1).copied().flatten() else {
                return false;
            };
            if new_last + 1 != new_start + flat.len {
                return false;
            }
            flat.start = new_start;
            true
        });
    }
}

#[cfg(test)]
#[path = "memo_reuse_tests.rs"]
mod tests;
