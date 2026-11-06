- **A document page's semantic mirror costs nothing to hold still, and a page's pictures below
  the fold wait for the reader.** `mirror_rect` added the page's scroll offset back in `f32` and
  rounded twice, where a fractional device-pixel offset can round the same document position two
  ways between frames; it now adds the offset in `f64` and rounds once. `subtree_end` walked the
  parent chain per node (a page's depth per node, on every mirror build); one reverse pass gives
  every node's subtree end. And `syncA11y` wrote `style.cssText` on every node of the mirror
  whenever a single item changed, invalidating a page's style and layout mid-scroll: it now writes
  only the nodes whose style actually changed. Separately, the site's pictures below the fold are
  requested when their section comes on screen instead of on the first frame, and never at more
  than 2.2 times their box, so a phone stops decoding and uploading 20 MB of AVIFs while the
  reader starts scrolling.
