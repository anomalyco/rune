- **Retained no-second-tree layout store (`rune-taffy`).** `Runtime` now drives Taffy 0.15's
  layout solver directly from 216-byte dense `RetainedSlot`s (`LayoutStore`), embedding a 64-byte
  `CompactStyle` directly on `El` (shrinking `El` by 488 B across `view` and `MemoCache`), pairing
  each slot with a 96-byte `CompactCache` and 32-byte `BoxLayout`, and syncing children through
  flat double-buffered arenas without per-container `Vec` allocations. On a 500-row list this cuts
  retained layout heap by 80% (5.52 MB to 1.10 MB; 5.51 MB to 35 KB after shrinking to 12 rows),
  and on a 1,000-row theme-stack rebuild benchmark drops `commit` p50 by 15% and rebuild p50 by 0.62 ms.
