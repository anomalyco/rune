- **Fold rows that open or shut together move as one block.** Consecutive `fold_glide` rows that a
  commit brings into a growth stack (or takes out of it) share one spring, which fills them top to
  bottom (or empties them bottom to top) under a single feathered edge, as a code diff opens a
  hunk. Before, each row opened on its own spring at once, every row half open under its own
  edge. A later change to any row breaks the block and each row carries on from where it is. A
  `fold_glide` row that leaves also shuts at once on its own spring, instead of fading for 0.15 s
  before its space starts to close.
