- **Compact layout-tree storage (`rune-taffy`).** `Runtime` now backs Taffy's layout solver with a
  dense `SparseSet` of 364-byte node slots (96-byte packed `CompactStyle`, 112-byte inline
  `CompactCache` with lazy overflow, inline `SmallChildren` for up to two children, and a single
  inline `Layout` when pixel rounding is off) instead of `taffy::TaffyTree`'s four `SlotMap`s and
  1,272-byte slots, cutting retained layout-tree heap by 71% on a 500-row list (5.52 MB to 1.61 MB)
  and shrinking excess capacity after a large-to-small view swap (5.51 MB to 99 KB).
