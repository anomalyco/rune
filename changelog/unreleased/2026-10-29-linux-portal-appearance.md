- **Linux follows the desktop's light or dark preference.** winit reports no theme on Wayland
  (and layer surfaces have none), so windows and overlays there always drew dark. When winit has
  nothing they now read the XDG desktop portal's `org.freedesktop.appearance` `color-scheme`: a
  worker thread reads it once and follows `SettingChanged`, a change asks every window to redraw
  (as a new accent does) and the redraw rereads it, and overlays read it each time they show. The
  first window waits at most 100 ms for the first answer. "No preference", no portal or no session
  bus keep dark as before, and `--theme` and winit's own theme still win.
