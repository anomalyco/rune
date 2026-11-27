- **`rune::prewarm_gpu_with(|gpu| ..)` hands an app the prewarmed device before the first frame.**
  On macOS it starts `prewarm_gpu` and runs the closure on a thread of its own with a
  `GpuHandle` as soon as the prewarmed device exists (at once if it already does), so an app
  builds its pipelines while the window is made instead of in its first frame. The handle is the
  one the first window's frames get from `Frame::gpu_handle` when that window draws with the
  prewarmed device (same `key`); when it doesn't, the device is dropped and the handle reports
  `is_lost`. Elsewhere the closure never runs.
