//! Live-binding handoff and retiring/concealing removed nodes during `RuntimeCore::apply`.
use super::*;

#[derive(Clone, Copy)]
struct FlexCollapse {
    horizontal: bool,
    reverse: bool,
    virtual_before: f32,
}

#[derive(Clone, Copy)]
struct SurvivingSibling {
    old_idx: usize,
    new_idx: usize,
    old_start: f32,
    old_end: f32,
    new_start: f32,
    new_end: f32,
}

#[derive(Clone, Copy)]
struct CollapsePlan {
    local: MRect,
    scene: MRect,
    anim: LayoutAnim,
    retarget: bool,
}

#[inline]
fn axis_edges(r: MRect, horizontal: bool) -> (f32, f32) {
    if horizontal { (r.x, r.x + r.w) } else { (r.y, r.y + r.h) }
}

fn collapse_container(
    parent_idx: usize,
    nodes: &[Node],
    taffy: &LayoutEngine,
    virtuals: &HashMap<String, crate::el::virtual_list::VirtualList>,
) -> Option<FlexCollapse> {
    let parent = &nodes[parent_idx];
    if parent.growth.is_some() {
        return None;
    }
    let style = taffy.style(parent.taffy);
    if style.display() != Display::Flex || style.flex_wrap() != FlexWrap::NoWrap {
        return None;
    }
    let dir = style.flex_direction();
    let horizontal = matches!(dir, FlexDirection::Row | FlexDirection::RowReverse);
    let reverse = match dir {
        FlexDirection::Row => parent.flow == crate::el::Flow::Rtl,
        FlexDirection::RowReverse => parent.flow == crate::el::Flow::Ltr,
        FlexDirection::Column => false,
        FlexDirection::ColumnReverse => true,
    };
    let virtual_before = if virtuals.is_empty() {
        0.0
    } else {
        virtuals.get(&parent.path).map_or(0.0, |v| v.before)
    };
    Some(FlexCollapse { horizontal, reverse, virtual_before })
}

#[allow(clippy::too_many_arguments)]
fn plan_hold_layout_collapses(
    node_index: &NodeIndex,
    scene: &Scene,
    taffy: &LayoutEngine,
    virtuals: &HashMap<String, crate::el::virtual_list::VirtualList>,
    flying: &[(NodeId, MRect)],
    reduced_motion: bool,
    nodes: &[Node],
    old_nodes: &[Node],
    old_ids: &[NodeId],
    present_len: usize,
    tx: &Tx,
    instant: bool,
    drew_nothing: &[bool],
    exit_parents: &HashSet<NodeId>,
) -> HashMap<usize, CollapsePlan> {
    let mut plans = HashMap::default();
    if tx.commit.lands() || reduced_motion {
        return plans;
    }
    let mut siblings_by_parent: HashMap<usize, Vec<SurvivingSibling>> = HashMap::default();
    for (oi, old) in old_nodes.iter().enumerate() {
        if old.is_moved_stub() || !old.in_flow || node_index.contains_key(&old.id) || !scene.contains(old.id) {
            continue;
        }
        if !flying.is_empty() && flying.iter().any(|f| f.0 == old.id) {
            continue;
        }
        let Some(old_parent_idx) = old.parent else { continue };
        let Some(&old_parent_id) = old_ids.get(old_parent_idx) else { continue };
        let Some(&parent_idx) = node_index.get(&old_parent_id).filter(|&&p| p < present_len) else { continue };
        let Some(flex) = collapse_container(parent_idx, nodes, taffy, virtuals) else { continue };
        let parent_now_instant = nodes[parent_idx].instant_changes;
        let kept_its_exit = old.animated_changes && !old.instant_changes;
        let node_instant = instant || (parent_now_instant && !kept_its_exit);
        let rides_parent = exit_parents.contains(&old_parent_id);
        let is_exiting = matches!(scene.phase(old.id), Some(PresencePhase::Exiting { .. }));
        if ((node_instant || tx.unprompted) && !rides_parent && !is_exiting)
            || drew_nothing.get(oi).copied().unwrap_or(false)
        {
            continue;
        }
        let holds = if is_exiting {
            old.collapsing
        } else if node_instant || tx.unprompted {
            false
        } else {
            let tr = nodes[parent_idx]
                .vis
                .child_exit
                .as_deref()
                .copied()
                .or_else(|| old.transition.as_deref().copied())
                .unwrap_or_else(Transition::opacity);
            !tr.pop_layout
        };
        if !holds {
            continue;
        }

        let siblings = siblings_by_parent.entry(old_parent_idx).or_insert_with(|| {
            let mut out = Vec::new();
            for (k, s) in old_nodes.iter().enumerate() {
                if s.parent != Some(old_parent_idx) || !s.in_flow {
                    continue;
                }
                let Some(&new_idx) = node_index.get(&s.id).filter(|&&idx| idx < present_len) else { continue };
                if !nodes[new_idx].in_flow || nodes[new_idx].parent != Some(parent_idx) {
                    continue;
                }
                let (old_start, old_end) = axis_edges(s.local, flex.horizontal);
                let (new_start, new_end) = axis_edges(nodes[new_idx].local, flex.horizontal);
                out.push(SurvivingSibling { old_idx: k, new_idx, old_start, old_end, new_start, new_end });
            }
            out
        });

        let (o_start, o_end) = axis_edges(old.local, flex.horizontal);
        let o_mid = 0.5 * (o_start + o_end);
        let mut lo: Option<SurvivingSibling> = None;
        let mut hi: Option<SurvivingSibling> = None;
        for &sib in siblings.iter() {
            let s_mid = 0.5 * (sib.old_start + sib.old_end);
            let is_lo = if (s_mid - o_mid).abs() > 1e-4 {
                s_mid < o_mid
            } else if !is_exiting {
                if flex.reverse { sib.old_idx > oi } else { sib.old_idx < oi }
            } else {
                sib.old_end <= o_start
            };
            if is_lo {
                if lo.is_none_or(|b| {
                    sib.old_end > b.old_end
                        || (sib.old_end == b.old_end
                            && (sib.old_start > b.old_start
                                || (sib.old_start == b.old_start && sib.old_idx.abs_diff(oi) < b.old_idx.abs_diff(oi))))
                }) {
                    lo = Some(sib);
                }
            } else if hi.is_none_or(|b| {
                sib.old_start < b.old_start
                    || (sib.old_start == b.old_start
                        && (sib.old_end < b.old_end
                            || (sib.old_end == b.old_end && sib.old_idx.abs_diff(oi) < b.old_idx.abs_diff(oi))))
            }) {
                hi = Some(sib);
            }
        }

        let seam = match (lo, hi) {
            (Some(l), Some(h)) => 0.5 * (l.new_end + h.new_start),
            (None, Some(h)) => h.new_start,
            (Some(l), None) => l.new_end,
            (None, None) => if flex.reverse { o_end } else { o_start },
        };
        let local = if flex.horizontal {
            MRect::new(seam, old.local.y, 0.0, old.local.h)
        } else {
            MRect::new(old.local.x, seam, old.local.w, 0.0)
        };
        let mut scene_rect = local;
        if flex.virtual_before != 0.0 {
            scene_rect.y -= flex.virtual_before;
        }
        let prev_target = scene.store().target(old.id, prop::FRAME).or_else(|| scene.get(old.id, prop::FRAME));
        let retarget = !is_exiting || prev_target != Some(scene_rect);
        let anim = if is_exiting && (node_instant || tx.relayout || tx.window_step) {
            LayoutAnim::INSTANT
        } else {
            let sib = hi.or(lo).map(|s| &nodes[s.new_idx]);
            if let Some(&anim) = sib.and_then(|s| s.frame_motion.as_deref()) {
                LayoutAnim::new(anim)
            } else {
                let anim_l = tx.motion.map_or_else(
                    || sib.map_or(old.motion, |s| s.motion).into(),
                    MotionSource::anim,
                );
                let mut la = LayoutAnim::new(anim_l);
                if tx.motion.is_none() {
                    if let Some((lead, trail)) = sib.and_then(|s| s.lead).or(old.lead) {
                        la = la.lead(lead, trail);
                    } else if sib.map_or(old.width_leads, |s| s.width_leads) {
                        la = la.width_leads();
                    }
                }
                la
            }
        };
        plans.insert(oi, CollapsePlan { local, scene: scene_rect, anim, retarget });
    }
    plans
}

// Kept inlined into `RuntimeCore::apply` so the common path with no live bindings stays a
// tight scan without an extra call frame.
#[inline]
pub(super) fn carry_live_bindings(
    node_index: &NodeIndex,
    scene: &mut Scene,
    nodes: &mut [Node],
    old_nodes: &[Node],
    tx: &Tx,
) {
    // A node reused from the last commit keeps its bindings' latches, which a commit that
    // lands drops as a rebuilt node's are (below).
    if tx.commit.lands() {
        for n in nodes.iter_mut().filter(|n| n.cold.get().is_some_and(|c| !c.live.is_empty())) {
            if let Some(cold) = n.cold.get_mut_if_some() { cold.live.iter_mut().for_each(|b| b.seen = None); }
        }
    }
    for old in old_nodes {
        if old.cold.get().is_none_or(|c| c.live.is_empty()) { continue; }
        if let Some(&i) = node_index.get(&old.id) {
            let mut kept = paint_tracks::LiveColors::default();
            if let Some(cold) = nodes[i].cold.get_mut_if_some() {
                for bound in &mut cold.live {
                    if let Some(previous) = old.live.iter().find(|b| b.property == bound.property && Arc::ptr_eq(&b.source, &bound.source)) {
                        bound.seen = if tx.commit.lands() { None } else { previous.seen };
                        kept.add(bound.property);
                    }
                }
            }
            // A kept colour binding keeps the colour it last latched until it publishes again.
            kept.carry(old, &mut nodes[i]);
            // Removing a binding restores the normal property, including transform defaults.
            for previous in &old.live {
                if !nodes[i].live.iter().any(|b| b.property == previous.property) {
                    match previous.property {
                        crate::live::Property::Offset => scene.set(old.id, prop::OFFSET, rune_motion::Vec2::ZERO),
                        crate::live::Property::Scale => scene.set(old.id, prop::SCALE, 1.0),
                        crate::live::Property::Trim => { scene.store_mut().remove(old.id, P_TRIM); }
                        _ => {}
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn retire_removed_nodes(
    node_index: &mut NodeIndex,
    scene: &mut Scene,
    taffy: &LayoutEngine,
    virtuals: &HashMap<String, crate::el::virtual_list::VirtualList>,
    flying: &[(NodeId, MRect)],
    text: &mut TextSystem,
    scale: f32,
    reduced_motion: bool,
    nodes: &mut Vec<Node>,
    old_nodes: &mut Vec<Node>,
    present_len: usize,
    tx: &Tx,
    instant: bool,
    drew_nothing: &[bool],
    exit_parents: &HashSet<NodeId>,
    scratch: &mut apply_scratch::ApplyScratch,
    pruned: &mut Vec<Node>,
) {
    scratch.old_ids.clear();
    scratch.old_ids.extend(old_nodes.iter().map(|n| n.id));
    let old_ids = &scratch.old_ids;
    let collapses = plan_hold_layout_collapses(
        node_index, scene, taffy, virtuals, flying, reduced_motion,
        nodes, old_nodes, old_ids, present_len, tx, instant, drew_nothing, exit_parents,
    );
    // Only the nodes that left are moved out; the rest drop where they are.
    for (oi, mut old) in old_nodes.drain(..).enumerate() {
        if old.is_moved_stub() {
            std::mem::forget(old);
            continue;
        }
        if node_index.contains_key(&old.id) {
            continue;
        }
        if !scene.contains(old.id) {
            pruned.push(old);
            continue;
        }
        let old_parent_id = old.parent.and_then(|p| old_ids.get(p).copied());
        let parent_remained_present =
            old_parent_id.is_none_or(|pid| node_index.get(&pid).is_some_and(|&i| i < present_len));
        let mut collapse = None;
        if parent_remained_present {
            let parent_now_instant = old_parent_id.and_then(|pid| node_index.get(&pid)).is_some_and(|&p| nodes[p].instant_changes);
            let kept_its_exit = old.animated_changes && !old.instant_changes;
            let instant = instant || (parent_now_instant && !kept_its_exit);
            let rides_parent = old_parent_id.is_some_and(|p| exit_parents.contains(&p));
            let is_exiting = matches!(scene.phase(old.id), Some(PresencePhase::Exiting { .. }));
            if tx.commit.lands() || ((instant || tx.unprompted) && !rides_parent && !is_exiting) || drew_nothing.get(oi).copied().unwrap_or(false) {
                scene.unmount(old.id);
            } else {
                if let Some(t) = old_parent_id.and_then(|pid| node_index.get(&pid)).and_then(|&p| nodes[p].vis.child_exit.as_deref().copied()) {
                    scene.set_transition(old.id, t);
                    if !is_exiting {
                        scene.clear_transition_motion(old.id);
                    }
                }
                scene.remove(old.id);
                if let Some(plan) = collapses.get(&oi).copied() && scene.contains(old.id) {
                    if plan.retarget {
                        scene.set_frame(old.id, plan.scene, plan.anim);
                    }
                    collapse = Some(plan.local);
                }
            }
        } else if !tx.commit.lands() && !old.vis.flight_in_place && flying.iter().any(|f| f.0 == old.id) {
            // A lifted flight paints outside its ancestors, so it can't fade with an exiting one.
            scene.remove(old.id);
        }
        old.parent = old_parent_id.and_then(|pid| node_index.get(&pid).copied());
        if let Some(cold) = old.cold.get_mut_if_some() { cold.live.clear(); }
        // Exit visuals outlive their model. Never call an accessor captured by
        // a deleted row while normalizing a later focus change.
        if let Some(input) = &mut old.input {
            let was_revealed = input.edit.masking() == crate::edit::Masking::Shown;
            input.access = None;
            input.key = None;
            input.change = None;
            input.selection_change = None;
            input.on_pill_press = None;
            input.on_pill_secondary = None;
            input.edit.focused = false;
            input.edit.cancel_composition();
            input.edit.conceal();
            if was_revealed && let Some(spec) = &mut old.text {
                spec.replace_content(input.edit.display().0);
                if input.edit.is_hidden_multiline() { spec.wrap = false; }
                let w = (old.local.w - old.inset[0] - old.inset[2]).max(0.0);
                let wrap = spec.wrap.then_some(w * scale);
                old.text_layout = Some(TextLayout::Single(text.layout_for_spec(&spec.as_text_input(scale, false), scale, wrap, line_alignment(spec))));
                old.elided = None;
            }
        } else if let Some(spec) = &mut old.text
            && spec.redacted
            && spec.cold.secure_revealed
        {
            spec.conceal();
            let w = (old.local.w - old.inset[0] - old.inset[2]).max(0.0);
            let wrap = spec.wrap.then_some(w * scale);
            let mut layout = text.layout_for_spec(&spec.as_text_input(scale, false), scale, wrap, line_alignment(spec));
            let mut elided = None;
            if spec.truncate && layout.full_width() > w * scale + 0.5
                && let Some((cut, e)) = truncated(text, spec, scale, w * scale)
            {
                (layout, elided) = (cut, Some(e));
            }
            old.text_layout = Some(TextLayout::Single(layout));
            old.elided = elided;
        }
        // A resize that laid the tree out again took the text layouts before the view ran.
        if old.text_layout.is_none() { relayout::lay_out_text_in_place(text, &mut old, scale); }
        if let Some(collapsed_local) = collapse {
            old.local = collapsed_local;
            old.collapsing = true;
        }
        node_index.insert(old.id, nodes.len());
        nodes.push(old);
    }
}
