- **`El::shrink` keeps an explicit minimum width.** It set the minimum width to 0 every time, so
  `.min_w(160.0).shrink()` lost the 160 while `.shrink().min_w(160.0)` kept it. It now drops only
  an automatic minimum width, so `min_w` holds in either order. An element that relied on
  `shrink` clearing an earlier `min_w` now keeps that minimum; use `.min_w(0.0)` to clear it.
