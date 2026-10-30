- **`on_ready` runs for a window launched out of sight.** A window launched covered, occluded,
  on another Space or hidden by the system (`open -g`) could go without a drawn first frame until
  it was uncovered, so `App::on_ready`, and the live work an app starts there, never ran. It now
  runs at the first frame skipped for being off screen, or a second after the window opens when
  no frame comes at all. A window on screen still runs it after its first frame, and
  `WindowHandle::is_ready` still waits for a drawn one.
