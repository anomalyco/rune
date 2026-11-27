- **Rune's device takes the adapter's buffer and binding sizes.** `Frame::device` now allows
  `max_buffer_size`, `max_storage_buffer_binding_size` and `max_uniform_buffer_binding_size` up to
  what the adapter does, instead of wgpu's defaults (a 128 MiB storage binding), so an embedded
  compute renderer can bind work buffers past them on big windows. The native Metal adapter
  reports the device's own sizes and compute workgroup limits (capped at `u32::MAX`, as wgpu's
  are) instead of wgpu's defaults. Nothing Rune draws changes.
