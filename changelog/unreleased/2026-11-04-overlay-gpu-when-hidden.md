- **Overlays can hold no GPU memory while hidden: `ConfiguredOverlay::release_gpu_when_hidden`.**
  On Windows and X11 such an overlay, prewarmed, makes only its window at startup, so
  `on_created` and capture exclusion still come then, and makes its GPU surface, swapchain and
  renderer at each show; each hide drops them, and the GPU device goes with the last surface on
  it. An app whose only surfaces are hidden overlays of this kind (a tray app's drawing layer and
  picker) makes no GPU device until one shows. The show waits for what it makes: on an RTX 5070 Ti,
  a full-screen surface on a device another surface holds presents its first frame about 16 ms
  after `show`; with no device, about 600 to 850 ms (adapter enumeration, the device and the
  pipelines). Wayland makes nothing until the show. macOS ignores it: a prewarmed panel keeps its
  pixels. Other overlays are unchanged, except that on Windows and X11 the window's input styles
  are now set before its surface is made rather than after. `rune::on_gpu` reports when Rune is
  about to make a GPU device for a window or overlay (`GpuEvent::Starting`) and when a frame
  first presents on it (`GpuEvent::Drew`), so an app can guard its launches against a crashing
  GPU driver only when it draws.
