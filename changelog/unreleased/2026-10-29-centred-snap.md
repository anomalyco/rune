- **A box centred in a painted parent stays centred on the device grid.** Rounding each edge on
  its own put a compact `Switch`'s knob 2 px from the top of its track and 3 px from the bottom
  at 1.25× (and the same for every `ControlSize` at some fractional scale): its 2 pt inset is
  2.5 px. Pixel snapping now gives a box whose resting edges are centred in its parent's equal
  whole-pixel gaps to the parent's snapped edges, and lets its size round by up to a pixel to
  make room: the Small knob draws 19 px in a 25 px track, 3 px from each edge. Only a parent
  that paints something (a fill, border, gradient, or hover or press colour) counts, so a box
  centred in a plain row keeps its own rounding. It is decided at rest, so a box gliding into
  or out of a centred spot, or one whose parent grows or shrinks around it, eases between the
  two roundings with no jump.
