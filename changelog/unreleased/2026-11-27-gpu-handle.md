- **`Frame::gpu_handle` lends Rune's device and queue to worker threads.** A `GpuHandle`
  (`Clone`, `Send + Sync`) runs `with(|device, queue| ..)` on any thread, so a compositor's
  renderer can upload buffers and atlases and build pipelines ahead of the frame instead of inside
  `Compositor::render`. It holds the device weakly: once the device is lost or Rune lets it go
  (replaced after a loss, window closed), `is_lost()` is true and `with` returns `None`, so a
  worker never keeps a lost device alive. `key()` matches `Frame::device_key`, and the next frame
  on a new device hands out a new handle.
