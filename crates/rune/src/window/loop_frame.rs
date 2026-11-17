//! A frame only a loop asked for: a spinner's next dot, a shimmer's next step. The runtime
//! samples the loops into the instances they drew and nothing else runs (`Runtime::loop_step`:
//! no poll, view, layout or paint), and the window redraws and presents only where they draw
//! (`Renderer::arm_loop`). Anything that reached the window since its last frame (input, a
//! wake, a request, an assistive client's action) makes the next one an ordinary frame, so
//! nothing waits behind a loop.

#[derive(Default)]
pub(super) struct LoopFrame {
    touched: bool,
}

impl LoopFrame {
    /// Something other than the window's own frames reached it.
    pub(super) fn touched(&mut self) { self.touched = true; }

    /// A frame begins: whether only its loops asked for it since the last. Counts afresh.
    pub(super) fn only_loops(&mut self) -> bool { !std::mem::replace(&mut self.touched, false) }
}
