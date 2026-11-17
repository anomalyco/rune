- **Layout, commit, Markdown rendering and element drops survive deeply nested trees.** On
  Windows' 1 MiB main-thread stack, a Markdown list nested 60 levels deep overflowed the call
  stack inside `rune_taffy`'s recursive layout pass, and deeper trees overflowed element
  flattening, Markdown block rendering and recursive `El` destructors on every desktop target.
  `rune_taffy`'s `LayoutStoreView::compute_child_layout_inner`, `flatten`, virtual-list walks,
  key diagnostics, accessibility JSON dumps and `Markdown` container rendering now continue onto
  demand-paged 1 MiB stack segments through `stacker` when remaining stack falls below 128 KiB,
  while `El` and cached Markdown parses drop iteratively, `memo` and `Markdown` margin,
  direction, search and streaming walks run iteratively, and `Markdown` list copy-indent tracking
  no longer overflows `u8` past 127 levels.
