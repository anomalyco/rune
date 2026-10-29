- **Scaled content lands on whole device pixels once its scale holds still.** Pixel snapping
  skipped everything under an `El::scale` other than 1, so a canvas zoomed to 1.37 drew its
  boxes at edges like 548.61 px, soft and up to half a pixel off unscaled outlines drawn over
  it. A node with its own scale now heads a layer: while the layer's drawn rect moves (a zoom,
  a pinch, a spring) its content keeps its exact sub-pixel edges, and once it holds still the
  correction to the grid eases in over 0.1 s, so a zoom ends with no pop. When it moves again,
  the resting correction fades out over the first pixel its edges travel. Cached subtrees snap
  at once. `El::pixel_snap(false)` still opts a subtree out.
