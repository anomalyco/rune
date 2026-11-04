- **Borderless app windows on Windows and Linux minimize and resize like titled windows.**
  `Tx::minimize()` works without a title bar, and a resizable window's edges and corners
  take resize drags ahead of `.window_drag()` regions. Windows keeps the system shadow.
  Use `Tx::toggle_maximize()` in a custom title bar to maximize or restore the window.
