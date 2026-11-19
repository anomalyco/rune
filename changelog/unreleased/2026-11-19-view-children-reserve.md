- **`Cx::view_children()` tells a view how many children its root had last frame, and
  `El::reserve_children(n)` reserves them.** A view that rebuilds a large subtree from state
  every frame grows its children `Vec` from zero by doubling: the 1,000-cell storm window's
  root folded 4,001 children each frame, so every frame re-allocated at 2,048 and 4,096 and
  copied 2.3 MB. Reading the last frame's count and reserving first lands the fold at its
  full size in one allocation: 1.5 MB less allocated per frame in the storm, with identical
  layout output.
