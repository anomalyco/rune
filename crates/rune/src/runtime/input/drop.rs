//! Files dragged onto a window from another app: `El::on_drop_files`, `El::accepts_files`,
//! `El::accepts_drag`, `El::on_drop_hover`, `El::on_drop_move`, `El::on_drop_rejected`,
//! `El::on_drop_rejected_hover`, `El::on_drop_hover_within` and `El::on_drop_move_within`.
use super::*;
use crate::el::{DropHoverFn, PointerEvent};
use crate::el::drop::DropMoveFn;
use std::path::PathBuf;
use std::rc::Rc;

/// One step of a file drag over the window, queued for the next frame.
pub(crate) enum FileEvent {
    /// The drag entered or moved. A position the platform can't give is `None`.
    Hover(Rc<[PathBuf]>, Option<(f32, f32)>),
    /// The drag moved over a target that follows it (`El::on_drop_move`), a passive observer
    /// (`El::on_drop_move_within`), or a hit chain with state-dependent acceptance
    /// (`El::accepts_drag`); it runs as a hover at the drag's latest point.
    Move,
    Drop(Rc<[PathBuf]>, Option<(f32, f32)>),
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
    /// The position last delivered to `on_drop_move` / `on_drop_move_within`, so a stationary
    /// `accepts_drag` recheck does not repeat move callbacks.
    last_moved_at: Option<(f32, f32)>,
    /// The target of the last hover queued or run (`None` while awaiting stateful `accepts_drag`
    /// evaluation on the frame), so a move that keeps it queues nothing.
    aimed: Option<Option<Aim>>,
    /// The `on_drop_hover_within` element paths of the last hover queued or run.
    within_aimed: Vec<String>,
    /// `commit.commits` after the frame in which `hover_files` last evaluated `accepts_drag`.
    checked_commit: u64,
    /// A `FileEvent::Move` waits to run.
    move_queued: bool,
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

impl capture::PointerState {
    /// A file drag is over the window.
    #[inline]
    pub(in crate::runtime) fn dragging_files(&self) -> bool { self.file_drag.over.is_some() }

    /// File steps wait for the next frame (`hold_files`).
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
        let aims = |a: &Action| matches!(a, Action::Files(FileEvent::Hover(..) | FileEvent::Move | FileEvent::Drop(..)));
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
        let paths: Rc<[PathBuf]> = paths.into();
        let chain = self.drop_hit_chain(at);
        let stateful = !paths.is_empty() && self.chain_has_accepts_drag(&chain);
        let aimed = if stateful {
            None
        } else {
            Some(self.drop_target_in_chain(&chain, None, &paths)
                .map(|t| (self.tree.nodes[t.i].path.clone(), !t.files.is_empty())))
        };
        let within_aimed = self.chain_within_hover_paths(&chain, &paths);
        // A move a lost frame never ran mustn't keep this drag's moves from queueing.
        self.pointer.file_drag.over = Some(Over {
            paths: paths.clone(),
            at,
            last_moved_at: None,
            aimed,
            within_aimed,
            checked_commit: self.commit.commits,
            move_queued: false,
        });
        self.frame.queue.push(Action::Files(FileEvent::Hover(paths, at)));
    }

    /// The file drag is at `at`, with the files it entered with. Queues a step, and returns
    /// true, only when that changes the drop target or `on_drop_hover_within` set, moves over a
    /// target or ancestor that follows it (`El::on_drop_move` / `El::on_drop_move_within`, once
    /// a frame), or moves/commits over a hit chain with `El::accepts_drag`, so a host can
    /// sample the pointer often and redraw only when something listens. Sampling while still
    /// also notices a target that unmounted or a view commit that changed `accepts_drag`.
    pub fn drag_files_to(&mut self, at: Option<(f32, f32)>) -> bool {
        let Some(over) = &self.pointer.file_drag.over else { return false };
        let moved = over.at != at;
        let paths = over.paths.clone();
        let chain = self.drop_hit_chain(at);
        let within = self.chain_within_hover_paths(&chain, &paths);
        let within_same = within == over.within_aimed;
        let stateful = !paths.is_empty() && self.chain_has_accepts_drag(&chain);
        let follows_within = at.is_some() && self.chain_has_move_within(&chain, &paths);

        if stateful {
            let committed = self.commit.commits != over.checked_commit;
            let Some(over) = &mut self.pointer.file_drag.over else { return false };
            over.at = at;
            if !within_same {
                over.within_aimed = within;
                self.frame.queue.push(Action::Files(FileEvent::Hover(paths, at)));
                return true;
            }
            if (!moved && !committed) || std::mem::replace(&mut over.move_queued, true) {
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
        if same {
            if !moved || !follows || std::mem::replace(&mut over.move_queued, true) { return false; }
            self.frame.queue.push(Action::Files(FileEvent::Move));
            return true;
        }
        over.aimed = Some(target);
        over.within_aimed = within;
        self.frame.queue.push(Action::Files(FileEvent::Hover(paths, at)));
        true
    }

    /// The file drag left the window, was cancelled, or the window hid.
    pub fn drag_files_leave(&mut self) {
        if self.pointer.file_drag.over.take().is_none() { return; }
        self.frame.queue.push(Action::Files(FileEvent::Leave));
    }

    /// `paths` dropped at `at` (points, or `None` when the platform doesn't say). Next frame the
    /// innermost [`El::on_drop_files`] under it that accepts any of them gets those it accepts,
    /// in one call, and the drop hover ends. Its [`El::on_drop_rejected`] gets the rest, and a
    /// target with one that takes none of them gets them all there. A drop nothing takes is
    /// ignored.
    pub fn drop_files(&mut self, paths: Vec<PathBuf>, at: Option<(f32, f32)>) {
        self.pointer.file_drag.over = None;
        self.frame.queue.push(Action::Files(FileEvent::Drop(paths.into(), at)));
    }

    /// A file drag is over the window. Hosts whose platform reports no drag positions sample
    /// the pointer while this holds.
    #[inline]
    pub fn dragging_files(&self) -> bool { self.pointer.dragging_files() }

    /// The key (else path) of the element showing the drop hover, for tests and tools.
    pub fn drop_hovered(&self) -> Option<String> { self.pointer.drop_hover_key(|p| self.node(p), true) }

    /// The key (else path) of the element showing the rejected drop hover
    /// ([`El::on_drop_rejected_hover`]), for tests and tools.
    pub fn drop_rejected_hovered(&self) -> Option<String> { self.pointer.drop_hover_key(|p| self.node(p), false) }

    /// A file step that hit-tests (a hover or drop) reaching a tree changed since its last build
    /// (`stale`: state changed this frame, or while the window was covered and built nothing)
    /// waits for this frame's build, once, so it lands on what is mounted and laid out now. The
    /// file steps after it wait too, to keep their order.
    #[inline]
    pub(in crate::runtime) fn hold_files(&mut self, queue: ActionQueue, stale: bool) -> ActionQueue {
        self.pointer.hold_files(queue, stale)
    }

    /// The held file steps go first next frame.
    pub(in crate::runtime) fn release_files(&mut self) {
        if let Some(held) = self.pointer.take_held_files() {
            self.frame.queue.prepend(held);
        }
    }

    /// File steps wait for the next frame (`hold_files`).
    #[inline]
    pub(in crate::runtime) fn files_held(&self) -> bool { self.pointer.files_held() }

    #[inline]
    pub(in crate::runtime) fn clear_drop_hits(&mut self) { self.hit.clear_drop_hits(); }

    /// Whether the live, allowed view tree currently mounts any drop target (`El::on_drop_files`,
    /// `El::on_drop_rejected` / `El::on_drop_rejected_hover`) or passive drop observer
    /// (`El::on_drop_hover_within` / `El::on_drop_move_within`). Matches `drop_hit_chain`: exiting
    /// subtrees and elements behind an active modal do not count.
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

    /// Live, allowed nodes from the topmost element under `at` up to the active modal or app root
    /// (innermost first, deduplicated). While a modal is open its root stands in for the window's,
    /// and a point outside it reaches nothing. A point outside the window reaches nothing; a drag
    /// without a position reaches only the root.
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

    /// The target for `paths` along `chain` and the files it takes: the first drop target that
    /// accepts any of them or stops what it doesn't take (`El::on_drop_rejected`).
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

    /// The `El::on_drop_move` of a target that takes the drag.
    fn drop_move_fn(&self, (path, accepted): &Aim) -> Option<DropMoveFn> {
        self.node(path).filter(|_| *accepted).and_then(|n| n.handlers.drop.as_ref()?.on_move.clone())
    }

    /// `at` on the mounted element `path`, for `Tx::drop_point`.
    fn drop_point(&self, path: &str, at: Option<(f32, f32)>) -> Option<PointerEvent> {
        let (x, y) = at?;
        let i = self.live_index(path)?;
        Some(self.pointer_event(i, x, y))
    }

    /// A hover at `at`: moves the drop hover to the target there, updates passive ancestor
    /// observers (`El::on_drop_hover_within`), then tells a target (`El::on_drop_move`) and
    /// passive observers (`El::on_drop_move_within`) where the drag is.
    fn hover_files(
        &mut self,
        app: &mut dyn AppDriver,
        paths: &Rc<[PathBuf]>,
        at: Option<(f32, f32)>,
        force_move: bool,
        tx: &mut Tx,
    ) -> bool {
        let chain = self.drop_hit_chain(at);
        let target = self.drop_target_in_chain(&chain, Some(app.states_ref()), paths)
            .map(|t| (self.tree.nodes[t.i].path.clone(), !t.files.is_empty()));
        let within: Vec<(String, DropHoverFn)> = if paths.is_empty() {
            Vec::new()
        } else {
            chain.iter().copied()
                .filter_map(|i| {
                    let f = self.tree.nodes[i].handlers.drop.as_ref()?.on_hover_within.clone()?;
                    Some((self.tree.nodes[i].path.clone(), f))
                })
                .collect()
        };
        let within_paths: Vec<String> = within.iter().map(|(p, _)| p.clone()).collect();
        let prev_target = self.pointer.file_drag.hovered.as_ref().map(|(aim, _)| aim.clone());
        let prev_moved_at = self.pointer.file_drag.over.as_ref().and_then(|o| o.last_moved_at);
        let moved = force_move || prev_moved_at != at;

        if let Some(over) = &mut self.pointer.file_drag.over {
            over.aimed = Some(target.clone());
            over.within_aimed.clone_from(&within_paths);
            if moved { over.last_moved_at = at; }
        }

        let target_changed = prev_target != target;
        let mut ran = self.set_drop_hover(app, target.clone(), Some(paths), at, tx);
        ran |= self.set_drop_hover_within(app, within, Some(paths), at, tx);

        if let Some(aim) = target
            && (moved || target_changed)
            && let Some(f) = self.drop_move_fn(&aim)
            && let Some(p) = self.drop_point(&aim.0, at)
        {
            self.trace_event(format_args!("drop-move:{}", aim.0));
            tx.drop_point = Some(p);
            tx.drag_paths = Some(paths.clone());
            f(app.states(), p, tx);
            tx.drop_point = None;
            tx.drag_paths = None;
            ran = true;
        }

        if moved && !paths.is_empty() && at.is_some() {
            let move_within: Vec<(String, DropMoveFn)> = chain.iter().copied()
                .filter_map(|i| {
                    let f = self.tree.nodes[i].handlers.drop.as_ref()?.on_move_within.clone()?;
                    Some((self.tree.nodes[i].path.clone(), f))
                })
                .collect();
            for (path, f) in move_within {
                let Some(p) = self.drop_point(&path, at) else { continue };
                self.trace_event(format_args!("drop-move-within:{path}"));
                tx.drop_point = Some(p);
                tx.drag_paths = Some(paths.clone());
                f(app.states(), p, tx);
                tx.drop_point = None;
                tx.drag_paths = None;
                ran = true;
            }
        }

        if let Some(over) = &mut self.pointer.file_drag.over {
            over.checked_commit = self.commit.commits + u64::from(ran && !tx.live_only);
        }
        ran
    }

    /// Moves the drop hover to `target`, notifying the old and new elements.
    fn set_drop_hover(
        &mut self,
        app: &mut dyn AppDriver,
        target: Option<Aim>,
        paths: Option<&Rc<[PathBuf]>>,
        at: Option<(f32, f32)>,
        tx: &mut Tx,
    ) -> bool {
        if self.pointer.file_drag.hovered.as_ref().map(|(aim, _)| aim) == target.as_ref() { return false; }
        let old = self.pointer.file_drag.hovered.take();
        let new = target.map(|aim| {
            let f = self.drop_hover_fn(&aim);
            (aim, f)
        });
        self.pointer.file_drag.hovered.clone_from(&new);
        let mut ran = false;
        for (hover, over) in [(old, false), (new, true)] {
            let Some((aim, kept)) = hover else { continue };
            // The element's current callback, else the one it had if it has gone.
            let Some(f) = self.drop_hover_fn(&aim).or(kept) else { continue };
            let (path, accepted) = &aim;
            let kind = if *accepted { "drop-hover" } else { "drop-rejected-hover" };
            self.trace_event(format_args!("{kind}:{over}:{path}"));
            tx.drop_point = if over { self.drop_point(path, at) } else { None };
            tx.drag_paths = if over { paths.cloned() } else { None };
            f(app.states(), over, tx);
            tx.drop_point = None;
            tx.drag_paths = None;
            ran = true;
        }
        ran
    }

    /// Moves the passive `El::on_drop_hover_within` set to `next`, notifying elements that left
    /// with `false` and elements that entered with `true` (innermost first).
    fn set_drop_hover_within(
        &mut self,
        app: &mut dyn AppDriver,
        next: Vec<(String, DropHoverFn)>,
        paths: Option<&Rc<[PathBuf]>>,
        at: Option<(f32, f32)>,
        tx: &mut Tx,
    ) -> bool {
        let old = std::mem::replace(&mut self.pointer.file_drag.within_hovered, next.clone());
        let mut ran = false;
        for (path, kept) in &old {
            if next.iter().any(|(p, _)| p == path) { continue; }
            let f = self.drop_hover_within_fn(path).unwrap_or_else(|| kept.clone());
            self.trace_event(format_args!("drop-hover-within:false:{path}"));
            tx.drop_point = None;
            tx.drag_paths = None;
            f(app.states(), false, tx);
            ran = true;
        }
        for (path, f) in &next {
            if old.iter().any(|(p, _)| p == path) { continue; }
            self.trace_event(format_args!("drop-hover-within:true:{path}"));
            tx.drop_point = self.drop_point(path, at);
            tx.drag_paths = paths.cloned();
            f(app.states(), true, tx);
            tx.drop_point = None;
            tx.drag_paths = None;
            ran = true;
        }
        ran
    }

    pub(in crate::runtime) fn run_files(&mut self, app: &mut dyn AppDriver, event: FileEvent, tx: &mut Tx) -> bool {
        match event {
            FileEvent::Hover(paths, at) => self.hover_files(app, &paths, at, true, tx),
            FileEvent::Move => {
                let Some(over) = &mut self.pointer.file_drag.over else { return false };
                over.move_queued = false;
                let (paths, at) = (over.paths.clone(), over.at);
                self.hover_files(app, &paths, at, false, tx)
            }
            FileEvent::Leave => {
                let mut ran = self.set_drop_hover(app, None, None, None, tx);
                ran |= self.set_drop_hover_within(app, Vec::new(), None, None, tx);
                ran
            }
            FileEvent::Drop(paths, at) => {
                let chain = self.drop_hit_chain(at);
                let target = self.drop_target_in_chain(&chain, Some(app.states_ref()), &paths);
                let mut ran = self.set_drop_hover(app, None, None, None, tx);
                ran |= self.set_drop_hover_within(app, Vec::new(), None, None, tx);
                let Some(Target { i, files, rejected }) = target else { return ran };
                let Some(drop) = self.tree.nodes[i].handlers.drop.as_deref().cloned() else { return ran };
                let path = self.tree.nodes[i].path.clone();
                tx.drop_point = self.drop_point(&path, at);
                tx.drag_paths = Some(paths);
                if !files.is_empty() && let Some(f) = drop.on_drop {
                    self.trace_event(format_args!("drop-files:{}:{path}", files.len()));
                    f(app.states(), &files, tx);
                    ran = true;
                }
                if !rejected.is_empty() && let Some(f) = drop.on_reject {
                    self.trace_event(format_args!("drop-rejected:{}:{path}", rejected.len()));
                    f(app.states(), &rejected, tx);
                    ran = true;
                }
                tx.drop_point = None;
                tx.drag_paths = None;
                ran
            }
        }
    }
}
