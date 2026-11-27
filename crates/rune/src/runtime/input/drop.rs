//! Files dragged onto a window from another app: `El::on_drop_files`, `El::accepts_files`,
//! `El::accepts_drag`, `El::drop_feedback`, `El::on_drop_hover`, `El::on_drop_move`,
//! `El::on_drop_rejected`, `El::on_drop_rejected_hover`, `El::on_drop_hover_within` and
//! `El::on_drop_move_within`.
use super::*;
use crate::drag_out::{DragOperation, DragOperations};
use crate::el::{DropDescription, DropFeedback, DropHoverFn, DropInfo, PointerEvent};
use crate::el::drop::DropMoveFn;
use std::path::PathBuf;
use std::rc::Rc;

/// Modifier keys and source-allowed operations accompanying a file drag step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DragMeta {
    pub mods: crate::Mods,
    pub allowed: DragOperations,
}

impl DragMeta {
    fn copy_default() -> Self { Self { mods: crate::Mods::default(), allowed: DragOperations::COPY } }
}

/// One step of a file drag over the window, queued for the next frame.
pub(crate) enum FileEvent {
    /// The drag entered or moved. A position the platform can't give is `None`.
    Hover(Rc<[PathBuf]>, Option<(f32, f32)>, DragMeta),
    /// The drag moved (or its modifiers / allowed operations changed) over a target that follows
    /// it (`El::on_drop_move`), a passive observer (`El::on_drop_move_within`), or a hit chain
    /// with `El::accepts_drag` / `El::drop_feedback`; runs as a hover at the latest point.
    Move,
    Drop {
        paths: Rc<[PathBuf]>,
        at: Option<(f32, f32)>,
        meta: DragMeta,
        /// Operation already committed to the OS drop source (Windows OLE `*pdwEffect`), or
        /// `None` to compute it from the target during this drop.
        operation: Option<DragOperation>,
        /// Per-target `Tx::set_drop_operation` override captured while hovering before drop.
        prev_override: Option<(String, Option<DragOperation>)>,
    },
    Leave,
}

#[derive(Default)]
pub(crate) struct FileDrag {
    /// The drag over the window (set when its step is queued, before it runs).
    over: Option<Over>,
    /// The drop target notified with `on_drop_hover(true)` (or, rejecting the drag,
    /// `on_drop_rejected_hover(true)`), and that callback, which still hears `false` after the
    /// element unmounts or starts to exit.
    hovered: Option<(Aim, Option<DropHoverFn>)>,
    /// Ancestors notified with `on_drop_hover_within(true)` (innermost first), with each
    /// element's kept callback so it still hears `false` after it unmounts or starts to exit.
    within_hovered: Vec<(String, DropHoverFn)>,
    /// File steps waiting for this frame's build (`hold_files`), and whether those leading the
    /// queue already waited one.
    held: Vec<Queued>,
    waited: bool,
}

/// A file drag over the window.
struct Over {
    /// The files it carries, for `drag_files_to` and `Tx::drag_paths`.
    paths: Rc<[PathBuf]>,
    /// Where it is now, in window points, when the platform says.
    at: Option<(f32, f32)>,
    /// Modifier keys and source-allowed operations.
    meta: DragMeta,
    /// Last position and metadata delivered to `on_drop_move` / `on_drop_move_within`, so a
    /// stationary `accepts_drag` recheck does not repeat move callbacks.
    last_moved_at: Option<(f32, f32)>,
    last_moved_meta: Option<DragMeta>,
    /// The target of the last hover queued or run (`None` while awaiting stateful evaluation on
    /// the frame), so a move that keeps it queues nothing.
    aimed: Option<Option<Aim>>,
    /// The `on_drop_hover_within` element paths of the last hover queued or run.
    within_aimed: Vec<String>,
    /// `commit.commits` after the frame in which `hover_files` last evaluated stateful hooks.
    checked_commit: u64,
    /// A `FileEvent::Move` waits to run.
    move_queued: bool,
    /// Per-target `Tx::set_drop_operation` / `Tx::set_drop_description` overrides; cleared on target change.
    tx_op_override: Option<Option<DragOperation>>,
    tx_desc_override: Option<Option<DropDescription>>,
    /// Effective resolved feedback for the OS drop target (populated once `hover_files` runs).
    feedback: Option<DropFeedback>,
}

/// A drop target's path, and whether it takes any of the dragged files (else it stops the drag
/// to reject it).
type Aim = (String, bool);

/// Where a drag or drop lands: the target, the files it takes and the rest.
struct Target {
    i: usize,
    files: Vec<PathBuf>,
    rejected: Vec<PathBuf>,
}

fn set_tx_drag(tx: &mut Tx, point: Option<PointerEvent>, paths: Option<Rc<[PathBuf]>>, meta: DragMeta, op: Option<DragOperation>) {
    tx.drop_point = point;
    tx.drag_paths = paths;
    tx.drag_mods = meta.mods;
    tx.drag_operations = meta.allowed;
    tx.drop_operation = op;
    tx.drop_op_set = None;
    tx.drop_desc_set = None;
}

fn clear_tx_drag(tx: &mut Tx) {
    set_tx_drag(tx, None, None, DragMeta::default(), None);
}

impl capture::PointerState {
    #[inline]
    pub(in crate::runtime) fn dragging_files(&self) -> bool { self.file_drag.over.is_some() }

    #[inline]
    pub(in crate::runtime) fn files_held(&self) -> bool { self.file_drag.waited }

    /// Drops the drag's target and the hover callback it would hear `false` on, unheard: their
    /// elements went with the state they were built for.
    #[inline]
    pub(in crate::runtime) fn forget_drop_targets(&mut self) {
        if let Some(over) = &mut self.file_drag.over {
            over.aimed = None;
            over.within_aimed.clear();
            over.last_moved_at = None;
            over.last_moved_meta = None;
            over.tx_op_override = None;
            over.tx_desc_override = None;
            over.feedback = None;
        }
        self.file_drag.hovered = None;
        self.file_drag.within_hovered.clear();
    }

    pub(super) fn drop_hover_key<'a>(&self, node: impl Fn(&str) -> Option<&'a Node>, accepted: bool) -> Option<String> {
        let ((path, _), _) = self.file_drag.hovered.as_ref().filter(|((_, a), _)| *a == accepted)?;
        Some(node(path).and_then(|n| n.key.clone()).unwrap_or_else(|| path.clone()))
    }

    /// Holds file steps for this frame's build when the tree changed since its last build.
    pub(in crate::runtime) fn hold_files(&mut self, queue: ActionQueue, stale: bool) -> ActionQueue {
        let aims = |a: &Action| matches!(a, Action::Files(FileEvent::Hover(..) | FileEvent::Move | FileEvent::Drop { .. }));
        // Intermediate timer sub-steps in `frame_split` stash queued input; keep `waited` until
        // the held file steps return to the queue.
        if !queue.iter().any(aims) { return queue; }
        let waited = std::mem::take(&mut self.file_drag.waited);
        if waited || !stale { return queue; }
        let (files, rest): (_, Vec<_>) = queue.into_iter().partition(|q| matches!(q.action, Action::Files(_)));
        self.file_drag.held = files;
        rest.into_iter().collect()
    }

    #[inline]
    pub(super) fn take_held_files(&mut self) -> Option<Vec<Queued>> {
        if self.file_drag.held.is_empty() { return None; }
        self.file_drag.waited = true;
        Some(std::mem::take(&mut self.file_drag.held))
    }
}

impl RuntimeCore {
    /// A file drag from another app entered the window or moved over it, carrying `paths`; `at`
    /// is its position in points, or `None` when the platform doesn't say. The innermost
    /// [`El::on_drop_files`] under it that accepts the files hears `on_drop_hover(true)` next
    /// frame.
    pub fn drag_files(&mut self, paths: Vec<PathBuf>, at: Option<(f32, f32)>) {
        self.drag_files_with(paths, at, crate::Mods::default(), DragOperations::COPY);
    }

    /// [`Self::drag_files`] with explicit drag modifier keys and source-allowed operations.
    pub fn drag_files_with(
        &mut self,
        paths: Vec<PathBuf>,
        at: Option<(f32, f32)>,
        mods: crate::Mods,
        allowed: DragOperations,
    ) {
        let meta = DragMeta { mods, allowed };
        let paths: Rc<[PathBuf]> = paths.into();
        let chain = self.drop_hit_chain(at);
        let stateful = !paths.is_empty() && (self.chain_has_accepts_drag(&chain) || self.chain_has_drop_feedback(&chain));
        let aimed = if stateful {
            None
        } else {
            Some(self.drop_target_in_chain(&chain, None, &paths)
                .map(|t| (self.tree.nodes[t.i].path.clone(), !t.files.is_empty())))
        };
        let within_aimed = self.chain_within_hover_paths(&chain, &paths);
        self.pointer.file_drag.over = Some(Over {
            paths: paths.clone(),
            at,
            meta,
            last_moved_at: None,
            last_moved_meta: None,
            aimed,
            within_aimed,
            checked_commit: self.commit.commits,
            move_queued: false,
            tx_op_override: None,
            tx_desc_override: None,
            feedback: None,
        });
        self.frame.queue.push(Action::Files(FileEvent::Hover(paths, at, meta)));
    }

    /// The file drag is at `at`, with the files it entered with. Queues a step, and returns
    /// true, only when that changes the drop target or `on_drop_hover_within` set, moves over a
    /// target or ancestor that follows it (`El::on_drop_move` / `El::on_drop_move_within`, once
    /// a frame), or moves/commits over a hit chain with `El::accepts_drag` / `El::drop_feedback`.
    pub fn drag_files_to(&mut self, at: Option<(f32, f32)>) -> bool {
        let Some(meta) = self.pointer.file_drag.over.as_ref().map(|o| o.meta) else { return false };
        self.drag_files_to_with(at, meta.mods, meta.allowed)
    }

    /// [`Self::drag_files_to`] with updated modifier keys and source-allowed operations.
    pub fn drag_files_to_with(
        &mut self,
        at: Option<(f32, f32)>,
        mods: crate::Mods,
        allowed: DragOperations,
    ) -> bool {
        let Some(over) = &self.pointer.file_drag.over else { return false };
        let meta = DragMeta { mods, allowed };
        let moved = over.at != at;
        let meta_changed = over.meta != meta;
        let paths = over.paths.clone();
        let chain = self.drop_hit_chain(at);
        let within = self.chain_within_hover_paths(&chain, &paths);
        let within_same = within == over.within_aimed;
        let stateful = !paths.is_empty() && (self.chain_has_accepts_drag(&chain) || self.chain_has_drop_feedback(&chain));
        let follows_within = at.is_some() && self.chain_has_move_within(&chain, &paths);

        if stateful {
            let committed = self.commit.commits != over.checked_commit;
            let Some(over) = &mut self.pointer.file_drag.over else { return false };
            over.at = at;
            over.meta = meta;
            if !within_same {
                over.within_aimed = within;
                self.frame.queue.push(Action::Files(FileEvent::Hover(paths, at, meta)));
                return true;
            }
            if (!moved && !meta_changed && !committed) || std::mem::replace(&mut over.move_queued, true) {
                return false;
            }
            self.frame.queue.push(Action::Files(FileEvent::Move));
            return true;
        }

        let target = self.drop_target_in_chain(&chain, None, &paths)
            .map(|t| (self.tree.nodes[t.i].path.clone(), !t.files.is_empty()));
        let same = over.aimed.as_ref() == Some(&target) && within_same;
        let follows = same && (
            (at.is_some() && target.as_ref().is_some_and(|aim| self.drop_move_fn(aim).is_some()))
            || follows_within
        );
        let Some(over) = &mut self.pointer.file_drag.over else { return false };
        over.at = at;
        over.meta = meta;
        if same {
            if meta_changed || (moved && follows) {
                if std::mem::replace(&mut over.move_queued, true) { return false; }
                self.frame.queue.push(Action::Files(FileEvent::Move));
                return true;
            }
            return false;
        }
        over.aimed = Some(target);
        over.within_aimed = within;
        self.frame.queue.push(Action::Files(FileEvent::Hover(paths, at, meta)));
        true
    }

    /// The file drag left the window, was cancelled, or the window hid.
    pub fn drag_files_leave(&mut self) {
        if self.pointer.file_drag.over.take().is_none() { return; }
        self.frame.queue.push(Action::Files(FileEvent::Leave));
    }

    /// `paths` dropped at `at` (points, or `None` when the platform doesn't say).
    pub fn drop_files(&mut self, paths: Vec<PathBuf>, at: Option<(f32, f32)>) {
        let meta = DragMeta::copy_default();
        self.drop_files_with(paths, at, meta.mods, meta.allowed, None);
    }

    /// [`Self::drop_files`] with modifier keys, source-allowed operations, and an optional
    /// OS-committed [`DragOperation`].
    pub fn drop_files_with(
        &mut self,
        paths: Vec<PathBuf>,
        at: Option<(f32, f32)>,
        mods: crate::Mods,
        allowed: DragOperations,
        operation: Option<DragOperation>,
    ) {
        let prev_override = self.pointer.file_drag.over.take().and_then(|o| {
            let aim = o.aimed.flatten()?;
            o.tx_op_override.map(|op| (aim.0, op))
        });
        self.frame.queue.push(Action::Files(FileEvent::Drop {
            paths: paths.into(),
            at,
            meta: DragMeta { mods, allowed },
            operation,
            prev_override,
        }));
    }

    #[inline]
    pub fn dragging_files(&self) -> bool { self.pointer.dragging_files() }

    pub fn drop_hovered(&self) -> Option<String> { self.pointer.drop_hover_key(|p| self.node(p), true) }

    pub fn drop_rejected_hovered(&self) -> Option<String> { self.pointer.drop_hover_key(|p| self.node(p), false) }

    /// The resolved [`DropFeedback`] (chosen operation and optional Windows drop description) for
    /// the active file drag after its latest frame evaluation, or `None` when no drag is over
    /// the window or its first hover has not yet been evaluated.
    pub fn drop_feedback(&self) -> Option<&DropFeedback> {
        self.pointer.file_drag.over.as_ref().and_then(|o| o.feedback.as_ref())
    }

    #[inline]
    pub(in crate::runtime) fn hold_files(&mut self, queue: ActionQueue, stale: bool) -> ActionQueue {
        self.pointer.hold_files(queue, stale)
    }

    pub(in crate::runtime) fn release_files(&mut self) {
        if let Some(held) = self.pointer.take_held_files() {
            self.frame.queue.prepend(held);
        }
    }

    #[inline]
    pub(in crate::runtime) fn files_held(&self) -> bool { self.pointer.files_held() }

    #[inline]
    pub(in crate::runtime) fn clear_drop_hits(&mut self) { self.hit.clear_drop_hits(); }

    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn has_drop_targets(&self) -> bool {
        (0..self.tree.nodes.len()).any(|i| {
            !self.subtree_exiting(i)
                && self.allowed(i)
                && self.tree.nodes[i].handlers.drop.as_ref().is_some_and(|d| {
                    d.on_drop.is_some() || d.claims_rejects() || d.observes_within()
                })
        })
    }

    fn drop_hit_chain(&self, at: Option<(f32, f32)>) -> Vec<usize> {
        let ok = |id: NodeId| self.tree.node_index.get(&id).copied()
            .filter(|&i| i < self.tree.nodes.len() && !self.subtree_exiting(i) && self.allowed(i));
        let start = match at {
            None => None,
            Some((x, y)) if !self.in_viewport(x, y) => return Vec::new(),
            Some((x, y)) => {
                let top = self.hit.top_drop_or_hit(|id, r| self.hit_contains(id, r, x, y), ok);
                if top.is_none() && self.active_modal().is_some() { return Vec::new(); }
                top
            }
        };
        let root = self.active_modal().unwrap_or_else(|| self.app_root());
        let mut chain = Vec::new();
        let mut cur = start;
        while let Some(i) = cur {
            if self.allowed(i) && !self.subtree_exiting(i) {
                chain.push(i);
            }
            cur = self.tree.nodes[i].parent;
        }
        if root < self.tree.nodes.len()
            && self.allowed(root)
            && !self.subtree_exiting(root)
            && !chain.contains(&root)
        {
            chain.push(root);
        }
        chain
    }

    fn drop_target_in_chain(
        &self,
        chain: &[usize],
        states: Option<crate::states::StatesRef<'_>>,
        paths: &[PathBuf],
    ) -> Option<Target> {
        chain.iter().copied().find_map(|i| {
            let drop = self.tree.nodes[i].handlers.drop.as_ref()?;
            let (files, rejected) = drop.split(states, paths);
            (!files.is_empty() || (!rejected.is_empty() && drop.claims_rejects()))
                .then_some(Target { i, files, rejected })
        })
    }

    fn chain_has_accepts_drag(&self, chain: &[usize]) -> bool {
        chain.iter().any(|&i| self.tree.nodes[i].handlers.drop.as_ref().is_some_and(|d| d.accepts_drag.is_some()))
    }

    fn chain_has_drop_feedback(&self, chain: &[usize]) -> bool {
        chain.iter().any(|&i| self.tree.nodes[i].handlers.drop.as_ref().is_some_and(|d| d.feedback.is_some()))
    }

    fn chain_within_hover_paths(&self, chain: &[usize], paths: &[PathBuf]) -> Vec<String> {
        if paths.is_empty() { return Vec::new(); }
        chain.iter().copied()
            .filter(|&i| self.tree.nodes[i].handlers.drop.as_ref().is_some_and(|d| d.on_hover_within.is_some()))
            .map(|i| self.tree.nodes[i].path.clone())
            .collect()
    }

    fn chain_has_move_within(&self, chain: &[usize], paths: &[PathBuf]) -> bool {
        !paths.is_empty()
            && chain.iter().any(|&i| self.tree.nodes[i].handlers.drop.as_ref().is_some_and(|d| d.on_move_within.is_some()))
    }

    fn drop_hover_fn(&self, (path, accepted): &Aim) -> Option<DropHoverFn> {
        self.node(path).and_then(|n| n.handlers.drop.as_ref()?.hover_fn(*accepted))
    }

    fn drop_hover_within_fn(&self, path: &str) -> Option<DropHoverFn> {
        self.node(path).and_then(|n| n.handlers.drop.as_ref()?.on_hover_within.clone())
    }

    fn drop_move_fn(&self, (path, accepted): &Aim) -> Option<DropMoveFn> {
        self.node(path).filter(|_| *accepted).and_then(|n| n.handlers.drop.as_ref()?.on_move.clone())
    }

    fn drop_point(&self, path: &str, at: Option<(f32, f32)>) -> Option<PointerEvent> {
        let (x, y) = at?;
        let i = self.live_index(path)?;
        Some(self.pointer_event(i, x, y))
    }

    fn evaluate_target_feedback(
        &self,
        states: crate::states::StatesRef<'_>,
        target: Option<&Target>,
        paths: &[PathBuf],
        at: Option<(f32, f32)>,
        meta: DragMeta,
    ) -> DropFeedback {
        let Some(t) = target else { return DropFeedback::none() };
        let Some(drop) = self.tree.nodes[t.i].handlers.drop.as_deref() else { return DropFeedback::none() };
        if let Some(f) = &drop.feedback {
            let point = self.drop_point(&self.tree.nodes[t.i].path, at);
            let info = DropInfo { paths, point, mods: meta.mods, allowed: meta.allowed };
            return f(states, &info).constrain(meta.allowed);
        }
        let accepts_drop = !t.files.is_empty() || (!t.rejected.is_empty() && drop.on_reject.is_some());
        let op = (accepts_drop && meta.allowed.contains(DragOperation::Copy)).then_some(DragOperation::Copy);
        DropFeedback::new(op)
    }

    fn hover_files(
        &mut self,
        app: &mut dyn AppDriver,
        paths: &Rc<[PathBuf]>,
        at: Option<(f32, f32)>,
        meta: DragMeta,
        force_move: bool,
        tx: &mut Tx,
    ) -> bool {
        let chain = self.drop_hit_chain(at);
        let target_info = self.drop_target_in_chain(&chain, Some(app.states_ref()), paths);
        let target = target_info.as_ref().map(|t| (self.tree.nodes[t.i].path.clone(), !t.files.is_empty()));
        let base_feedback = self.evaluate_target_feedback(app.states_ref(), target_info.as_ref(), paths, at, meta);
        let within: Vec<(String, DropHoverFn)> = if paths.is_empty() {
            Vec::new()
        } else {
            chain.iter().copied()
                .filter_map(|i| Some((self.tree.nodes[i].path.clone(), self.tree.nodes[i].handlers.drop.as_ref()?.on_hover_within.clone()?)))
                .collect()
        };
        let within_paths: Vec<String> = within.iter().map(|(p, _)| p.clone()).collect();
        let prev_target = self.pointer.file_drag.hovered.as_ref().map(|(aim, _)| aim.clone());
        let (prev_at, prev_meta) = self.pointer.file_drag.over.as_ref()
            .map(|o| (o.last_moved_at, o.last_moved_meta))
            .unwrap_or((None, None));
        let moved = force_move || prev_at != at;
        let meta_changed = prev_meta != Some(meta);
        let target_changed = prev_target != target;

        if let Some(over) = &mut self.pointer.file_drag.over {
            over.aimed = Some(target.clone());
            over.within_aimed.clone_from(&within_paths);
            if target_changed {
                over.tx_op_override = None;
                over.tx_desc_override = None;
            }
            if moved || meta_changed {
                over.last_moved_at = at;
                over.last_moved_meta = Some(meta);
            }
        }

        let (mut ran, hover_op_set, hover_desc_set) = self.set_drop_hover(
            app, target.clone(), Some(paths), at, meta, base_feedback.operation(), tx,
        );
        if let Some(over) = &mut self.pointer.file_drag.over {
            if let Some(op) = hover_op_set {
                over.tx_op_override = Some(op.filter(|&o| meta.allowed.contains(o)));
            }
            if let Some(desc) = hover_desc_set {
                over.tx_desc_override = Some(desc);
            }
        }

        let mut feedback = base_feedback;
        if let Some(over) = &self.pointer.file_drag.over {
            if let Some(op) = over.tx_op_override {
                feedback = feedback.with_operation(op).constrain(meta.allowed);
            }
            if let Some(desc) = &over.tx_desc_override {
                feedback = feedback.with_description(desc.clone());
            }
        }

        ran |= self.set_drop_hover_within(app, within, Some(paths), at, meta, feedback.operation(), tx);

        if let Some(aim) = target
            && (moved || meta_changed || target_changed)
            && let Some(f) = self.drop_move_fn(&aim)
            && let Some(p) = self.drop_point(&aim.0, at)
        {
            self.trace_event(format_args!("drop-move:{}", aim.0));
            set_tx_drag(tx, Some(p), Some(paths.clone()), meta, feedback.operation());
            f(app.states(), p, tx);
            let (op_set, desc_set) = (tx.drop_op_set.take(), tx.drop_desc_set.take());
            clear_tx_drag(tx);
            if let Some(op) = op_set {
                let clamped = op.filter(|&o| meta.allowed.contains(o));
                feedback = feedback.with_operation(clamped);
                if let Some(over) = &mut self.pointer.file_drag.over { over.tx_op_override = Some(clamped); }
            }
            if let Some(desc) = desc_set {
                feedback = feedback.with_description(desc.clone());
                if let Some(over) = &mut self.pointer.file_drag.over { over.tx_desc_override = Some(desc); }
            }
            ran = true;
        }

        if (moved || meta_changed) && !paths.is_empty() && at.is_some() {
            let move_within: Vec<(String, DropMoveFn)> = chain.iter().copied()
                .filter_map(|i| Some((self.tree.nodes[i].path.clone(), self.tree.nodes[i].handlers.drop.as_ref()?.on_move_within.clone()?)))
                .collect();
            for (path, f) in move_within {
                let Some(p) = self.drop_point(&path, at) else { continue };
                self.trace_event(format_args!("drop-move-within:{path}"));
                set_tx_drag(tx, Some(p), Some(paths.clone()), meta, feedback.operation());
                f(app.states(), p, tx);
                clear_tx_drag(tx);
                ran = true;
            }
        }

        if let Some(over) = &mut self.pointer.file_drag.over {
            over.feedback = Some(feedback);
            over.checked_commit = self.commit.commits + u64::from(ran && !tx.live_only);
        }
        ran
    }

    fn set_drop_hover(
        &mut self,
        app: &mut dyn AppDriver,
        target: Option<Aim>,
        paths: Option<&Rc<[PathBuf]>>,
        at: Option<(f32, f32)>,
        meta: DragMeta,
        operation: Option<DragOperation>,
        tx: &mut Tx,
    ) -> (bool, Option<Option<DragOperation>>, Option<Option<DropDescription>>) {
        if self.pointer.file_drag.hovered.as_ref().map(|(aim, _)| aim) == target.as_ref() {
            return (false, None, None);
        }
        let old = self.pointer.file_drag.hovered.take();
        let new = target.map(|aim| {
            let f = self.drop_hover_fn(&aim);
            (aim, f)
        });
        self.pointer.file_drag.hovered.clone_from(&new);
        let (mut ran, mut op_set, mut desc_set) = (false, None, None);
        for (hover, over) in [(old, false), (new, true)] {
            let Some((aim, kept)) = hover else { continue };
            let Some(f) = self.drop_hover_fn(&aim).or(kept) else { continue };
            let (path, accepted) = &aim;
            let kind = if *accepted { "drop-hover" } else { "drop-rejected-hover" };
            self.trace_event(format_args!("{kind}:{over}:{path}"));
            if over {
                set_tx_drag(tx, self.drop_point(path, at), paths.cloned(), meta, operation);
            } else {
                clear_tx_drag(tx);
            }
            f(app.states(), over, tx);
            if over {
                op_set = tx.drop_op_set.take();
                desc_set = tx.drop_desc_set.take();
            }
            clear_tx_drag(tx);
            ran = true;
        }
        (ran, op_set, desc_set)
    }

    fn set_drop_hover_within(
        &mut self,
        app: &mut dyn AppDriver,
        next: Vec<(String, DropHoverFn)>,
        paths: Option<&Rc<[PathBuf]>>,
        at: Option<(f32, f32)>,
        meta: DragMeta,
        operation: Option<DragOperation>,
        tx: &mut Tx,
    ) -> bool {
        let old = std::mem::replace(&mut self.pointer.file_drag.within_hovered, next.clone());
        let mut ran = false;
        for (path, kept) in &old {
            if next.iter().any(|(p, _)| p == path) { continue; }
            let f = self.drop_hover_within_fn(path).unwrap_or_else(|| kept.clone());
            self.trace_event(format_args!("drop-hover-within:false:{path}"));
            clear_tx_drag(tx);
            f(app.states(), false, tx);
            clear_tx_drag(tx);
            ran = true;
        }
        for (path, f) in &next {
            if old.iter().any(|(p, _)| p == path) { continue; }
            self.trace_event(format_args!("drop-hover-within:true:{path}"));
            set_tx_drag(tx, self.drop_point(path, at), paths.cloned(), meta, operation);
            f(app.states(), true, tx);
            clear_tx_drag(tx);
            ran = true;
        }
        ran
    }

    pub(in crate::runtime) fn run_files(&mut self, app: &mut dyn AppDriver, event: FileEvent, tx: &mut Tx) -> bool {
        match event {
            FileEvent::Hover(paths, at, meta) => self.hover_files(app, &paths, at, meta, true, tx),
            FileEvent::Move => {
                let Some(over) = &mut self.pointer.file_drag.over else { return false };
                over.move_queued = false;
                let (paths, at, meta) = (over.paths.clone(), over.at, over.meta);
                self.hover_files(app, &paths, at, meta, false, tx)
            }
            FileEvent::Leave => {
                let (mut ran, _, _) = self.set_drop_hover(app, None, None, None, DragMeta::default(), None, tx);
                ran |= self.set_drop_hover_within(app, Vec::new(), None, None, DragMeta::default(), None, tx);
                ran
            }
            FileEvent::Drop { paths, at, meta, operation, prev_override } => {
                let chain = self.drop_hit_chain(at);
                let target = self.drop_target_in_chain(&chain, Some(app.states_ref()), &paths);
                let base_feedback = self.evaluate_target_feedback(app.states_ref(), target.as_ref(), &paths, at, meta);
                let (mut ran, _, _) = self.set_drop_hover(app, None, None, None, DragMeta::default(), None, tx);
                ran |= self.set_drop_hover_within(app, Vec::new(), None, None, DragMeta::default(), None, tx);
                let Some(Target { i, files, rejected }) = target else { return ran };
                let Some(drop) = self.tree.nodes[i].handlers.drop.as_deref().cloned() else { return ran };
                let path = self.tree.nodes[i].path.clone();
                let mut resolved_op = operation.or_else(|| base_feedback.operation());
                let mut had_override = drop.feedback.is_some();
                if operation.is_none() && let Some((over_path, over_op)) = prev_override && over_path == path {
                    resolved_op = over_op.filter(|&o| meta.allowed.contains(o));
                    had_override = true;
                }
                let refused_by_feedback = operation.is_none() && had_override && resolved_op.is_none();
                set_tx_drag(tx, self.drop_point(&path, at), Some(paths), meta, resolved_op);
                if !files.is_empty() && !refused_by_feedback && let Some(f) = drop.on_drop {
                    self.trace_event(format_args!("drop-files:{}:{path}", files.len()));
                    f(app.states(), &files, tx);
                    ran = true;
                }
                if !rejected.is_empty() && let Some(f) = drop.on_reject {
                    self.trace_event(format_args!("drop-rejected:{}:{path}", rejected.len()));
                    f(app.states(), &rejected, tx);
                    ran = true;
                }
                clear_tx_drag(tx);
                ran
            }
        }
    }
}
