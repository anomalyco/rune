- **Layout continues onto demand-paged stack segments when remaining stack is low.** Deeply nested
  element trees (such as a Markdown list nested 60 levels deep on Windows' 1 MiB main-thread stack)
  previously overflowed the call stack inside `rune_taffy`'s recursive layout pass.
  `LayoutStoreView::compute_child_layout_inner` now continues onto 1 MiB stack segments via
  `stacker` when remaining stack drops below 128 KiB on desktop targets.
