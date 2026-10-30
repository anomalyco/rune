- **A Metal frame is one command buffer, submitted and presented where it was drawn.** On
  `RUNE_GPU_BACKEND=metal`, overlays submit and present on their own thread instead of handing
  the frame to a present thread, which only paid off for wgpu's costlier submit. A
  `Queue::write_buffer` to a buffer that only submitted work still reads takes a spare storage
  instead of a staged copy, so a compositor writing a uniform each frame adds no command buffer.
  `examples/window_scroll_cost.rs` scrolls a real window and prints its CPU, instructions and
  Metal calls per frame; `scripts/motion-audit` records a sticky-header scroll.
