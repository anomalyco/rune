- **Uploads write in place on Metal, and only the instances that changed.** On
  `RUNE_GPU_BACKEND=metal`, buffer writes through `Staging` and `Queue::write_buffer` go straight
  into the buffer's shared memory; while the GPU may still read the old bytes, a spare brought up
  to date takes the buffer's place, so a frame records no copy and allocates no buffer.
  `rune_staging::set_direct` registers such a writer, and `Stats::direct` counts its writes. On
  every backend, `Renderer::prepare` writes each span of instances that changed instead of
  everything from the first to the last, so a frame where a few far-apart instances move uploads
  only those. `examples/upload_bench.rs` also prints the instructions a frame ran (`kinstr`), which
  a busy machine leaves unchanged.
