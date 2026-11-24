//! Tests: CPU frames with no display behind them, which count what a display would have had.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{Canvas, Put};

pub(crate) struct Frames {
    /// Frames sent so far (a Wayland commit with a buffer, an X11 image).
    pub(crate) sent: Arc<AtomicUsize>,
    /// Stands for a Wayland compositor with `wp_linux_drm_syncobj_manager_v1`.
    pub(crate) explicit_sync: bool,
    buffer: Vec<u8>,
    size: (u32, u32),
    painted: bool,
}

impl Frames {
    pub(crate) fn new(explicit_sync: bool) -> (super::Frames, Arc<AtomicUsize>) {
        let sent = Arc::new(AtomicUsize::new(0));
        (super::Frames::Fake(Frames { sent: sent.clone(), explicit_sync, buffer: Vec::new(), size: (0, 0), painted: false }), sent)
    }

    pub(super) fn begin(&mut self, size: (u32, u32)) -> Option<Canvas<'_>> {
        if self.size != size {
            self.buffer = vec![0; size.0 as usize * size.1 as usize * 4];
            self.size = size;
            self.painted = false;
        }
        Some(Canvas { bytes: &mut self.buffer, stride: size.0 as usize * 4, kept: self.painted })
    }

    pub(super) fn present(&mut self, put: Put<'_>) -> bool {
        self.painted = true;
        if matches!(put, Put::Rects([])) { return false; }
        self.sent.fetch_add(1, Ordering::Relaxed);
        true
    }

    pub(super) fn forget(&mut self) { self.painted = false; }

    pub(super) fn last(&self) -> Option<(Vec<u8>, (u32, u32))> {
        self.painted.then(|| (self.buffer.clone(), self.size))
    }
}
