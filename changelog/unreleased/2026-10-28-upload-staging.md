- **A steady animating frame allocates no GPU buffers.** Per-frame uploads in `Renderer`,
  `FxRenderer`, `LightRenderer` and `GlassRenderer` go through reused staging chunks (the new
  `rune-staging` crate) instead of `Queue::write_buffer`/`write_texture`, which allocate a fresh
  staging buffer each call on native backends. `Renderer::prepare` writes only the instances that
  changed and writes globals only when the viewport or sRGB flag changes. Its uploads now land
  ahead of the renderer's next draw, recorded into that draw's encoder, or are submitted ahead of
  `draw_range*` and `Renderer::submit`, so submit the encoder a renderer drew into. One dropped
  unsubmitted is detected, and the next frame uploads everything again. On the web, writes still
  go straight to the queue. A headless 2,000-instance frame takes about half the CPU time to
  record and submit (`examples/upload_bench.rs`).
