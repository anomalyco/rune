//! Linux-specific window host pieces: the first frames drawn on the CPU and the hand-over to
//! the GPU.

pub(in crate::window) mod cpu_frame;
pub(in crate::window) mod handover;
