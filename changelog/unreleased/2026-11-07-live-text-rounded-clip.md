- **Live text inside a rounded clip no longer draws through its own offscreen layer when it sits
  clear of the corners.** Live and truncated text clip their glyphs to their box, which used to
  force a layer whenever an ancestor clipped with rounded corners, one render pass per label. A
  label whose clip rectangle lies inside the rounded mask now draws on the direct path. A wall of
  512 cards with three live labels each went from about 82 ms to under 3 ms of encode and submit
  per frame.
