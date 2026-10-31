- **Layout reads don't follow scrolling.** `Cx::layout`, `layout_with`, `layout_size` and
  `memo::layout` now give where layout puts an element with every scroll viewport at its
  origin, so a view reading a row inside a scrolling list no longer re-runs every frame of the
  scroll; a commit that moves or resizes the element still re-runs it. A view that needs where
  the window shows the element now, after scrolling, reads `Cx::window_layout` or
  `window_layout_with` (and `memo::window_layout`). `El::on_layout` still reports after scrolling,
  and the new `El::on_layout_tx` takes a `Tx`, so a handler that only moves live values says
  `Tx::live_only` and a scroll runs no view.
