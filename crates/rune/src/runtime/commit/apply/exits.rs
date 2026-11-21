//! Live-binding handoff and retiring/concealing removed nodes during `RuntimeCore::apply`.
use super::*;

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
    flying: &[(NodeId, MRect)],
    text: &mut TextSystem,
    scale: f32,
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
        if parent_remained_present {
            let instant = instant || (old.instant_changes && old_parent_id.and_then(|pid| node_index.get(&pid)).is_some_and(|&p| nodes[p].instant_changes));
            let rides_parent = old_parent_id.is_some_and(|p| exit_parents.contains(&p));
            let is_exiting = matches!(scene.phase(old.id), Some(PresencePhase::Exiting { .. }));
            if tx.commit.lands() || ((instant || tx.unprompted) && !rides_parent && !is_exiting) || drew_nothing.get(oi).copied().unwrap_or(false) {
                scene.unmount(old.id);
            } else {
                if let Some(t) = old_parent_id.and_then(|pid| node_index.get(&pid)).and_then(|&p| nodes[p].vis.child_exit.as_deref().copied()) {
                    scene.set_transition(old.id, t);
                }
                scene.remove(old.id);
            }
        } else if !tx.commit.lands() && flying.iter().any(|f| f.0 == old.id) {
            // A flight paints outside its ancestors, so it can't fade with an exiting one.
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
            if spec.truncate && layout.full_width() > w * scale + 0.5 {
                let (cut, e) = truncated(text, spec, scale, w * scale);
                (layout, elided) = (cut, Some(e));
            }
            old.text_layout = Some(TextLayout::Single(layout));
            old.elided = elided;
        }
        // A resize that laid the tree out again took the text layouts before the view ran.
        if old.text_layout.is_none() { relayout::lay_out_text_in_place(text, &mut old, scale); }
        node_index.insert(old.id, nodes.len());
        nodes.push(old);
    }
}
