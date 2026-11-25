//! The host's side of drag-outs: starting OS drag sessions and reporting how they ended.

use super::*;

impl RuntimeCore {
    /// A drag-out the pointer move just delivered asked for. The shell must start the OS
    /// session before returning from that move event.
    /// The drag to start, its rects and press in window points (the view's times its zoom).
    pub(crate) fn take_drag_out(&mut self) -> Option<crate::drag_out::DragOutStart> {
        let k = self.host.zoom();
        self.pointer.drag_out_request.take().map(|s| crate::drag_out::DragOutStart {
            rect: s.rect.map(|v| v * k),
            bounds: s.bounds.map(|v| v * k),
            press: (s.press.0 * k, s.press.1 * k),
            ..s
        })
    }

    /// Where the host reports how a drag-out ended; delivered to `on_drag_end` next frame.
    pub(crate) fn drag_end_sink(&self) -> crate::drag_out::DragEnds { self.pointer.drag_ends.clone() }
    pub(crate) fn drag_ends_pending(&self) -> bool { !self.pointer.drag_ends.borrow().is_empty() }

    /// Whether this host can start OS drags. Without, `drag_out` elements act as if they had
    /// none.
    pub(crate) fn set_os_drag(&mut self, supported: bool) { self.pointer.os_drag = supported; }

    pub(super) fn drags_out(&self, n: &Node) -> bool { self.pointer.os_drag && n.handlers.get().is_some_and(|h| h.drag_out.is_some()) }

    /// Whether the live, allowed view tree currently mounts any `El::drag_out` element while the
    /// host supports OS drags. Matches `has_drop_targets`: exiting subtrees and elements behind an
    /// active modal do not count.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn has_drag_out(&self) -> bool {
        self.pointer.os_drag
            && (0..self.tree.nodes.len()).any(|i| {
                !self.subtree_exiting(i)
                    && self.allowed(i)
                    && self.tree.nodes[i].handlers.get().is_some_and(|h| h.drag_out.is_some())
            })
    }

    /// Whether an outgoing OS drag session started from this runtime is still awaiting `on_drag_end`.
    #[cfg_attr(not(any(test, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))), allow(dead_code))]
    pub(crate) fn has_active_drag_out(&self) -> bool {
        self.pointer.drags_open > 0
    }
}
