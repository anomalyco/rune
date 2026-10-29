- **Windows and overlays blend in sRGB-encoded space by default on every platform, as Core
  Animation, CoreText and CSS do.** Text drawn at the colour's full coverage (no compensating coverage curve)
  now has native weight in light and dark themes, and translucent fills, shadows and blurs
  composite as AppKit's would. `App::srgb_blend(false)` (or the overlay's) keeps linear-light
  blending. Rune's Metal layer is now tagged sRGB, so a wide-gamut display colour-matches its
  pixels instead of showing them as display-native values.
