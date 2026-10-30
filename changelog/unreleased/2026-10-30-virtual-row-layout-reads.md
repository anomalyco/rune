- **`Cx::layout` and `Cx::layout_with` read an element inside `virtual_rows` (or `virtualize`)
  where it draws.** With rows culled above the mounted window, the rect read was the culled
  rows' height too far down.
