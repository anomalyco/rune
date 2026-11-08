- **A covered window's due timer runs when its frame can't come, instead of pinning a core.** On
  Wayland winit withholds `RedrawRequested` while a surface frame callback is outstanding, and a
  compositor may not send one for a fully covered surface. A due `wake_after` timer asked for that
  frame, stayed due when it never came, and `wake_deadline` returned the past deadline, so the
  event loop woke at once for as long as the window stayed covered (Hex at 100% CPU). The host now
  records the frame a timer asked for and, after `OCCLUDED_RETRY`, runs the timer covered and gives
  the next ask the same bound; a frame that arrives promptly behaves as before.
