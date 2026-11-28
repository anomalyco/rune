//! Touch text-selection UI bridge: caret/handle geometry (clipped to scroll viewports and the
//! window), handle grab/drag/release for focused text fields and `El::selectable` text (including
//! `El::selection_group`), and re-opening a text toolbar for the live selection.
//!
//! Each dragged handle stops short of the opposite end so a range selection never inverts or
//! collapses to empty; holding a handle at a viewport's top or bottom edge drives the runtime's
//! shared `edge_scroll` machine.
#![cfg_attr(not(any(target_os = "android", test)), allow(dead_code))]

use super::*;
use super::selectable::phase;
use super::selectable::virtual_selection::EndRef;

/// Which handle of a touch selection is being manipulated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Handle {
    /// Leading end of a non-empty selection (smaller offset / earlier in tree order).
    Start,
    /// Trailing end of a non-empty selection (larger offset / later in tree order).
    End,
    /// Collapsed caret of an editable focused text field.
    Caret,
}

/// Where a caret stands at one end of a selection, in window points (after zoom), along with
/// whether it lies inside its enclosing clip rect and viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct HandleAnchor {
    pub x: f32,
    pub top: f32,
    pub bottom: f32,
    pub visible: bool,
}

impl HandleAnchor {
    pub(crate) const fn hidden() -> Self {
        Self { x: 0.0, top: 0.0, bottom: 0.0, visible: false }
    }
}

/// Active touch selection mode exposed to a native host (e.g. Android handles + floating toolbar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TouchSelectionKind {
    /// Editable focused text field with a collapsed caret.
    FieldCaret,
    /// Focused text field (editable or read-only) with a non-empty range selection.
    FieldRange,
    /// `El::selectable` text (or `El::selection_group`) with a non-empty range selection.
    SelectableRange,
}

/// Snapshot of the active selection's clipped handle geometry and toolbar avoidance rect in
/// window points (after zoom).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TouchSelectionSnapshot {
    pub kind: TouchSelectionKind,
    /// Start handle for a range, or the caret anchor for `FieldCaret`.
    pub start: HandleAnchor,
    /// End handle for a range, or `None` for `FieldCaret`.
    pub end: Option<HandleAnchor>,
    /// Visible bounding box `[left, top, right, bottom]` of the selection in window points
    /// (`[0.0; 4]` when clipped completely out of sight).
    pub content_rect: [f32; 4],
    /// Whether the target allows edits (Cut and Paste).
    pub editable: bool,
    /// Whether a non-empty, non-secure selection exists (Copy; and Cut when `editable`).
    pub can_copy: bool,
    /// Whether Select All can select text in the active target.
    pub can_select_all: bool,
    /// Monotonic revision bumped on touch taps / field-menu opens / caret handle releases.
    pub revision: u64,
}

/// Explicit state of an in-progress native handle drag, owned by `TextInputState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::runtime) enum HandleDrag {
    #[default]
    Idle,
    /// Dragging a handle on focused `text_input` at `path`.
    Field {
        path: String,
        handle: Handle,
        /// Fixed opposite byte offset (`End` when dragging `Start`, `Start` when dragging `End`).
        fixed: usize,
        /// Last drag point in view points (fed to `edge_scroll`).
        pos: (f32, f32),
    },
    /// Dragging `Start` or `End` of an `El::selectable` selection.
    Selectable {
        handle: Handle,
        /// Last drag point in view points (fed to `edge_scroll`).
        pos: (f32, f32),
    },
}

impl HandleDrag {
    #[inline]
    pub(in crate::runtime) fn is_active(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    #[inline]
    pub(in crate::runtime) fn pos(&self) -> Option<(f32, f32)> {
        match *self {
            Self::Idle => None,
            Self::Field { pos, .. } | Self::Selectable { pos, .. } => Some(pos),
        }
    }

    #[inline]
    pub(in crate::runtime) fn field_path(&self) -> Option<&str> {
        match self {
            Self::Field { path, .. } => Some(path.as_str()),
            Self::Idle | Self::Selectable { .. } => None,
        }
    }

    #[inline]
    pub(in crate::runtime) fn is_selectable(&self) -> bool {
        matches!(self, Self::Selectable { .. })
    }
}

/// Next UTF-8 char boundary strictly after `byte` in `text`, or `None` at end.
fn next_char_boundary(text: &str, byte: usize) -> Option<usize> {
    let at = text.floor_char_boundary(byte.min(text.len()));
    text.get(at..)?.chars().next().map(|c| at + c.len_utf8())
}

/// Previous UTF-8 char boundary strictly before `byte` in `text`, or `None` at 0.
fn prev_char_boundary(text: &str, byte: usize) -> Option<usize> {
    let at = text.floor_char_boundary(byte.min(text.len()));
    text.get(..at)?.chars().next_back().map(|c| at - c.len_utf8())
}

/// Maximum byte offset strictly before `fixed` in `e` that does not split an atomic pill.
fn prev_field_stop(e: &crate::edit::TextEdit, fixed: usize) -> Option<usize> {
    let mut stop = prev_char_boundary(&e.text, fixed)?;
    for (r, _) in e.pills() {
        if r.start < stop && stop < r.end {
            stop = r.start;
        }
    }
    Some(stop)
}

/// Minimum byte offset strictly after `fixed` in `e` that does not split an atomic pill.
fn next_field_stop(e: &crate::edit::TextEdit, fixed: usize) -> Option<usize> {
    let mut stop = next_char_boundary(&e.text, fixed)?;
    for (r, _) in e.pills() {
        if r.start < stop && stop < r.end {
            stop = r.end;
        }
    }
    Some(stop)
}

fn union_rect(acc: Option<[f32; 4]>, r: [f32; 4]) -> Option<[f32; 4]> {
    if r[2] <= r[0] || r[3] <= r[1] {
        return acc;
    }
    Some(match acc {
        None => r,
        Some(a) => [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])],
    })
}

fn scale_rect(r: [f32; 4], k: f32) -> [f32; 4] {
    [r[0] * k, r[1] * k, r[2] * k, r[3] * k]
}

impl RuntimeCore {
    /// Clipped handle anchors and toolbar content rect for the active field or selectable text,
    /// in window points (after zoom). Returns `None` when nothing has a touch-selectable caret or
    /// range.
    pub(crate) fn touch_selection_snapshot(&self) -> Option<TouchSelectionSnapshot> {
        if let Some(snap) = self.field_touch_selection_snapshot() {
            return Some(snap);
        }
        self.selectable_touch_selection_snapshot()
    }

    fn field_touch_selection_snapshot(&self) -> Option<TouchSelectionSnapshot> {
        let i = self.focused_index()?;
        if self.subtree_exiting(i) || self.node_disabled(i) {
            return None;
        }
        let n = &self.tree.nodes[i];
        let (inp, spec, laid) = (n.input.as_deref()?, n.text.as_deref()?, n.text_layout.as_ref()?);
        if !inp.edit.focused || inp.edit.preedit().is_some() {
            return None;
        }
        let read_only = self.is_node_read_only(i);
        let secure = inp.edit.is_secure();
        let (a, b) = inp.edit.range();
        if a == b && read_only {
            return None;
        }
        let valid = self.tree.built && self.paint.drawn.len() == self.tree.nodes.len();
        let rect = if valid { self.drawn_box(i) } else { n.target };
        let z = if valid { self.paint.drawn_scale.get(i).copied().unwrap_or(1.0).max(1e-4) } else { 1.0 };
        let s = self.env.scale;
        let flow = laid.view();
        let [mut ox, oy] = links::origin(n, spec, flow, rect, s, z);
        if inp.single_line {
            ox -= self.edit.single_line_scroll(&n.path) * z;
        }
        let clip = self.node_visible_clip(i, Some(rect));
        let zoom = self.host.zoom.k;
        let anchor_at = |byte: usize, affinity: parley::Affinity| -> HandleAnchor {
            let bb = flow.caret(byte, affinity, 0.0);
            let x = ox + (bb.x0 as f32 / s) * z;
            let top = oy + (bb.y0 as f32 / s) * z;
            let bottom = if bb.y1 > bb.y0 {
                oy + (bb.y1 as f32 / s) * z
            } else {
                top + spec.style.line_px() * z
            };
            let visible = Self::anchor_visible(x, top, bottom, clip);
            HandleAnchor { x: x * zoom, top: top * zoom, bottom: bottom * zoom, visible }
        };
        let can_select_all = !inp.edit.text.is_empty();
        if a == b {
            let byte = caret_index(&inp.edit, &spec.content);
            let start = anchor_at(byte, inp.edit.affinity);
            let content_rect = if start.visible {
                [start.x, start.top, start.x, start.bottom]
            } else {
                [0.0; 4]
            };
            return Some(TouchSelectionSnapshot {
                kind: TouchSelectionKind::FieldCaret,
                start,
                end: None,
                content_rect,
                editable: true,
                can_copy: false,
                can_select_all,
                revision: self.edit.touch_selection_rev,
            });
        }
        let (da, db) = inp.edit.display_selection_range(a, b);
        let start = anchor_at(da, parley::Affinity::Downstream);
        let end = anchor_at(db, parley::Affinity::Upstream);
        let mut content: Option<[f32; 4]> = None;
        for (bb, _) in flow.range_boxes(da, db, true) {
            let r = [
                ox + (bb.x0 as f32 / s) * z,
                oy + (bb.y0 as f32 / s) * z,
                ox + (bb.x1 as f32 / s) * z,
                oy + (bb.y1 as f32 / s) * z,
            ];
            content = union_rect(content, intersect(r, clip));
        }
        let content_rect = content
            .map(|r| scale_rect(r, zoom))
            .unwrap_or_else(|| Self::handles_fallback_rect(start, Some(end)));
        Some(TouchSelectionSnapshot {
            kind: TouchSelectionKind::FieldRange,
            start,
            end: Some(end),
            content_rect,
            editable: !read_only,
            can_copy: !secure,
            can_select_all,
            revision: self.edit.touch_selection_rev,
        })
    }

    fn selectable_touch_selection_snapshot(&self) -> Option<TouchSelectionSnapshot> {
        let h = self.selection_holder_end()?;
        let f = self.focus_holder_end(h)?;
        let sel = self.edit.selectable.phase.selection()?;
        let ((start_end, start_byte), (end_end, end_byte)) = if h == f {
            let (a, b) = (sel.anchor.min(sel.focus), sel.anchor.max(sel.focus));
            if a >= b {
                return None;
            }
            ((h, a), (h, b))
        } else if self.tree_order(h) < self.tree_order(f) {
            ((h, sel.anchor), (f, sel.focus))
        } else {
            ((f, sel.focus), (h, sel.anchor))
        };
        let start = self.selectable_end_anchor(start_end, start_byte, parley::Affinity::Downstream);
        let end = self.selectable_end_anchor(end_end, end_byte, parley::Affinity::Upstream);
        let zoom = self.host.zoom.k;
        let mut content: Option<[f32; 4]> = None;
        for (i, a, b) in self.selection_ranges() {
            let clip = self.node_visible_clip(i, None);
            self.for_each_selectable_box(i, a, b, |r| {
                content = union_rect(content, intersect(r, clip));
            });
        }
        let content_rect = content
            .map(|r| scale_rect(r, zoom))
            .unwrap_or_else(|| Self::handles_fallback_rect(start, Some(end)));
        Some(TouchSelectionSnapshot {
            kind: TouchSelectionKind::SelectableRange,
            start,
            end: Some(end),
            content_rect,
            editable: false,
            can_copy: true,
            can_select_all: false,
            revision: self.edit.touch_selection_rev,
        })
    }

    fn selectable_end_anchor(&self, end: EndRef, byte: usize, affinity: parley::Affinity) -> HandleAnchor {
        let EndRef::Mounted(i) = end else {
            return HandleAnchor::hidden();
        };
        if self.is_text_hidden(i) {
            return HandleAnchor::hidden();
        }
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_deref() else {
            return HandleAnchor::hidden();
        };
        let valid = self.tree.built && self.paint.drawn.len() == self.tree.nodes.len();
        if valid && self.paint.drawn.get(i).is_none_or(|d| d[2] <= 0.0 || d[3] <= 0.0) {
            return HandleAnchor::hidden();
        }
        let rect = if valid { self.drawn_box(i) } else { n.target };
        let z = if valid { self.paint.drawn_scale.get(i).copied().unwrap_or(1.0).max(1e-4) } else { 1.0 };
        let clip = self.node_visible_clip(i, None);
        let zoom = self.host.zoom.k;
        if let Some(pt) = spec.positioned() {
            let tf = super::positioned::PositionedTransform::from_window_box(n, pt, rect, z);
            let Some(c) = pt.caret(byte.min(pt.len())) else {
                return HandleAnchor::hidden();
            };
            let x = tf.ox + c[0] * tf.sx;
            let top = tf.oy + c[1] * tf.sy;
            let bottom = top + (c[3] * tf.sy).max(1.0);
            let visible = Self::anchor_visible(x, top, bottom, clip);
            return HandleAnchor { x: x * zoom, top: top * zoom, bottom: bottom * zoom, visible };
        }
        let Some(laid) = n.text_layout.as_ref() else {
            return HandleAnchor::hidden();
        };
        let flow = laid.view();
        let s = self.env.scale;
        let [ox, oy] = links::origin(n, spec, flow, rect, s, z);
        let clamped = byte.min(spec.content.len());
        let drawn_byte = n.elided.map_or(clamped, |e| e.range(clamped, clamped).0);
        let bb = flow.caret(drawn_byte, affinity, 0.0);
        let x = ox + (bb.x0 as f32 / s) * z;
        let top = oy + (bb.y0 as f32 / s) * z;
        let bottom = if bb.y1 > bb.y0 {
            oy + (bb.y1 as f32 / s) * z
        } else {
            top + spec.style.line_px() * z
        };
        let visible = Self::anchor_visible(x, top, bottom, clip);
        HandleAnchor { x: x * zoom, top: top * zoom, bottom: bottom * zoom, visible }
    }

    fn for_each_selectable_box(&self, i: usize, a: usize, b: usize, mut f: impl FnMut([f32; 4])) {
        let n = &self.tree.nodes[i];
        let Some(spec) = n.text.as_deref() else { return };
        let valid = self.tree.built && self.paint.drawn.len() == self.tree.nodes.len();
        let rect = if valid { self.drawn_box(i) } else { n.target };
        let z = if valid { self.paint.drawn_scale.get(i).copied().unwrap_or(1.0).max(1e-4) } else { 1.0 };
        if let Some(pt) = spec.positioned() {
            let len = pt.len();
            let (a, b) = (a.min(len), b.min(len));
            if a >= b { return; }
            let tf = super::positioned::PositionedTransform::from_window_box(n, pt, rect, z);
            pt.for_each_range_box(a, b, |r| {
                let [x, y, w, h] = tf.rect_xywh(r);
                f([x, y, x + w, y + h]);
            });
            return;
        }
        let Some(laid) = n.text_layout.as_ref() else { return };
        let flow = laid.view();
        let len = spec.content.len();
        let (a, b) = (a.min(len), b.min(len));
        if a >= b { return; }
        let (a, b) = n.elided.map_or((a, b), |e| e.range(a, b));
        let s = self.env.scale;
        let [ox, oy] = links::origin(n, spec, flow, rect, s, z);
        for (bb, _) in flow.range_boxes(a, b, true) {
            f([
                ox + (bb.x0 as f32 / s) * z,
                oy + (bb.y0 as f32 / s) * z,
                ox + (bb.x1 as f32 / s) * z,
                oy + (bb.y1 as f32 / s) * z,
            ]);
        }
    }

    fn node_visible_clip(&self, i: usize, extra: Option<[f32; 4]>) -> [f32; 4] {
        let viewport = [0.0, 0.0, self.viewport.0, self.viewport.1];
        let mut clip = if self.tree.built && self.paint.inh_scratch.len() == self.tree.nodes.len() {
            intersect(self.screen_clip_of_parent(Some(i)).map(|v| v / self.env.scale), viewport)
        } else {
            viewport
        };
        if let Some(r) = extra {
            clip = intersect(clip, r);
        }
        clip
    }

    #[inline]
    fn anchor_visible(x: f32, top: f32, bottom: f32, clip: [f32; 4]) -> bool {
        clip[0] < clip[2]
            && clip[1] < clip[3]
            && x >= clip[0] - 1.0
            && x <= clip[2] + 1.0
            && bottom > clip[1] + 1.0
            && top < clip[3] - 1.0
    }

    fn handles_fallback_rect(start: HandleAnchor, end: Option<HandleAnchor>) -> [f32; 4] {
        let mut acc: Option<[f32; 4]> = None;
        for h in [Some(start), end].into_iter().flatten() {
            if h.visible {
                acc = Some(match acc {
                    None => [h.x, h.top, h.x, h.bottom],
                    Some(a) => [a[0].min(h.x), a[1].min(h.top), a[2].max(h.x), a[3].max(h.bottom)],
                });
            }
        }
        acc.unwrap_or([0.0; 4])
    }

    /// Starts dragging `handle` at window point `(window_x, window_y)`. Returns `true` when a
    /// matching field or selectable text selection accepted the grab.
    pub(crate) fn touch_handle_grab(&mut self, handle: Handle, window_x: f32, window_y: f32) -> bool {
        let zoom = self.host.zoom.k.max(1e-4);
        let pos = (window_x / zoom, window_y / zoom);
        self.edit.stop_edge_scroll();
        self.edit.handle_drag = HandleDrag::Idle;

        if let Some(i) = self.focused_index()
            && !self.subtree_exiting(i)
            && !self.node_disabled(i)
            && let Some(inp) = self.tree.nodes[i].input.as_deref()
            && inp.access.is_some()
            && inp.edit.focused
            && inp.edit.preedit().is_none()
        {
            let read_only = self.is_node_read_only(i);
            let (a, b) = inp.edit.range();
            let path = self.tree.nodes[i].path.clone();
            match handle {
                Handle::Caret if !read_only => {
                    self.edit.handle_drag = HandleDrag::Field { path, handle: Handle::Caret, fixed: 0, pos };
                    return true;
                }
                Handle::Start if a < b => {
                    self.edit.handle_drag = HandleDrag::Field { path, handle: Handle::Start, fixed: b, pos };
                    return true;
                }
                Handle::End if a < b => {
                    self.edit.handle_drag = HandleDrag::Field { path, handle: Handle::End, fixed: a, pos };
                    return true;
                }
                _ => {}
            }
        }

        if handle == Handle::Caret {
            return false;
        }
        if self.orient_selectable_for_handle(handle) {
            self.edit.handle_drag = HandleDrag::Selectable { handle, pos };
            return true;
        }
        false
    }

    /// Ensures `sel.anchor` holds the fixed opposite end and `sel.focus` / `sel.far` holds the
    /// end being dragged (`handle`), returning `false` when no non-empty selectable selection exists.
    fn orient_selectable_for_handle(&mut self, handle: Handle) -> bool {
        let Some(h) = self.selection_holder_end() else { return false };
        let Some(f) = self.focus_holder_end(h) else { return false };
        let Some(sel) = self.edit.selectable.phase.selection() else { return false };
        let is_forward = if h == f {
            if sel.anchor == sel.focus {
                return false;
            }
            sel.anchor < sel.focus
        } else {
            self.tree_order(h) < self.tree_order(f)
        };
        // Dragging `End` wants `anchor` at the start (forward); dragging `Start` wants `anchor`
        // at the end (backward).
        let want_forward = handle == Handle::End;
        if is_forward != want_forward {
            if let Some(sel) = self.edit.selectable.phase.selection_mut() {
                if let Some(mut far) = sel.far.take() {
                    std::mem::swap(&mut sel.path, &mut far.path);
                    std::mem::swap(&mut sel.content, &mut far.content);
                    std::mem::swap(&mut sel.anchor_end, &mut far.end);
                    std::mem::swap(&mut sel.anchor, &mut sel.focus);
                    sel.far = Some(far);
                } else {
                    std::mem::swap(&mut sel.anchor, &mut sel.focus);
                }
                sel.origin = (sel.anchor, sel.anchor);
            }
            self.seal_between();
        }
        true
    }

    /// Moves the currently grabbed handle to window point `(window_x, window_y)`. Returns `true`
    /// when the selection or caret moved.
    pub(crate) fn touch_handle_drag(
        &mut self,
        app: &mut dyn AppDriver,
        window_x: f32,
        window_y: f32,
    ) -> bool {
        let zoom = self.host.zoom.k.max(1e-4);
        let (x, y) = (window_x / zoom, window_y / zoom);
        match &mut self.edit.handle_drag {
            HandleDrag::Idle => return false,
            HandleDrag::Field { pos, .. } | HandleDrag::Selectable { pos, .. } => *pos = (x, y),
        }
        let changed = match &self.edit.handle_drag {
            HandleDrag::Field { .. } => self.drag_field_handle_view(app, x, y),
            HandleDrag::Selectable { .. } => {
                let changed = self.drag_selectable_handle_view(x, y);
                if changed {
                    let heard = self.selection_changes(app);
                    self.frame.queue.extend(heard);
                    self.commit.dirty = true;
                }
                changed
            }
            HandleDrag::Idle => false,
        };
        self.edge_scroll_moved(x, y);
        changed
    }

    /// Releases any grabbed selection handle and stops edge scroll.
    pub(crate) fn touch_handle_release(&mut self) {
        if matches!(self.edit.handle_drag, HandleDrag::Field { handle: Handle::Caret, .. }) {
            self.edit.bump_touch_selection_rev();
        }
        self.edit.handle_drag = HandleDrag::Idle;
        self.edit.stop_edge_scroll();
    }

    /// Moves a grabbed field handle to view point `(x, y)`, stopping short of the opposite end.
    pub(in crate::runtime) fn drag_field_handle_view(
        &mut self,
        app: &mut dyn AppDriver,
        x: f32,
        y: f32,
    ) -> bool {
        let HandleDrag::Field { path, handle, fixed, .. } = self.edit.handle_drag.clone() else {
            return false;
        };
        if self.kbd.focus.path() != Some(path.as_str()) {
            self.touch_handle_release();
            return false;
        }
        let Some(i) = self.live(&path) else {
            self.touch_handle_release();
            return false;
        };
        let (selection_change, access, single, layout) = {
            let n = &self.tree.nodes[i];
            let Some(inp) = n.input.as_deref() else {
                self.touch_handle_release();
                return false;
            };
            let Some(access) = inp.access.clone() else {
                self.touch_handle_release();
                return false;
            };
            (inp.selection_change.clone(), access, inp.single_line, n.text_layout.clone())
        };
        let (Some(laid), Some([tx, ty])) = (layout, self.input_text_point(i, x, y)) else {
            return false;
        };
        let s = self.env.scale;
        let changed = {
            let e = access(app.states());
            e.set_single_line(single);
            if e.preedit().is_some() {
                return false;
            }
            let h = input_click::hit(e, laid.view(), tx, ty, s, single);
            let at = input_click::unit_at(e, h, super::selectable::Unit::Char).0;
            let before = (e.anchor, e.focus);
            match handle {
                Handle::Caret => {
                    e.set_caret(at);
                    e.affinity = h.side;
                }
                Handle::Start => {
                    let fixed = fixed.min(e.text.len());
                    let Some(max_start) = prev_field_stop(e, fixed) else { return false };
                    let start = at.min(max_start);
                    e.set_selection(fixed, start);
                }
                Handle::End => {
                    let fixed = fixed.min(e.text.len());
                    let Some(min_end) = next_field_stop(e, fixed) else { return false };
                    let end = at.max(min_end);
                    e.set_selection(fixed, end);
                }
            }
            if (e.anchor, e.focus) == before {
                false
            } else {
                e.break_history_group();
                true
            }
        };
        if changed {
            if let Some(f) = selection_change {
                let mut one = self.transaction(app);
                one.commit = one.commit.instant();
                f(app.states(), &mut one);
                self.latch_requests(app, &mut one);
                merge(&mut self.frame.pending, one);
            }
            self.commit.dirty = true;
        }
        changed
    }

    /// Moves a grabbed `El::selectable` handle to view point `(x, y)`, stopping short of the
    /// opposite end so the selection never inverts or collapses.
    pub(in crate::runtime) fn drag_selectable_handle_view(&mut self, x: f32, y: f32) -> bool {
        let HandleDrag::Selectable { handle, .. } = self.edit.handle_drag else {
            return false;
        };
        let Some(h) = self.selection_holder_end() else {
            self.touch_handle_release();
            return false;
        };
        let Some(fixed) = self.edit.selectable.phase.selection().map(|s| s.anchor) else {
            self.touch_handle_release();
            return false;
        };
        let g = self.end_group_path(h).and_then(|gp| self.live(gp));
        let Some((mut j, mut at)) = (match (g, h) {
            (Some(g), _) => self.group_point(g, h, x, y),
            (None, EndRef::Mounted(hi)) => self.text_index_at(hi, x, y).map(|at| (hi, at)),
            (None, EndRef::Offscreen(..)) => None,
        }) else {
            return false;
        };

        let h_ord = self.tree_order(h);
        let j_ord = self.tree_order(EndRef::Mounted(j));
        let h_len = self.end_content(h).len();

        match handle {
            Handle::End => {
                if j_ord < h_ord || (j_ord == h_ord && at <= fixed) {
                    let EndRef::Mounted(hi) = h else { return false };
                    let Some(min_end) = next_char_boundary(self.content_of(hi), fixed) else {
                        return false;
                    };
                    j = hi;
                    at = min_end;
                } else if j_ord > h_ord && at == 0 && fixed >= h_len {
                    if let Some(min_end) = next_char_boundary(self.content_of(j), 0) {
                        at = min_end;
                    }
                }
            }
            Handle::Start => {
                if j_ord > h_ord || (j_ord == h_ord && at >= fixed) {
                    let EndRef::Mounted(hi) = h else { return false };
                    let Some(max_start) = prev_char_boundary(self.content_of(hi), fixed) else {
                        return false;
                    };
                    j = hi;
                    at = max_start;
                } else if j_ord < h_ord && at >= self.content_of(j).len() && fixed == 0 {
                    if let Some(max_start) = prev_char_boundary(self.content_of(j), self.content_of(j).len()) {
                        at = max_start;
                    }
                }
            }
            Handle::Caret => return false,
        }

        let same_as_anchor = EndRef::Mounted(j) == h;
        let far_info = (!same_as_anchor).then(|| (self.tree.nodes[j].path.clone(), self.content_of(j).to_owned()));
        let changed = if let Some(sel) = self.edit.selectable.phase.selection_mut() {
            if let Some((path, content)) = far_info {
                let was_same = sel.far.as_ref().is_some_and(|f| f.path == path) && sel.focus == at;
                if was_same {
                    false
                } else {
                    sel.focus = at;
                    sel.far = Some(phase::Far::new(path, content));
                    true
                }
            } else {
                let was_same = sel.far.is_none() && sel.focus == at;
                if was_same {
                    false
                } else {
                    sel.focus = at;
                    sel.far = None;
                    true
                }
            }
        } else {
            false
        };
        if changed {
            self.seal_between();
        }
        changed
    }

    /// Queues a fresh native text menu (`text_menu: true`) for the currently focused field or
    /// non-empty `El::selectable` selection without moving the caret or selection. Used when
    /// reopening the Android floating `ActionMode` after a handle drag, Select All, or tapping
    /// the caret handle.
    pub(crate) fn open_selection_text_menu(&mut self, app: &mut dyn AppDriver) -> bool {
        if self.front_is_text_menu() {
            let _ = self.resolve_popup(app, None);
        }
        if let Some(i) = self.focused_index()
            && !self.subtree_exiting(i)
            && !self.node_disabled(i)
            && self.tree.nodes[i].input.as_deref().is_some_and(|inp| inp.access.is_some() && inp.edit.preedit().is_none())
        {
            let path = self.tree.nodes[i].path.clone();
            let Some((menu, field)) = self.build_field_menu(app, i, path) else {
                return false;
            };
            self.host.popups.push_back(crate::popup_menu::Request {
                menu,
                anchor: crate::PopupAnchor::Point(0.0, 0.0),
                on_pick: Box::new(|_, _, _| {}),
                field: Some(Box::new(field)),
                text_menu: true,
            });
            return true;
        }
        let Some(text) = self.copy_text(app) else {
            return false;
        };
        let html = self.copy_html();
        let Some(menu) = link_menu::build_context_menu(link_menu::ContextMenuTarget {
            has_link: false,
            has_on_link: false,
            image: None,
            selectable: true,
            copy_enabled: true,
        }) else {
            return false;
        };
        self.host.popups.push_back(crate::popup_menu::Request {
            menu,
            anchor: crate::PopupAnchor::Point(0.0, 0.0),
            on_pick: Box::new(move |_, picked, tx| {
                if picked == Some("copy") {
                    match html {
                        Some(html) => tx.copy_rich(crate::ClipboardContent::new(text).with_html(html)),
                        None => tx.copy(text),
                    }
                }
            }),
            field: None,
            text_menu: true,
        });
        true
    }
}

#[cfg(test)]
#[path = "touch_ui_tests.rs"]
mod tests;
