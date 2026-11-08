- **An XWayland overlay's hit regions follow every frame it draws.** A drawn frame never set the
  surface's input shape, and only a frame that drew nothing new did, so an overlay that redraws
  each frame (a capsule with a ticking clock) kept the empty shape it was created with and took no
  clicks on a Wayland desktop without layer-shell. Each drawn frame now sets the shape from the
  geometry it drew, before its pixels present.
