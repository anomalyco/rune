- **A dimmed box with a solid fill fades as CSS `opacity` does.** `El::dim`, which disabled
  kit controls use, no longer lets the box's outer shadows show through its faded fill, and
  a fill under a gradient (a contrast button's sheen) fades with it as one, still without a
  layer. A disabled contrast button on white now paints the browser's light grey (about 176
  instead of 159 mid-height with the OpenCode theme), and a disabled neutral button no longer
  shows its drop shadow through. A crisp offset shadow under a native material, backdrop blur
  or dimmed box now leaves the interior clear too.
