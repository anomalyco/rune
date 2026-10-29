- **macOS overlays finish, submit and present off the main thread, and compositors can stage
  buffer writes.** An overlay frame with no system materials, no presents-with-transaction and
  no native video surface now hands its encoder and drawable to a per-overlay present thread,
  which runs `CommandEncoder::finish`, `Queue::submit` and `Queue::present`; the main thread
  waits for it only before it next touches the surface. An overlay animating at 120 Hz spends
  about half the main-thread time per frame it did. `Frame::write_buffer(buffer, offset, data)`
  writes a buffer through Rune's staging chunks and records the copy into the frame's encoder,
  so a compositor's per-frame uniform writes reuse staging memory instead of
  `Queue::write_buffer`'s per-call staging buffer, and land ahead of what it records next.
