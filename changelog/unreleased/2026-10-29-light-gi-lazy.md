- **rune-light allocates GI only when GI runs, and a bigger scene keeps its render targets.** With
  `Settings::gi` off, `LightRenderer` makes none of the GI textures (media, five cascades, fluence)
  and records no empty GI passes when timing; they're made the first frame GI is on.
  `LightRenderer::inspect` returns black views for the GI buffers until then, and `Timings::gi` is
  0 for a frame without GI. Growing past the primitive buffer's capacity now rebuilds only the two
  bind groups that read it instead of reallocating every render target.
