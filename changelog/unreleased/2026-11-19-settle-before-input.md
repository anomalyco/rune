- **A host settle lands the tree a frame began with, not the change that frame's input makes.** When
  an uncover, a restore or a shown window called `Runtime::settle_all()` on the same frame as a
  press or key, the frame's whole commit was forced to land: a dialog that press closed was
  unmounted with no exit, and so was any keyed layer the input removed. The settle now commits the
  tree as it stood before the frame's input, and the input's own commit animates, so a dialog
  closed by the first press after the window comes back still plays its exit.
