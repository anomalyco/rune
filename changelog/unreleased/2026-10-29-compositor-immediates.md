- **A compositor that asks for `wgpu::Features::IMMEDIATES` can use immediates.**
  `Compositor::features` returning `IMMEDIATES` now also raises the device's
  `max_immediate_size` to the adapter's (4 KB on Metal), so per-draw values set with
  `RenderPass::set_immediates` travel inside the pass instead of in a uniform written through
  `Queue::write_buffer` or `Frame::write_buffer`, which costs a macOS overlay frame another
  command buffer.
