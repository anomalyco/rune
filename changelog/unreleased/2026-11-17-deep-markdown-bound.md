- **Markdown nests lists and block quotes to Chromium's 512-element parser bound and flattens
  deeper items in place.** `Markdown` now tracks open container elements across nested lists,
  list items and block quotes up to 510 elements (255 list levels or 510 block quotes, matching
  Chromium's 512-element HTML parser cap after `<html>` and `<body>` in `DOMParser`), flattening
  deeper list items as siblings at the final list level and deeper block quotes into the final
  quote. List copy-indent tracking widens from `u8` to `usize` so copying past 127 levels no
  longer wraps, and collapsed-margin scanning walks first-child and last-child edges separately
  instead of branching `2^D` times on single-child nested containers.
