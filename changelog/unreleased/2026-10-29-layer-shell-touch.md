- **A finger can tap a Wayland layer-shell overlay.** The overlay bound `wl_pointer` and `wl_keyboard`
  but never `wl_touch`, so cards and buttons drawn on a layer surface ignored a touchscreen even
  though the compositor offered touch. A touch on the surface now acts as the primary button: down
  presses where the finger landed, motion follows it, and up releases and clears hover, so a tap
  clicks and a drag off a button doesn't. A touch the compositor cancels (a system gesture) leaves
  before it releases, so it never counts as a click. One finger at a time; extra fingers are ignored.
