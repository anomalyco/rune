- **Glyph pens floor to CoreGraphics' subpixel steps,
  as CoreText's quantisation does.** The step depends on the glyph's size in device pixels:
  fifths of a pixel below 8⅓ ppem, quarters to 11.1, thirds to 16.7, halves to 33.3 and whole
  pixels above, at every scale. Glyphs used to round to the nearest quarter pixel. Measured
  against CoreText, the report paragraph's mean error falls to 0.06 levels at 2× and 0.10 at 1×.
