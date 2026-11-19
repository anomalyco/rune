- **Cold style setters reuse the style's `extra` Box instead of re-boxing.** A `CompactStyle`
  setter that can't pack inline (`set_absolute_inset`, a `set_width` on an already-boxed style)
  used to unpack the whole style to the stack, run the setter, and pack it back into a fresh
  allocation: a boxed style took an allocation and a dealloc per cold setter, 16 per cell in a
  dense list view. The setter now runs on the Box where it is, the inline mirror is repacked
  from it by the same code `from_style` uses, and the Box is kept — dropping it only when the
  style has become hot-packable, which is what `from_style` would have decided. Identical
  layout output (element rects byte-for-byte); in the 1,000-cell stress window, 10,000 fewer
  allocations and 5.5 MB fewer allocated bytes per frame.
