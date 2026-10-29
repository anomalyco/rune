- **Wayland overlays take right-clicks.** A layer-shell overlay passed on only the left
  button, so `on_secondary_press` never fired on an overlay. Its right button now opens the
  secondary gesture, as a window's does.
