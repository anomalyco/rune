- **`MarkdownStyle::code_margin(top, bottom)` sets the space around code blocks, diffs and
  diagrams.** It was fixed at 12 above and 24 below, so a block sat closer to the paragraph that
  introduced it than to the one after it. The default is unchanged; an app with its own rhythm
  can now give a block equal room on both sides.
