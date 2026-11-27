- **Wayland windows present Mailbox when the surface offers it.** A window there already paces
  each frame on the compositor's frame callback (`pre_present_notify`); presenting FIFO added a
  wait for the compositor to latch each commit (the Vulkan driver's `wp_fifo_v1` barrier), which
  a compositor with nothing else to repaint can leave unanswered. In a headless GNOME Shell an
  animation's frames stopped mid-fade for up to 17 s, until the pointer moved; with Mailbox every
  frame lands, still paced at the display's rate. X11, Windows, macOS, overlays and embedded views
  keep `AutoVsync`.
