- **A scaled layer snaps the same wherever its scale is anchored.** Content within a pixel of
  a layer's anchor snapped to the unscaled grid while the rest of the layer drew exact, and a
  layer's resting correction faded out by how far that layer's own edges moved, so outlines on
  an overlay anchored elsewhere drifted up to 0.2 px off the art beneath them during a zoom.
  The unscaled-grid weight is now one value per layer, taken from the layer's edges. Each box
  lets go of its resting correction over the first pixel that box moves, so content at the
  anchor stays put instead of drifting. A layer counts as moving by how far it moves points
  of the window.
