- **Uncached deep container layouts continue onto stack segments when remaining stack is low.**
  Deeply nested container trees (such as a Markdown list nested 60 levels deep on a 1 MiB thread
  stack) previously overflowed the call stack inside `rune_taffy`'s recursive layout pass.
  `LayoutStoreView::compute_child_layout_inner` now checks remaining stack inside the cache-miss
  container path on Windows, macOS, and Linux, continuing onto a new `stacker` segment below
  `128 KiB` (with a `16 KiB` fiber stack guarantee set on Windows) so cache hits and leaf nodes
  never grow and siblings share their parent container's segment.
