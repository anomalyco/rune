- **What mounts out of sight enters at once.** An element new under one that drew nothing last
  frame and still lies outside the window or its clips (a ticking digit in a row scrolled out of
  view, or in a virtual list's overscan) skips its entrance, as its exit already did, so nothing
  animates where nobody sees it. A matched element still flies to its match.
- **Colours out of sight land at once.** A node that drew nothing last frame and still lies
  outside the window or its clips lands its colour, hover and blur changes instead of fading them
  where nobody sees them (a list's selected row scrolled out of view when focus moves away).
